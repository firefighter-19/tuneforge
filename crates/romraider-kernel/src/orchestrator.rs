//! High-level orchestrator для **полного** Mac-native ROM dump через
//! UDS/ISO15765 на Subaru SH7058 ECU. Это та же sequence что в
//! `dump-rom-can` CLI-команде, но извлечённая в reusable библиотечный
//! API, чтобы её мог вызывать **и CLI, и GUI** (Slice 26).
//!
//! Progress emitted через [`DumpProgress`]-callback после каждой
//! milestone (по фазе) и после каждого read-chunk-а. UI может рисовать
//! progress-bar, CLI — печатать в stderr.
//!
//! Flow:
//! 1. **Phase A** — OBD-II Mode 01/09 PIDs (wake-up + VIN/CVN)
//! 2. **Phase B** — `10 03` ExtendedSession → `27 01/02` SecurityAccess
//!    (через [`subaru_genkey_can`]) → `10 02` ProgrammingSession
//! 3. **Phase C** — 64× SID `0xB6` TransferData (encrypted V1.07 kernel,
//!    8192 байт) → SID `0x37` TransferExit → SID `0x31` StartRoutine
//!    (RAM jump в `0xFFFF3000`)
//! 4. **Phase D** — kernel handshake (`0x01` → ASCII banner)
//! 5. **Phase E** — серия `0x03 <addr32 BE> <len16 BE>` read-memory @
//!    2 КБ chunks
//!
//! Caller отвечает за: **открытие Tactrix-CAN транспорта** (с
//! flow-control filter) и **сохранение байтов в файл** после успеха.

use std::time::{Duration, Instant};

use romraider_core::bytes;
use romraider_io::transport::Transport;

use crate::can_upload::{self, CAN_KERNEL_V107_ENCRYPTED};
use crate::can_wire::{self, read_memory};
use crate::error::KernelError;
use crate::seed_key::subaru_genkey_can;

/// CAN OBD-II 11-bit ECU request ID (тот же что у `obd2`/`uds` модулей,
/// дублируется тут чтобы не было cross-crate-зависимостей).
pub const OBD_REQUEST_ID:  u32 = 0x7E0;
/// CAN OBD-II 11-bit ECU response ID.
pub const OBD_RESPONSE_ID: u32 = 0x7E8;

/// Progress events emitted by [`dump_rom_via_can`].
///
/// Caller подписывается через `progress: FnMut(DumpProgress)`. Каждая
/// фаза эмитит свой milestone event. Phase E дополнительно эмитит
/// `ReadProgress` после **каждого chunk-а** (для progress-bar в UI).
#[derive(Debug, Clone)]
pub enum DumpProgress {
    /// Phase A done — Mode 01 PID 00 returned (supported-PIDs bitmap).
    PhaseAObdiiWake     { mode01_response: Vec<u8> },
    /// Phase A — VIN (Mode 09 PID 02) raw response.
    PhaseAVin           { vin_raw: Vec<u8> },
    /// Phase A — CVN (Mode 09 PID 06) raw response.
    PhaseACvn           { cvn_raw: Vec<u8> },
    /// Phase B — `10 03` ExtendedDiagnosticSession accepted.
    PhaseBExtSession,
    /// Phase B — security access seed received (4 bytes).
    PhaseBSeed          { seed: [u8; 4] },
    /// Phase B — key computed via [`subaru_genkey_can`].
    PhaseBKey           { key:  [u8; 4] },
    /// Phase B — `27 02` SecurityAccess granted.
    PhaseBGranted,
    /// Phase B — `10 02` ProgrammingSession active.
    PhaseBProgSession,
    /// Phase C — kernel upload starting (`total_bytes` бесед к ECU).
    PhaseCUploadStart   { total_bytes: usize },
    /// Phase C — kernel uploaded + StartRoutine executed (jump в RAM).
    PhaseCUploadDone    { elapsed_secs: f64 },
    /// Phase D — kernel handshake response, ASCII banner.
    PhaseDBanner        { banner: String },
    /// Phase E — read loop start.
    PhaseEReadStart     { start_addr: u32, total: usize, chunk_size: u16 },
    /// Phase E — после каждого chunk-а.
    PhaseEReadProgress  { done: usize, total: usize, rate_bps: f64 },
    /// Phase E — read loop done.
    PhaseEDone          { bytes_dumped: usize, elapsed_secs: f64 },
}

