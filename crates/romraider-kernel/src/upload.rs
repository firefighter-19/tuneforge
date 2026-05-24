//! Upload-последовательность: загрузить kernel-binary в RAM ECU и передать
//! ему управление.
//!
//! См. `nisprog/np_cli.c::cmd_sprunkernel()` (GPL-3.0) для C-эталона.
//!
//! Высокоуровневые шаги:
//!
//! 1. **SID 0x81** `startCommunication` — войти в KWP2000-сессию (4800 baud).
//! 2. **SID 0x27** `securityAccess` (seed/key challenge — см. [`crate::seed_key`]).
//!    Sub `0x01` — запросить seed, sub `0x02` — отправить key.
//! 3. **SID 0x10 0x85 0x02** `startDiagnosticSession` (programming mode).
//! 4. **(хост-side)** Переключиться на повышенный baud (обычно 15625).
//! 5. **SID 0x34** `requestDownload` — объявить адрес и длину upload-а:
//!    `34 <AH AM AL> 04 <LH LM LL>` (`04` = «uncompressed + encrypted»).
//! 6. **SID 0x36** `transferData` — заливаем encrypted-kernel блоками по 128 байт.
//!    Encrypt применяется к **всему буферу сразу** перед chunking-ом
//!    (см. `crate::seed_key::subaru_encrypt_buffer`), а не per-chunk.
//! 7. Повторить SID 0x34 + SID 0x36 для **4-байт trailer `00 00 5A A5`**
//!    (тоже encrypted) по адресу `load_addr + pl_len`. Boot ROM ECU проверяет
//!    этот magic перед allow trampoline jump.
//! 8. **SID 0x31** `startRoutineByAddress` с под-функцией `01 01` — ECU
//!    прыгает на trampoline в RAM (для SH7058 — `0xFFFF3004`).
//!
//! После SID 0x31 общение продолжается с kernel-side wire protocol —
//! см. [`crate::kernel_wire`].

use std::time::Duration;

use romraider_io::transport::Transport;

use crate::error::{KernelError, KernelResult};
use crate::kernels::KernelBinary;
// `self` (т.е. module `kwp2000`) импортируется для use в `#[cfg(test)] mod tests`
// ниже, где `kwp2000::POSITIVE_RESPONSE_MASK` / `kwp2000::HEADER` нужны для
// сборки synthetic response-фреймов. Production-код использует только
// `request_response` и `sid`.
#[allow(unused_imports, clippy::single_component_path_imports)]
use crate::kwp2000::{self, request_response, sid};
use crate::seed_key::{subaru_encrypt_buffer, subaru_genkey};

/// Маркер trailer-а, который проверяет boot-ROM ECU перед allow jump.
/// Заливается отдельным SID 0x34 + SID 0x36 после основного payload.
pub const CHECKSUM_BYPASS_TRAILER: [u8; 4] = [0x00, 0x00, 0x5A, 0xA5];

/// Формат-байт в SID 0x34: `0x04` = «uncompressed + vehicle-specific encrypted».
const DOWNLOAD_FORMAT_BYTE: u8 = 0x04;

/// Размер одного chunk-а в SID 0x36 — фиксированный 128 байт (последний может быть короче).
const TRANSFER_CHUNK_SIZE: usize = 128;

/// Sub-function для startDiagnosticSession — programming mode.
/// Из nisprog: `txdata = [0x10, 0x85, 0x02]`.
const PROGRAMMING_SESSION_SUB: u8 = 0x85;
const PROGRAMMING_SESSION_SUBSUB: u8 = 0x02;

// ---------------------------------------------------------------------------
// Step 1: start communication
// ---------------------------------------------------------------------------

