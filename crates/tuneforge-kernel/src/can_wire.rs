//! Kernel-side wire-protocol для OpenECU CAN Kernel V1.07.
//!
//! Это **НЕ** UDS — после SID 0x31 StartRoutine, kernel в RAM запускает свой
//! собственный 1-byte command protocol. Communication продолжается по тем же
//! CAN-IDs (`0x7E0` request, `0x7E8` response) и тому же Tactrix ISO15765
//! transport — Tactrix не знает что собеседник сменился.
//!
//! ## Banner verification
//!
//! Первое что делаем после kernel jump — TX `01`, expect response начинающийся
//! с `0x81` + ASCII banner: `"OpenECU Subaru SH7058 OCP CAN Kernel V1.07"`.
//! Если banner совпадает — kernel живой и готов принимать команды.
//!
//! ## Команды kernel
//!
//! | TX | Response | Назначение |
//! |----|----------|------------|
//! | `01` | `81 <ASCII banner>` | identification |
//! | `02 <addr_4B BE> <len_3B BE>` | `82 <data...>` | ???-extended read (variants) |
//! | `03 <addr_4B BE> <len_2B BE>` | `83 <data...>` | **read memory** — основная команда дампа |
//! | `05` | `85 <4B>` | возможно "memory config / chunk size" |
//! | `06` | `86 <4B>` | возможно "ROM region info" |
//!
//! ## Read Memory (наш hot path)
//!
//! Wire: `03 <AH AM AL A0> <LH LL>` (7 bytes)
//! - `<addr 4B BE>` — start address (full 32-bit ROM addr, e.g. `0x00000000`)
//! - `<len 2B BE>` — chunk size (max ~2048 байт на запрос; за один проход 1 МБ
//!   дампится за ~500 запросов).
//!
//! Response: `83 <data 0..len>` — bytes of ROM. Tactrix re-собирает multi-frame
//! ISO15765 в один RxEnd, мы получаем чистый buffer.

use std::time::Duration;

use tuneforge_io::transport::Transport;

use crate::can_upload::{CAN_REQUEST_ID, CAN_RESPONSE_ID};
use crate::error::KernelError;

/// Размер chunk-а в одном запросе read memory. Из capture: `0x0800` = 2048.
pub const READ_CHUNK_SIZE: u16 = 0x0800;

/// Послать proprietary CAN-kernel команду и вернуть response payload (без CAN_ID).
fn kernel_request(
    tr: &mut dyn Transport,
    cmd: &[u8],
    timeout: Duration,
) -> Result<Vec<u8>, KernelError> {
    let mut tx = Vec::with_capacity(4 + cmd.len());
    tx.extend_from_slice(&CAN_REQUEST_ID.to_be_bytes());
    tx.extend_from_slice(cmd);
    tr.write_all(&tx, timeout)?;

    let mut buf = vec![0u8; 4096];
    let n = tr.read_frame(&mut buf, timeout)?;
    if n < 4 {
        return Err(KernelError::UploadAborted(format!(
            "kernel RX too short: {n} bytes"
        )));
    }
    let resp_id = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if resp_id != CAN_RESPONSE_ID {
        return Err(KernelError::UploadAborted(format!(
            "wrong CAN response ID: 0x{resp_id:08X}"
        )));
    }
    Ok(buf[4..n].to_vec())
}

/// Прочитать banner kernel-а: TX `01` → expect `0x81 <ASCII>`.
/// Подтверждает что kernel живой и готов.
pub fn handshake(tr: &mut dyn Transport, timeout: Duration) -> Result<String, KernelError> {
    let resp = kernel_request(tr, &[0x01], timeout)?;
    if resp.first() != Some(&0x81) {
        return Err(KernelError::HandoverFailed);
    }
    // Все печатные ASCII-байты после command echo (0x81) до первого NUL.
    let banner: String = resp[1..]
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect();
    Ok(banner)
}

/// Прочитать `len` байт из ROM по адресу `addr` (32-bit absolute).
/// `len` ≤ [`READ_CHUNK_SIZE`] (2048). За один проход — multi-frame ISO-TP,
/// Tactrix re-assembles, нам прилетает один большой RxEnd.
pub fn read_memory(
    tr: &mut dyn Transport,
    addr32: u32,
    len: u16,
    timeout: Duration,
) -> Result<Vec<u8>, KernelError> {
    let cmd = [
        0x03,
        ((addr32 >> 24) & 0xFF) as u8,
        ((addr32 >> 16) & 0xFF) as u8,
        ((addr32 >> 8) & 0xFF) as u8,
        (addr32 & 0xFF) as u8,
        ((len >> 8) & 0xFF) as u8,
        (len & 0xFF) as u8,
    ];
    let resp = kernel_request(tr, &cmd, timeout)?;
    if resp.first() != Some(&0x83) {
        return Err(KernelError::UploadAborted(format!(
            "read_memory unexpected echo: 0x{:02X?}",
            resp.first(),
        )));
    }
    let data = &resp[1..];
    if data.len() != len as usize {
        return Err(KernelError::UploadAborted(format!(
            "read_memory length mismatch: got {}, requested {}",
            data.len(),
            len,
        )));
    }
    Ok(data.to_vec())
}

/// Полный ROM-дамп через kernel-side read-memory loop.
///
/// `progress(done, total)` зовётся после каждого chunk-а.
pub fn dump_rom_via_can_kernel(
    tr: &mut dyn Transport,
    start_addr: u32,
    total_bytes: usize,
    chunk_size: u16,
    timeout: Duration,
    mut progress: impl FnMut(usize, usize),
) -> Result<Vec<u8>, KernelError> {
    if chunk_size == 0 || chunk_size > READ_CHUNK_SIZE {
        return Err(KernelError::UploadAborted(format!(
            "chunk_size {chunk_size} вне допустимого 1..={READ_CHUNK_SIZE}"
        )));
    }
    let mut out = Vec::with_capacity(total_bytes);
    let mut addr = start_addr;
    progress(0, total_bytes);
    while out.len() < total_bytes {
        let remaining = total_bytes - out.len();
        let this_chunk = remaining.min(chunk_size as usize) as u16;
        let data = read_memory(tr, addr, this_chunk, timeout)?;
        out.extend_from_slice(&data);
        addr += u32::from(this_chunk);
        progress(out.len(), total_bytes);
    }
    Ok(out)
}
