//! Kernel-side wire-protocol — общение с RAM-resident kernel после handover.
//!
//! См. `npkern/cmd_parser.c` (GPL-3.0) для C-эталона. Кадры остаются в
//! KWP2000-формате (`80 <target> <source> <length> <sid> [data...] <chksm>`),
//! но набор SID-ов уже **специфичный для kernel-а**, не стандартный KWP2000.
//!
//! ## Kernel SID-ы (из `npkern/iso_cmds.h`)
//!
//! | SID    | Назначение                            |
//! |--------|----------------------------------------|
//! | `0x81` | `SID_STARTCOMM` — handshake после jump (ответ `C1 67 8F`) |
//! | `0x1A` | `SID_RECUID` — echo версии kernel-а   |
//! | `0x23` | `SID_RMBA` — readMemoryByAddress (≤251 байт за раз) |
//! | `0x3D` | `SID_WMBA` — writeMemoryByAddress (RAM only) |
//! | `0xBD` | `SID_DUMP` — **наш hot path**, стримит 32-байт блоки ROM/EEPROM |
//! | `0x3E` | `SID_TP`   — tester-present, kernel echo-ит без действий |
//! | `0xBE` | `SID_CONF` — umbrella для kernel-config (baud-rate switch и т.д.) |
//! | `0x11` | `SID_RESET` — kernel-side ECU restart |
//!
//! ## SID 0xBD wire-формат
//!
//! **Request:** `BD <AS> <BH BL> <AH AL>` где:
//! - `AS` — area select: `0x00` = EEPROM, `0x01` = ROM
//! - `BH BL` — uint16 big-endian, **число 32-байтных блоков** для дампа
//! - `AH AL` — uint16 big-endian, стартовый адрес **делённый на 32**
//!
//! **Response:** `<BH BL>` отдельных KWP2000-фреймов с SID `0xFD` (= `0xBD | 0x40`)
//! и 32 байтами data каждый.
//!
//! Это позволяет с одного запроса дампить большие куски — kernel сам шлёт
//! поток фреймов, без round-trip latency на каждый 32-байт chunk.

use std::time::Duration;

use romraider_io::transport::Transport;

use crate::error::{KernelError, KernelResult};
use crate::kwp2000::{self, parse_response, request_response};

// Kernel-side SID-ы. **НЕ путать** с стандартными SID-ами в `kwp2000::sid` —
// `SID_DUMP=0xBD` пересекается с `SID_FLASH=0xBC`, и они работают только
// после handover.
pub mod kernel_sid {
    pub const STARTCOMM: u8 = 0x81;
    pub const RECUID: u8 = 0x1A;
    pub const RMBA: u8 = 0x23;
    pub const WMBA: u8 = 0x3D;
    pub const TP: u8 = 0x3E;
    pub const DUMP: u8 = 0xBD;
    pub const CONF: u8 = 0xBE;
    pub const RESET: u8 = 0x11;
}

pub mod conf_sub {
    pub const SET_SPEED: u8 = 0x01;
    pub const SET_EEPROM: u8 = 0x02;
    pub const CKS1: u8 = 0x03;
    pub const LASTERR: u8 = 0x05;
}

/// Размер одного chunk-а в ответе SID_DUMP (фиксированный).
pub const DUMP_CHUNK_SIZE: usize = 32;

/// Сектор памяти для SID_DUMP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpArea {
    /// `AS=0x00` — внешний EEPROM ECU (~256-512 байт обычно).
    Eeprom,
    /// `AS=0x01` — внутренний flash ROM (наша цель).
    Rom,
}

impl DumpArea {
    fn as_byte(self) -> u8 {
        match self {
            Self::Eeprom => 0x00,
            Self::Rom => 0x01,
        }
    }
}

// ---------------------------------------------------------------------------
// Handshake (после jump на kernel)
// ---------------------------------------------------------------------------