/// Послать UDS-команду по CAN и получить response (без 4-байтового
/// CAN_ID prefix-а, который Tactrix добавляет в TX и оставляет в RX).
///
/// На фронте теста ([`dump_rom_via_can`]) этот же helper использует
/// CLI и GUI — extracted из `cli/main.rs`.
pub fn uds_request(
    tr:      &mut dyn Transport,
    uds_tx:  &[u8],
    timeout: Duration,
) -> Result<Vec<u8>, KernelError> {
    let mut tx = Vec::with_capacity(4 + uds_tx.len());
    tx.extend_from_slice(&OBD_REQUEST_ID.to_be_bytes());
    tx.extend_from_slice(uds_tx);
    tr.write_all(&tx, timeout)?;

    let mut buf = [0u8; 512];
    let n = tr.read_frame(&mut buf, timeout)?;
    if n < 4 {
        return Err(KernelError::UploadAborted(format!(
            "CAN RX too short: {n} bytes"
        )));
    }
    Ok(buf[4..n].to_vec())
}

/// Полный orchestrator: OBD-II → SecurityAccess → Kernel upload → ROM dump.
///
/// `tr` — открытый Tactrix-CAN transport (caller отвечает за
/// `open_tactrix_can()` + flow-control filter).
/// `start_addr` — 32-bit RAM-адрес начала чтения (типично `0x000000`).
/// `length` — сколько байт читать. `0` ⇒ default `1 МБ` для SH7058.
/// `chunk_size` — размер одного `0x03 read_memory` запроса (max 2048).
/// `timeout` — per-frame timeout.
/// `progress` — emitted после каждой milestone и каждого read-chunk-а.
///
/// Возвращает полный dump (`Vec<u8>` длиной `length`).
pub fn dump_rom_via_can<F>(
    tr:           &mut dyn Transport,
    start_addr:   u32,
    length:       usize,
    chunk_size:   u16,
    timeout:      Duration,
    mut progress: F,
) -> Result<Vec<u8>, KernelError>
where
    F: FnMut(DumpProgress),
{
    // ── Phase A — OBD-II wake-up ─────────────────────────────────────
    let r = uds_request(tr, &[0x01, 0x00], timeout)?;
    progress(DumpProgress::PhaseAObdiiWake { mode01_response: r });
    let r = uds_request(tr, &[0x09, 0x02], timeout)?;
    progress(DumpProgress::PhaseAVin { vin_raw: r });
    let r = uds_request(tr, &[0x09, 0x06], timeout)?;
    progress(DumpProgress::PhaseACvn { cvn_raw: r });

    // ── Phase B — ExtSession + SecurityAccess + ProgSession ─────────
    let resp = uds_request(tr, &[0x10, 0x03], timeout)?;
    if resp.first() != Some(&0x50) {
        return Err(KernelError::UploadAborted(format!(
            "SID 10 03 (ExtendedSession) failed: {}",
            bytes::hex_dump(&resp)
        )));
    }
    progress(DumpProgress::PhaseBExtSession);

    let resp = uds_request(tr, &[0x27, 0x01], timeout)?;
    if resp.len() < 6 || resp[0] != 0x67 || resp[1] != 0x01 {
        return Err(KernelError::UploadAborted(format!(
            "SID 27 01 (Seed) failed: {}",
            bytes::hex_dump(&resp)
        )));
    }
    let seed = [resp[2], resp[3], resp[4], resp[5]];
    progress(DumpProgress::PhaseBSeed { seed });
    let key = subaru_genkey_can(seed);
    progress(DumpProgress::PhaseBKey { key });

    let mut send = Vec::with_capacity(6);
    send.push(0x27);
    send.push(0x02);
    send.extend_from_slice(&key);
    let resp = uds_request(tr, &send, timeout)?;
    if resp.first() != Some(&0x67) || resp.get(1) != Some(&0x02) {
        return Err(KernelError::UploadAborted(format!(
            "SID 27 02 (Key) rejected: {}",
            bytes::hex_dump(&resp)
        )));
    }
    progress(DumpProgress::PhaseBGranted);

    let resp = uds_request(tr, &[0x10, 0x02], timeout)?;
    if resp.first() != Some(&0x50) || resp.get(1) != Some(&0x02) {
        return Err(KernelError::UploadAborted(format!(
            "SID 10 02 (ProgSession) failed: {}",
            bytes::hex_dump(&resp)
        )));
    }
    progress(DumpProgress::PhaseBProgSession);

    // ── Phase C — Kernel upload ──────────────────────────────────────
    progress(DumpProgress::PhaseCUploadStart {
        total_bytes: CAN_KERNEL_V107_ENCRYPTED.len(),
    });
    let started_upload = Instant::now();
    can_upload::upload_and_jump(tr, CAN_KERNEL_V107_ENCRYPTED, timeout)?;
    progress(DumpProgress::PhaseCUploadDone {
        elapsed_secs: started_upload.elapsed().as_secs_f64(),
    });

    // ── Phase D — Kernel handshake ───────────────────────────────────
    let banner = can_wire::handshake(tr, timeout)?;
    progress(DumpProgress::PhaseDBanner { banner });

    // ── Phase E — ROM dump loop ──────────────────────────────────────
    let length_val = if length == 0 { 1024 * 1024 } else { length };
    progress(DumpProgress::PhaseEReadStart {
        start_addr,
        total: length_val,
        chunk_size,
    });
    let started_read = Instant::now();

    let mut out = Vec::with_capacity(length_val);
    let mut addr = start_addr;
    while out.len() < length_val {
        let remaining = length_val - out.len();
        let this_chunk = remaining.min(chunk_size as usize) as u16;
        let data = read_memory(tr, addr, this_chunk, timeout)?;
        out.extend_from_slice(&data);
        addr += u32::from(this_chunk);
        let rate = out.len() as f64 / started_read.elapsed().as_secs_f64().max(1e-6);
        progress(DumpProgress::PhaseEReadProgress {
            done: out.len(),
            total: length_val,
            rate_bps: rate,
        });
    }

    progress(DumpProgress::PhaseEDone {
        bytes_dumped: out.len(),
        elapsed_secs: started_read.elapsed().as_secs_f64(),
    });

    Ok(out)
}

