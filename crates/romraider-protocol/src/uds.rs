//! UDS (Unified Diagnostic Services, ISO-14229) over ISO15765 / CAN.
//!
//! Этот модуль реализует *общие* UDS-сервисы: ReadMemoryByAddress (0x23),
//! DiagnosticSessionControl (0x10), SecurityAccess (0x27) и т.п. — без
//! Subaru-специфики. Subaru-специфичные расширения (Mode 0xA8 SSM-over-CAN,
//! firmware-bound seed/key с round-keys из ROM) живут в `romraider-kernel`.
//!
//! Текущий scope (Slice 24c):
//! - [`read_memory_by_address`] — `23 <ALFID> <addr> <len>` → `63 <data>`.
//!   На Subaru SH7058 + ISO15765 default session работает без security
//!   access для **RAM-адресов** (`0xFFxxxx`). ROM-адреса (`0x00xxxx`–`0x0Fxxxx`)
//!   на 2007+ режутся анти-fuzz защитой → для них нужен kernel-upload
//!   (см. `romraider-kernel::can_wire`).

use std::time::Duration;

use romraider_io::transport::Transport;

use crate::error::{ProtocolError, ProtocolResult};

/// CAN OBD-II 11-bit ECU **request** ID (то же, что `obd2::OBD_REQUEST_ID`).
pub const CAN_REQUEST_ID: u32 = 0x7E0;
/// CAN OBD-II 11-bit ECU **response** ID.
pub const CAN_RESPONSE_ID: u32 = 0x7E8;

/// UDS SID `0x23` `ReadMemoryByAddress`.
pub const SID_READ_MEMORY_BY_ADDRESS: u8 = 0x23;
/// Положительный ответ = SID | 0x40.
pub const RESP_READ_MEMORY_BY_ADDRESS: u8 = 0x63;

/// Address-Length Format Identifier для SH7058: 1-byte length, 4-byte addr.
/// High nibble = байты для memorySize, low nibble = байты для memoryAddress.
pub const ALFID_4B_ADDR_1B_LEN: u8 = 0x14;

/// Прочитать `len` байт памяти ECU по 32-bit адресу через UDS Mode `0x23`.
///
/// Wire:
/// - **TX:** `23 14 <addr_4B BE> <len_1B>`  (через CAN `0x7E0`)
/// - **RX:** `63 <data...>`                 (через CAN `0x7E8`)
///
/// Возвращает только data-payload (без `63` echo). Caller интерпретирует
/// raw байты под нужный storagetype (uint8, float BE и т.п.).
///
/// Negative responses (`7F 23 <NRC>`):
/// - `0x13` Incorrect Message Length / Invalid Format
/// - `0x22` Conditions Not Correct (нужна другая сессия)
/// - `0x31` Request Out Of Range (запрещённый адрес)
/// - `0x33` Security Access Denied (для ROM-адресов)
pub fn read_memory_by_address<T: Transport + ?Sized>(
    tr: &mut T,
    addr: u32,
    len: u8,
    timeout: Duration,
) -> ProtocolResult<Vec<u8>> {
    if len == 0 {
        return Err(ProtocolError::ResponseTooShort {
            got: 0,
            expected: 1,
        });
    }
    let mut tx = Vec::with_capacity(4 + 7);
    tx.extend_from_slice(&CAN_REQUEST_ID.to_be_bytes());
    tx.push(SID_READ_MEMORY_BY_ADDRESS);
    tx.push(ALFID_4B_ADDR_1B_LEN);
    tx.extend_from_slice(&addr.to_be_bytes());
    tx.push(len);
    tr.write_all(&tx, timeout)?;

    let mut buf = [0u8; 256];
    let n = tr.read_frame(&mut buf, timeout)?;
    if n < 4 + 2 {
        return Err(ProtocolError::ResponseTooShort {
            got: n,
            expected: 4 + 2,
        });
    }
    // Strip CAN_ID prefix
    let resp_id = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if resp_id != CAN_RESPONSE_ID {
        return Err(ProtocolError::UnexpectedResponse(buf[0]));
    }
    let uds = &buf[4..n];
    // Negative response: `7F 23 <NRC>`.
    if uds.len() >= 3 && uds[0] == 0x7F {
        // Возвращаем NRC как `UnexpectedResponse(NRC)` — caller увидит и
        // сможет debug-нуть. Полное наименование NRC обрабатывается выше.
        return Err(ProtocolError::UnexpectedResponse(uds[2]));
    }
    if uds.is_empty() || uds[0] != RESP_READ_MEMORY_BY_ADDRESS {
        return Err(ProtocolError::UnexpectedResponse(
            uds.first().copied().unwrap_or(0),
        ));
    }
    let data = &uds[1..];
    if data.len() < len as usize {
        return Err(ProtocolError::ResponseTooShort {
            got: data.len(),
            expected: len as usize,
        });
    }
    Ok(data[..len as usize].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alfid_constant_is_correct() {
        assert_eq!(ALFID_4B_ADDR_1B_LEN, 0x14);
        // high nibble = 1 byte size, low nibble = 4 byte addr
        assert_eq!(ALFID_4B_ADDR_1B_LEN >> 4, 0x1);
        assert_eq!(ALFID_4B_ADDR_1B_LEN & 0x0F, 0x4);
    }
}
