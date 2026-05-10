//! OBD-II Mode 01 (current data) — минимальный набор для логгера.
//!
//! Полная имплементация ISO-15765-2 (CAN) живёт под J2534-каналом.
//! Здесь только сборка запроса и валидация ответа.

use crate::error::{ProtocolError, ProtocolResult};

pub const MODE_01_RESPONSE: u8 = 0x41;

#[must_use]
pub fn build_mode_01(pid: u8) -> [u8; 2] {
    [0x01, pid]
}

pub fn parse_mode_01(pid: u8, response: &[u8]) -> ProtocolResult<&[u8]> {
    if response.len() < 2 {
        return Err(ProtocolError::ResponseTooShort { got: response.len(), expected: 2 });
    }
    if response[0] != MODE_01_RESPONSE {
        return Err(ProtocolError::UnexpectedResponse(response[0]));
    }
    if response[1] != pid {
        return Err(ProtocolError::UnexpectedResponse(response[1]));
    }
    Ok(&response[2..])
}