/// Открыть KWP2000-сессию (SID 0x81). Не требует payload-а.
/// Положительный ответ обычно содержит KB1+KB2 (key bytes ISO-14230).
pub fn start_communication(transport: &mut dyn Transport, timeout: Duration) -> KernelResult<()> {
    let _ = request_response(transport, sid::START_COMMUNICATION, &[], timeout)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 2: security access (seed/key)
// ---------------------------------------------------------------------------

/// Запросить seed (SID 0x27 sub `0x01`). Возвращает 4 байта seed-а.
pub fn request_security_seed(
    transport: &mut dyn Transport,
    timeout: Duration,
) -> KernelResult<[u8; 4]> {
    let frame = request_response(transport, sid::SECURITY_ACCESS, &[0x01], timeout)?;
    // Ожидаем `67 01 S3 S2 S1 S0` (sub-function echo + 4 bytes seed).
    if frame.data.len() != 5 || frame.data[0] != 0x01 {
        return Err(KernelError::UploadAborted(format!(
            "expected `67 01 S3 S2 S1 S0`, got {} bytes (data[0]=0x{:02X?})",
            frame.data.len(),
            frame.data.first(),
        )));
    }
    let seed = [frame.data[1], frame.data[2], frame.data[3], frame.data[4]];
    tracing::debug!(seed = ?seed, "received SID 0x27 seed");
    Ok(seed)
}

/// Отправить computed key (SID 0x27 sub `0x02`). На неудачный key ECU отвечает
/// `7F 27 35` ([`crate::kwp2000::nrc::INVALID_KEY`]).
pub fn submit_security_key(
    transport: &mut dyn Transport,
    key: [u8; 4],
    timeout: Duration,
) -> KernelResult<()> {
    let payload = [0x02, key[0], key[1], key[2], key[3]];
    let frame = request_response(transport, sid::SECURITY_ACCESS, &payload, timeout)?;
    // Ожидаем `67 02` (echo sub-function).
    if frame.data.first().copied() != Some(0x02) {
        return Err(KernelError::UploadAborted(format!(
            "expected `67 02`, got data={:02X?}",
            frame.data,
        )));
    }
    tracing::debug!("SID 0x27 unlock OK");
    Ok(())
}

/// Пройти полный seed/key handshake. Использует [`subaru_genkey`] для
/// вычисления ключа по seed-у.
pub fn unlock(transport: &mut dyn Transport, timeout: Duration) -> KernelResult<()> {
    let seed = request_security_seed(transport, timeout)?;
    let key = subaru_genkey(seed);
    tracing::debug!(seed = ?seed, key = ?key, "computed Subaru SID 0x27 key");
    submit_security_key(transport, key, timeout)
}

// ---------------------------------------------------------------------------
// Step 3: start programming session
// ---------------------------------------------------------------------------

/// Войти в programming-mode (SID 0x10 0x85 0x02).
pub fn start_programming_session(
    transport: &mut dyn Transport,
    timeout: Duration,
) -> KernelResult<()> {
    let _ = request_response(
        transport,
        sid::START_DIAGNOSTIC_SESSION,
        &[PROGRAMMING_SESSION_SUB, PROGRAMMING_SESSION_SUBSUB],
        timeout,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 5: request download (SID 0x34)
// ---------------------------------------------------------------------------

/// `34 <AH AM AL> 04 <LH LM LL>` — объявить адрес и длину блока, который
/// собираемся залить через SID 0x36. ECU отвечает положительным `74 [...]`.
pub fn request_download(
    transport: &mut dyn Transport,
    address: u32,
    length: u32,
    timeout: Duration,
) -> KernelResult<()> {
    let payload = [
        ((address >> 16) & 0xFF) as u8,
        ((address >> 8) & 0xFF) as u8,
        (address & 0xFF) as u8,
        DOWNLOAD_FORMAT_BYTE,
        ((length >> 16) & 0xFF) as u8,
        ((length >> 8) & 0xFF) as u8,
        (length & 0xFF) as u8,
    ];
    let _ = request_response(transport, sid::REQUEST_DOWNLOAD, &payload, timeout)?;
    tracing::debug!(addr = format!("0x{address:06X}"), length, "SID 0x34 OK");
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 6: transfer data (SID 0x36)
// ---------------------------------------------------------------------------

/// Залить **уже-зашифрованный** буфер по адресу `address` 128-байтными
/// чанками. Адрес инкрементируется на размер chunk-а после каждого.
/// Последний chunk может быть короче.
pub fn transfer_data_chunks(
    transport: &mut dyn Transport,
    address: u32,
    encrypted_buffer: &[u8],
    timeout: Duration,
) -> KernelResult<()> {
    let mut offset = 0usize;
    let mut addr = address;
    while offset < encrypted_buffer.len() {
        let end = (offset + TRANSFER_CHUNK_SIZE).min(encrypted_buffer.len());
        let chunk = &encrypted_buffer[offset..end];
        let chunk_len = chunk.len();

        let mut payload = Vec::with_capacity(3 + chunk_len);
        payload.extend_from_slice(&[
            ((addr >> 16) & 0xFF) as u8,
            ((addr >> 8) & 0xFF) as u8,
            (addr & 0xFF) as u8,
        ]);
        payload.extend_from_slice(chunk);

        let _ = request_response(transport, sid::TRANSFER_DATA, &payload, timeout)?;
        tracing::trace!(
            addr = format!("0x{addr:06X}"),
            chunk_len,
            "SID 0x36 chunk OK",
        );

        offset += chunk_len;
        addr += chunk_len as u32;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 8: start routine (SID 0x31 01 01)
// ---------------------------------------------------------------------------

/// Прыгнуть на trampoline в RAM. После этого ECU начинает исполнять kernel —
/// дальше общаемся через [`crate::kernel_wire`].
pub fn start_routine_jump(transport: &mut dyn Transport, timeout: Duration) -> KernelResult<()> {
    let _ = request_response(
        transport,
        sid::START_ROUTINE_BY_ADDRESS,
        &[0x01, 0x01],
        timeout,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Orchestrator: pad → encrypt → request_download → transfer chunks
// ---------------------------------------------------------------------------

/// Pad payload до multiple-of-4, encrypt, и залить через SID 0x34 + SID 0x36.
/// Возвращает реальную (padded) длину, использованную в request_download.
fn pad_encrypt_and_upload(
    transport: &mut dyn Transport,
    address: u32,
    plain: &[u8],
    timeout: Duration,
) -> KernelResult<u32> {
    // Subaru encrypt оперирует 4-байтными словами — паддинг 0xFF (как в nisprog).
    let mut padded = plain.to_vec();
    while padded.len() % 4 != 0 {
        padded.push(0xFF);
    }
    let padded_len = padded.len() as u32;
    let encrypted = subaru_encrypt_buffer(&padded);

    request_download(transport, address, padded_len, timeout)?;
    transfer_data_chunks(transport, address, &encrypted, timeout)?;
    Ok(padded_len)
}

/// Запустить полный upload-pipeline на уже-открытой и unlock-нутой сессии:
/// kernel-payload + trailer + jump.
///
/// **Предусловия:** должен быть вызван [`start_communication`], [`unlock`],
/// [`start_programming_session`] и (опционально) переключение хост-baud.
pub fn upload_and_jump(
    transport: &mut dyn Transport,
    kernel: &KernelBinary,
    timeout: Duration,
) -> KernelResult<()> {
    let load_addr = kernel.mcu.load_address();
    let kernel_len = pad_encrypt_and_upload(transport, load_addr, kernel.bytes, timeout)?;

    // Trailer `00 00 5A A5` (encrypted) по адресу load_addr + kernel_len.
    let trailer_addr = load_addr + kernel_len;
    let _ = pad_encrypt_and_upload(transport, trailer_addr, &CHECKSUM_BYPASS_TRAILER, timeout)?;

    tracing::info!(
        kernel = %kernel.identifier, load_addr = format!("0x{load_addr:08X}"),
        bytes = kernel.bytes.len(),
        "kernel uploaded; jumping to RAM",
    );

    start_routine_jump(transport, timeout)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use romraider_io::mock::MockTransport;
    use std::time::Duration;

    fn t() -> Duration {
        Duration::from_millis(100)
    }

    /// Хелпер: построить положительный KWP2000-ответ `80 F0 10 <len> <sid|0x40> [data...] <chksm>`.
    fn build_positive_response(req_sid: u8, data: &[u8]) -> Vec<u8> {
        let response_sid = req_sid | kwp2000::POSITIVE_RESPONSE_MASK;
        let length = 1 + data.len();
        let mut frame = vec![
            kwp2000::HEADER,
            kwp2000::TOOL_ADDR,
            kwp2000::ECU_ADDR,
            length as u8,
            response_sid,
        ];
        frame.extend_from_slice(data);
        let chksm = frame.iter().fold(0u8, |a, b| a.wrapping_add(*b));
        frame.push(chksm);
        frame
    }

    #[test]
    fn start_communication_sends_sid_81() {
        let mut mock = MockTransport::with_responses(vec![build_positive_response(
            sid::START_COMMUNICATION,
            &[0xEA, 0x8F],
        )]);
        start_communication(&mut mock, t()).unwrap();
    }

    #[test]
    fn request_security_seed_returns_4_bytes() {
        // Ответ: `67 01 12 34 56 78`
        let response =
            build_positive_response(sid::SECURITY_ACCESS, &[0x01, 0x12, 0x34, 0x56, 0x78]);
        let mut mock = MockTransport::with_responses(vec![response]);
        let seed = request_security_seed(&mut mock, t()).unwrap();
        assert_eq!(seed, [0x12, 0x34, 0x56, 0x78]);
    }

    #[test]
    fn unlock_full_flow_with_known_seed_key_pair() {
        // Используем bit-perfect-vector: seed = `12 34 56 78` → key = `E7 8F 1F 21`.
        let seed_response =
            build_positive_response(sid::SECURITY_ACCESS, &[0x01, 0x12, 0x34, 0x56, 0x78]);
        let key_ack = build_positive_response(sid::SECURITY_ACCESS, &[0x02]);
        let mut mock = MockTransport::with_responses(vec![seed_response, key_ack]);
        unlock(&mut mock, t()).unwrap();
    }

    #[test]
    fn submit_security_key_rejected_returns_invalid_key() {
        // ECU отвечает negative: `7F 27 35` (INVALID_KEY).
        // 80 F0 10 03 7F 27 35 <chksm = 0x5E>
        let nrc_response = vec![0x80, 0xF0, 0x10, 0x03, 0x7F, 0x27, 0x35, 0x5E];
        let mut mock = MockTransport::with_responses(vec![nrc_response]);
        let err = submit_security_key(&mut mock, [0xDE, 0xAD, 0xBE, 0xEF], t()).unwrap_err();
        match err {
            KernelError::NegativeResponse { request, nrc, .. } => {
                assert_eq!(request, 0x27);
                assert_eq!(nrc, kwp2000::nrc::INVALID_KEY);
            }
            other => panic!("expected NegativeResponse with NRC 0x35, got {other:?}"),
        }
    }

    #[test]
    fn start_programming_session_sends_85_02() {
        let response = build_positive_response(sid::START_DIAGNOSTIC_SESSION, &[0x85, 0x02]);
        let mut mock = MockTransport::with_responses(vec![response]);
        start_programming_session(&mut mock, t()).unwrap();
    }

    #[test]
    fn request_download_wire_format() {
        // Положительный 74-ответ. ECU может в payload вернуть max-block-size byte
        // (`84` в nisprog), мы игнорируем.
        let response = build_positive_response(sid::REQUEST_DOWNLOAD, &[0x84]);
        let mut mock = MockTransport::with_responses(vec![response]);
        request_download(&mut mock, 0xFF_FF30_00 & 0xFF_FFFF, 0x100, t()).unwrap();
    }

    #[test]
    fn transfer_data_chunks_splits_into_128_byte_pieces() {
        // 300 байт → 3 chunk-а: 128 + 128 + 44.
        let payload = vec![0xAA; 300];
        let responses = vec![
            build_positive_response(sid::TRANSFER_DATA, &[]),
            build_positive_response(sid::TRANSFER_DATA, &[]),
            build_positive_response(sid::TRANSFER_DATA, &[]),
        ];
        let mut mock = MockTransport::with_responses(responses);
        transfer_data_chunks(&mut mock, 0x3000, &payload, t()).unwrap();
    }

    #[test]
    fn upload_and_jump_full_orchestration() {
        // Минимальный kernel: 8 байт. Pad-н до 8 (уже кратен 4), encrypt применится.
        let fake_kernel = KernelBinary {
            mcu: crate::kernels::McuFamily::Sh7058,
            bytes: &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88],
            identifier: "fake-test-kernel",
        };
        // Ожидаемые ответы: payload (1 SID34 + 1 SID36) + trailer (1 SID34 + 1 SID36) + 1 SID31
        let responses = vec![
            build_positive_response(sid::REQUEST_DOWNLOAD, &[0x84]),
            build_positive_response(sid::TRANSFER_DATA, &[]),
            build_positive_response(sid::REQUEST_DOWNLOAD, &[0x84]),
            build_positive_response(sid::TRANSFER_DATA, &[]),
            build_positive_response(sid::START_ROUTINE_BY_ADDRESS, &[0x01, 0x01]),
        ];
        let mut mock = MockTransport::with_responses(responses);
        upload_and_jump(&mut mock, &fake_kernel, t()).unwrap();
    }
}