/// Compact view of OBD-II identity (VIN + CVN) used by [`peek_ecu_info`].
#[derive(Debug, Clone, Default)]
pub struct EcuInfo {
    pub mode01_supported_bitmap: u32,
    pub vin: Option<String>,
    pub cvn: Option<[u8; 4]>,
}

/// Read-only «опознавательный» опрос ECU: Mode 01 PID 00 (supported PIDs
/// bitmap), Mode 09 PID 02 (VIN), Mode 09 PID 06 (CVN). Используется для
/// GUI «View ECU Info» панели — никакой SecurityAccess не требуется.
pub fn peek_ecu_info(
    tr:      &mut dyn Transport,
    timeout: Duration,
) -> Result<EcuInfo, KernelError> {
    let mut info = EcuInfo::default();

    // Mode 01 PID 00 → bitmap
    if let Ok(resp) = uds_request(tr, &[0x01, 0x00], timeout) {
        // Tactrix strip уже снял CAN-ID, мы получаем `41 00 <4B bitmap>`.
        if resp.len() >= 6 && resp[0] == 0x41 && resp[1] == 0x00 {
            info.mode01_supported_bitmap =
                u32::from_be_bytes([resp[2], resp[3], resp[4], resp[5]]);
        }
    }

    // Mode 09 PID 02 → VIN
    if let Ok(resp) = uds_request(tr, &[0x09, 0x02], timeout) {
        // Reply: `49 02 <num_chunks=01> <17-byte VIN, ASCII>`.
        if resp.len() >= 20 && resp[0] == 0x49 && resp[1] == 0x02 {
            let vin_bytes = &resp[3..20];
            info.vin = Some(
                vin_bytes
                    .iter()
                    .map(|&b| {
                        if (0x20..0x7F).contains(&b) {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect(),
            );
        }
    }

    // Mode 09 PID 06 → CVN
    if let Ok(resp) = uds_request(tr, &[0x09, 0x06], timeout) {
        // Reply: `49 06 <num=01> <4-byte CVN>`.
        if resp.len() >= 7 && resp[0] == 0x49 && resp[1] == 0x06 {
            info.cvn = Some([resp[3], resp[4], resp[5], resp[6]]);
        }
    }

    Ok(info)
}