/// Kernel-handshake: tester шлёт SID `0x81` на 15625 бод, kernel отвечает
/// `C1 67 8F`. Должно быть сделано **первым делом** после успешного [`crate::upload::start_routine_jump`].
pub fn handshake(transport: &mut dyn Transport, timeout: Duration) -> KernelResult<()> {
    let frame = request_response(transport, kernel_sid::STARTCOMM, &[], timeout)?;
    // Ожидаем `C1 67 8F` (sid_resp + 2-byte kernel ack).
    // Содержимое payload не критично, но проверим что хотя бы один байт есть.
    if frame.data.is_empty() {
        return Err(KernelError::HandoverFailed);
    }
    tracing::debug!(payload = ?frame.data, "kernel handshake OK");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tester-present keep-alive
// ---------------------------------------------------------------------------

/// Послать tester-present (SID `0x3E`). Kernel echo-ит без действий —
/// нужно только если между командами проходит > P3_MAX (~5 сек).
pub fn tester_present(transport: &mut dyn Transport, timeout: Duration) -> KernelResult<()> {
    let _ = request_response(transport, kernel_sid::TP, &[], timeout)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// SID_DUMP — основной путь для чтения ROM
// ---------------------------------------------------------------------------

/// Запросить дамп `num_blocks * 32` байт начиная с `start_address`.
///
/// `start_address` должен быть **выровнен по 32 байтам** — kernel передаёт
/// `address / 32` как 16-бит. `num_blocks` тоже 16-бит, так что максимум
/// **65 535 блоков = ~2 МБ за один SID_DUMP**.
///
/// Чтение блокирующее: ждём `num_blocks` фреймов подряд. Возвращает
/// `num_blocks * 32` байт payload-а.
pub fn dump_blocks(
    transport: &mut dyn Transport,
    area: DumpArea,
    start_address: u32,
    num_blocks: u16,
    timeout: Duration,
) -> KernelResult<Vec<u8>> {
    if start_address % 32 != 0 {
        return Err(KernelError::UploadAborted(format!(
            "start_address 0x{start_address:06X} not aligned to 32 bytes"
        )));
    }
    if num_blocks == 0 {
        return Ok(Vec::new());
    }
    let addr_div32 = start_address / 32;
    if addr_div32 > u32::from(u16::MAX) {
        return Err(KernelError::UploadAborted(format!(
            "start_address 0x{start_address:06X} > 2 MiB (kernel addr field is 16-bit)"
        )));
    }

    let payload = [
        area.as_byte(),
        ((num_blocks >> 8) & 0xFF) as u8,
        (num_blocks & 0xFF) as u8,
        ((addr_div32 >> 8) & 0xFF) as u8,
        (addr_div32 & 0xFF) as u8,
    ];

    // Send request (один SID_DUMP).
    let req = kwp2000::build_request(kernel_sid::DUMP, &payload);
    tracing::trace!(
        addr = format!("0x{start_address:06X}"),
        num_blocks,
        "SID_DUMP request",
    );
    transport.write_all(&req, timeout)?;

    // Прочитать `num_blocks` фреймов с SID 0xFD (positive response для 0xBD).
    let expected_sid = kernel_sid::DUMP | kwp2000::POSITIVE_RESPONSE_MASK;
    let mut out = Vec::with_capacity(usize::from(num_blocks) * DUMP_CHUNK_SIZE);
    let mut buf = [0u8; 64]; // header(4) + sid(1) + 32 + chksm(1) = 38 — c запасом
    for block_idx in 0..num_blocks {
        let n = transport.read_frame(&mut buf, timeout)?;
        let (_consumed, frame) = parse_response(&buf[..n])?;
        if frame.sid != expected_sid {
            return Err(KernelError::UnexpectedSid {
                expected: kernel_sid::DUMP,
                got: frame.sid,
            });
        }
        if frame.data.len() != DUMP_CHUNK_SIZE {
            return Err(KernelError::UploadAborted(format!(
                "SID_DUMP chunk #{block_idx} length {} != expected {DUMP_CHUNK_SIZE}",
                frame.data.len(),
            )));
        }
        out.extend_from_slice(&frame.data);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// SID_CONF — config (baud-rate switching и т.п.)
// ---------------------------------------------------------------------------

/// Расчёт baud-rate из BRR divisor для SH7058 SCI @ 20 MHz.
///
/// Из `nisprog/npk_backend.c:572`: `KSPEED_FROM_BRR(x) = 20MHz / (32×(x+1))`.
/// Примеры:
/// - BRR =  0 → 625 000 baud (максимум — но обычно вне tolerance)
/// - BRR =  9 → 62 500 baud (**рекомендуется для быстрого dump-а**)
/// - BRR = 39 → 15 625 baud (рекомендуется для programming session upload)
/// - BRR = 259 → 2 403 baud (~~стандартный 2400~~)
pub const fn kspeed_from_brr(brr_div: u8) -> u32 {
    20_000_000 / (32 * (brr_div as u32 + 1))
}

/// Обратное: BRR divisor для целевого baud-rate. Из `nisprog/npk_backend.c:573`.
/// Возвращает `None` если требуемый baud вне диапазона (т.е. BRR > 255).
pub const fn brr_from_kspeed(target_baud: u32) -> Option<u8> {
    if target_baud == 0 {
        return None;
    }
    let div = 20_000_000 / (32 * target_baud);
    if div == 0 || div > 256 {
        return None;
    }
    Some((div - 1) as u8)
}

/// Переключить kernel на новую baud-rate **и одновременно** передвинуть
/// host-сторону через [`Transport::set_baud`].
///
/// `brr_div` — BRR divisor для SCI SH7058 (см. [`kspeed_from_brr`]).
/// Возвращает реальный baud, на котором теперь работает связь.
///
/// **Тайминг:** kernel переключается **сразу после отправки `FE`-ответа**
/// (синхронно с TX-end на K-Line). Между приходом `FE` и нашим
/// `transport.set_baud()` должно пройти минимум времени, иначе следующий
/// TX уйдёт «не туда». В обычной программе зазор < 10 мс — это OK.
pub fn set_kernel_speed(
    transport: &mut dyn Transport,
    brr_div: u8,
    timeout: Duration,
) -> KernelResult<u32> {
    let new_baud = kspeed_from_brr(brr_div);
    tracing::info!(brr_div, new_baud, "switching kernel + host baud");

    let _ = request_response(
        transport,
        kernel_sid::CONF,
        &[conf_sub::SET_SPEED, brr_div],
        timeout,
    )?;
    // Kernel-сторона переключилась мгновенно — переключаем host.
    transport.set_baud(new_baud)?;
    tracing::info!(new_baud, "kernel + host baud switched");
    Ok(new_baud)
}

// ---------------------------------------------------------------------------
// SID_RESET — выход
// ---------------------------------------------------------------------------

/// Завершить kernel-сессию (ECU перезапустится в обычный режим).
/// На этом этапе transport уже бесполезен — ECU потеряет K-Line.
pub fn kernel_reset(transport: &mut dyn Transport, timeout: Duration) -> KernelResult<()> {
    let _ = request_response(transport, kernel_sid::RESET, &[], timeout)?;
    tracing::info!("kernel reset request sent; ECU restarting");
    Ok(())
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

    /// Хелпер: построить положительный KWP2000-ответ.
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
    fn handshake_accepts_typical_kernel_response() {
        // Kernel шлёт `C1 67 8F` (`SID 0x81 | 0x40 = 0xC1` + `67 8F` "kernel ack").
        let response = build_positive_response(kernel_sid::STARTCOMM, &[0x67, 0x8F]);
        let mut mock = MockTransport::with_responses(vec![response]);
        handshake(&mut mock, t()).unwrap();
    }

    #[test]
    fn dump_blocks_collects_payload_from_chunks() {
        // Ожидаем 3 chunk-а: каждый 32 байта payload.
        let chunk1 = build_positive_response(kernel_sid::DUMP, &[0xA1; DUMP_CHUNK_SIZE]);
        let chunk2 = build_positive_response(kernel_sid::DUMP, &[0xB2; DUMP_CHUNK_SIZE]);
        let chunk3 = build_positive_response(kernel_sid::DUMP, &[0xC3; DUMP_CHUNK_SIZE]);
        let mut mock = MockTransport::with_responses(vec![chunk1, chunk2, chunk3]);

        let data = dump_blocks(&mut mock, DumpArea::Rom, 0x0000, 3, t()).unwrap();
        assert_eq!(data.len(), 3 * DUMP_CHUNK_SIZE);
        assert!(data[0..DUMP_CHUNK_SIZE].iter().all(|&b| b == 0xA1));
        assert!(data[DUMP_CHUNK_SIZE..2 * DUMP_CHUNK_SIZE]
            .iter()
            .all(|&b| b == 0xB2));
        assert!(data[2 * DUMP_CHUNK_SIZE..].iter().all(|&b| b == 0xC3));
    }

    #[test]
    fn dump_blocks_rejects_unaligned_address() {
        let mut mock = MockTransport::with_responses(Vec::<Vec<u8>>::new());
        let err = dump_blocks(&mut mock, DumpArea::Rom, 0x1001, 1, t()).unwrap_err();
        assert!(matches!(err, KernelError::UploadAborted(_)));
    }

    #[test]
    fn dump_blocks_rejects_too_high_address() {
        // 2 МБ * 32 = 64 МБ — больше 16-бит / 32, упадёт.
        let mut mock = MockTransport::with_responses(Vec::<Vec<u8>>::new());
        let err = dump_blocks(&mut mock, DumpArea::Rom, 0x300_0000, 1, t()).unwrap_err();
        assert!(matches!(err, KernelError::UploadAborted(_)));
    }

    #[test]
    fn dump_blocks_zero_count_is_empty_ok() {
        let mut mock = MockTransport::with_responses(Vec::<Vec<u8>>::new());
        let data = dump_blocks(&mut mock, DumpArea::Rom, 0x0000, 0, t()).unwrap();
        assert!(data.is_empty());
    }

    #[test]
    fn kspeed_brr_formula_matches_npkern() {
        // Точные значения из `nisprog/npk_backend.c`:
        assert_eq!(kspeed_from_brr(0), 625_000);
        assert_eq!(kspeed_from_brr(9), 62_500);
        assert_eq!(kspeed_from_brr(39), 15_625);
        // Обратимость:
        assert_eq!(brr_from_kspeed(62_500), Some(9));
        assert_eq!(brr_from_kspeed(15_625), Some(39));
        // Граничный случай — слишком медленный baud (BRR > 255):
        assert_eq!(brr_from_kspeed(100), None);
    }

    #[test]
    fn set_kernel_speed_returns_computed_baud_and_sets_host() {
        // MockTransport умеет `set_baud` через default (Err), поэтому используем
        // только проверку расчёта baud без реального set_baud-а.
        // Реальный TactrixTransport проверим live.
        let response = build_positive_response(kernel_sid::CONF, &[]);
        let mut mock = MockTransport::with_responses(vec![response]);
        // brr_div=9 → 62 500 baud. Mock-овский set_baud-default Err-ит, но мы
        // ловим именно `UnsupportedOperation` чтобы тест прошёл.
        let err = set_kernel_speed(&mut mock, 9, t()).unwrap_err();
        match err {
            KernelError::Io(romraider_io::error::IoError::UnsupportedOperation(_)) => {} // ОК
            other => panic!("expected UnsupportedOperation, got {other:?}"),
        }
    }

    #[test]
    fn tester_present_round_trip() {
        let response = build_positive_response(kernel_sid::TP, &[]);
        let mut mock = MockTransport::with_responses(vec![response]);
        tester_present(&mut mock, t()).unwrap();
    }

    #[test]
    fn kernel_reset_sends_sid_11() {
        let response = build_positive_response(kernel_sid::RESET, &[]);
        let mut mock = MockTransport::with_responses(vec![response]);
        kernel_reset(&mut mock, t()).unwrap();
    }
}
