//! CAN-side UDS kernel upload для Subaru SH7058 ECU (2007+ Forester, Impreza
//! и т.п.). Используется поверх ISO15765 transport (Tactrix `ato6`).
//!
//! Отличия от K-Line варианта ([`crate::upload`]):
//! - **TransferData = SID `0xB6`** (на K-Line — 0x36). Ack = `0xF6` (на K-Line — 0x76).
//! - SID `0x10 0x02` (programmingSession) **не нужен** — ExtendedDiagnosticSession
//!   `10 03` + SecurityAccess дают достаточные привилегии.
//! - Address-Length Format Identifier (ALFID) = `0x33` (3-byte addr, 3-byte size).
//! - Load address `0xFF3000` (24-bit, ECU интерпретирует как `0xFFFF3000` RAM).
//! - Encrypted payload — тот же 4-round Feistel что K-Line (`subaru_encrypt_buffer`).
//!
//! Полный flow (после SecurityAccess granted):
//! 1. [`request_download`] — `34 04 33 FF 30 00 00 20 00`
//! 2. 64× [`transfer_data`] — `B6 <addr_3B BE> <128 encrypted bytes>`
//! 3. [`start_routine`] — `31 01 01` → ECU jumps to kernel at `0xFFFF3000`
//!
//! После handover общаемся с kernel через [`crate::can_wire`] (1-byte command set).

use std::time::Duration;

use romraider_io::transport::Transport;

use crate::error::KernelError;

/// CAN OBD-II standard 11-bit ECU request CAN ID.
pub const CAN_REQUEST_ID: u32 = 0x7E0;
/// CAN OBD-II standard 11-bit ECU response CAN ID.
pub const CAN_RESPONSE_ID: u32 = 0x7E8;

/// Subaru SH7058 RAM load address (24-bit truncated; ECU зашит `0xFFFF` upper byte).
pub const KERNEL_LOAD_ADDR_24: u32 = 0xFF_3000;
/// Полный 32-bit kernel RAM address (используем для документации/dump-cross-check).
pub const KERNEL_LOAD_ADDR_FULL: u32 = 0xFFFF_3000;
/// Total encrypted payload size (включая padding до 4-byte alignment).
pub const KERNEL_PAYLOAD_SIZE: u32 = 0x0000_2000; // 8192 байт

/// Chunk size для SID 0xB6 TransferData.
pub const TRANSFER_CHUNK_SIZE: usize = 128;

/// SID константы для CAN-side Subaru UDS.
pub mod can_sid {
    pub const SESSION_CONTROL: u8 = 0x10;
    pub const SECURITY_ACCESS: u8 = 0x27;
    pub const REQUEST_DOWNLOAD: u8 = 0x34;
    pub const TRANSFER_DATA_CAN: u8 = 0xB6;
    pub const REQUEST_TRANSFER_EXIT: u8 = 0x37;
    pub const START_ROUTINE_BY_LOCAL_ID: u8 = 0x31;

    pub const RESP_REQUEST_DOWNLOAD: u8 = 0x74;
    pub const RESP_TRANSFER_DATA_CAN: u8 = 0xF6;
    pub const RESP_TRANSFER_EXIT: u8 = 0x77;
    pub const RESP_START_ROUTINE: u8 = 0x71;
}

/// Embedded encrypted CAN kernel binary (extracted from EcuFlash session).
/// 8192 bytes, OpenECU CAN Kernel V1.07 (verified от capture banner).
#[cfg(feature = "sh7058")]
pub const CAN_KERNEL_V107_ENCRYPTED: &[u8] =
    include_bytes!("../kernels/openecu_can_v1.07_encrypted.bin");

/// Хелпер: послать UDS-command по CAN и получить UDS-response (без CAN_ID prefix).
fn uds_request_can(
    tr: &mut dyn Transport,
    uds_tx: &[u8],
    timeout: Duration,
) -> Result<Vec<u8>, KernelError> {
    let mut tx = Vec::with_capacity(4 + uds_tx.len());
    tx.extend_from_slice(&CAN_REQUEST_ID.to_be_bytes());
    tx.extend_from_slice(uds_tx);
    tr.write_all(&tx, timeout)?;

    let mut buf = [0u8; 4096];
    let n = tr.read_frame(&mut buf, timeout)?;
    if n < 4 {
        return Err(KernelError::UploadAborted(format!(
            "CAN RX too short: {n} bytes"
        )));
    }
    // Validate response CAN ID + strip prefix.
    let resp_id = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if resp_id != CAN_RESPONSE_ID {
        return Err(KernelError::UploadAborted(format!(
            "wrong CAN response ID: 0x{resp_id:08X} (expected 0x{CAN_RESPONSE_ID:08X})"
        )));
    }
    let uds = buf[4..n].to_vec();
    // Check for NRC `7F <req_sid> <nrc>`.
    if uds.len() >= 3 && uds[0] == 0x7F {
        return Err(KernelError::NegativeResponse {
            request: uds[1],
            nrc: uds[2],
            nrc_meaning: crate::kwp2000::nrc_meaning(uds[2]),
        });
    }
    Ok(uds)
}

/// SID 0x34 `requestDownload` для CAN-варианта.
///
/// Wire: `34 04 33 <addr_3B BE> <size_3B BE>`
/// - DFI = `0x04` (uncompressed + manufacturer-encrypted)
/// - ALFID = `0x33` (3-byte addr, 3-byte size)
///
/// Положительный ответ: `74 <max_block_size_format>` (мы игнорируем — chunk_size fixed).
pub fn request_download(
    tr: &mut dyn Transport,
    addr24: u32,
    size24: u32,
    timeout: Duration,
) -> Result<(), KernelError> {
    let payload = [
        can_sid::REQUEST_DOWNLOAD,
        0x04, // DFI
        0x33, // ALFID
        ((addr24 >> 16) & 0xFF) as u8,
        ((addr24 >> 8) & 0xFF) as u8,
        (addr24 & 0xFF) as u8,
        ((size24 >> 16) & 0xFF) as u8,
        ((size24 >> 8) & 0xFF) as u8,
        (size24 & 0xFF) as u8,
    ];
    let resp = uds_request_can(tr, &payload, timeout)?;
    if resp.first() != Some(&can_sid::RESP_REQUEST_DOWNLOAD) {
        return Err(KernelError::UploadAborted(format!(
            "SID 0x34 RequestDownload unexpected response: {resp:02X?}"
        )));
    }
    Ok(())
}

/// SID 0xB6 `transferData` для CAN-варианта.
///
/// Wire: `B6 <addr_3B BE> <data...>` — обычно 128 байт data на chunk.
/// Положительный ответ: `F6`.
pub fn transfer_data(
    tr: &mut dyn Transport,
    addr24: u32,
    chunk: &[u8],
    timeout: Duration,
) -> Result<(), KernelError> {
    let mut payload = Vec::with_capacity(4 + chunk.len());
    payload.push(can_sid::TRANSFER_DATA_CAN);
    payload.push(((addr24 >> 16) & 0xFF) as u8);
    payload.push(((addr24 >> 8) & 0xFF) as u8);
    payload.push((addr24 & 0xFF) as u8);
    payload.extend_from_slice(chunk);

    let resp = uds_request_can(tr, &payload, timeout)?;
    if resp.first() != Some(&can_sid::RESP_TRANSFER_DATA_CAN) {
        return Err(KernelError::UploadAborted(format!(
            "SID 0xB6 TransferData unexpected response: {resp:02X?}"
        )));
    }
    Ok(())
}

/// SID 0x37 `requestTransferExit` — финализирует upload-сессию.
///
/// Wire: `37` (1 byte). Положительный ответ: `77`.
/// Должен быть отправлен **между последним SID 0xB6 и SID 0x31**.
pub fn request_transfer_exit(tr: &mut dyn Transport, timeout: Duration) -> Result<(), KernelError> {
    let resp = uds_request_can(tr, &[can_sid::REQUEST_TRANSFER_EXIT], timeout)?;
    if resp.first() != Some(&can_sid::RESP_TRANSFER_EXIT) {
        return Err(KernelError::UploadAborted(format!(
            "SID 0x37 RequestTransferExit unexpected: {resp:02X?}"
        )));
    }
    Ok(())
}

/// SID 0x31 `startRoutineByLocalIdentifier` — jump to RAM-resident kernel.
///
/// Wire (Subaru CAN, **отличается от K-Line `01 01`!**): `31 01 02 02 02`.
/// Положительный ответ: `71 01 02 02`.
pub fn start_routine(tr: &mut dyn Transport, timeout: Duration) -> Result<(), KernelError> {
    let resp = uds_request_can(
        tr,
        &[can_sid::START_ROUTINE_BY_LOCAL_ID, 0x01, 0x02, 0x02, 0x02],
        timeout,
    )?;
    if resp.first() != Some(&can_sid::RESP_START_ROUTINE) {
        return Err(KernelError::UploadAborted(format!(
            "SID 0x31 StartRoutine unexpected response: {resp:02X?}"
        )));
    }
    Ok(())
}

/// Полный orchestrator: RequestDownload + 64× TransferData + StartRoutine.
///
/// Принимает **уже зашифрованный** kernel binary (8192 байт = 64 × 128).
/// Caller отвечает за encrypt; для replay-attack использовать
/// [`CAN_KERNEL_V107_ENCRYPTED`] как-есть из capture.
pub fn upload_and_jump(
    tr: &mut dyn Transport,
    encrypted_payload: &[u8],
    timeout: Duration,
) -> Result<(), KernelError> {
    if encrypted_payload.len() != KERNEL_PAYLOAD_SIZE as usize {
        return Err(KernelError::UploadAborted(format!(
            "encrypted payload size {} ≠ expected {}",
            encrypted_payload.len(),
            KERNEL_PAYLOAD_SIZE,
        )));
    }

    tracing::info!(
        addr = format!("0x{:06X}", KERNEL_LOAD_ADDR_24),
        size = encrypted_payload.len(),
        "SID 0x34 RequestDownload",
    );
    request_download(tr, KERNEL_LOAD_ADDR_24, KERNEL_PAYLOAD_SIZE, timeout)?;

    let n_chunks = encrypted_payload.len() / TRANSFER_CHUNK_SIZE;
    tracing::info!(n_chunks, "starting SID 0xB6 TransferData loop");
    for (i, chunk) in encrypted_payload
        .chunks_exact(TRANSFER_CHUNK_SIZE)
        .enumerate()
    {
        let chunk_addr = KERNEL_LOAD_ADDR_24 + (i * TRANSFER_CHUNK_SIZE) as u32;
        transfer_data(tr, chunk_addr, chunk, timeout)?;
        if i % 8 == 0 || i + 1 == n_chunks {
            tracing::debug!(
                chunk = i + 1,
                total = n_chunks,
                addr = format!("0x{:06X}", chunk_addr),
                "TransferData ack",
            );
        }
    }

    tracing::info!("SID 0x37 RequestTransferExit (finalize upload)");
    request_transfer_exit(tr, timeout)?;

    tracing::info!("SID 0x31 StartRoutine → jumping to kernel");
    start_routine(tr, timeout)?;
    Ok(())
}
