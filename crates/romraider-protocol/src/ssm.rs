//! Subaru Select Monitor (SSM2/SSM3).
//!
//! Кадр: `0x80 dst src len cmd [payload…] checksum` (см. `com.romraider.io.protocol.ssm.SSMProtocol`).
//! Контрольная сумма — простое сложение по модулю 256 всех предшествующих байт.

use std::time::Duration;

use romraider_core::Address;
use romraider_io::Transport;
use tracing::{debug, instrument};

use crate::error::{ProtocolError, ProtocolResult};

pub const HEADER:   u8 = 0x80;
pub const ECU_ADDR: u8 = 0x10;
pub const TOOL_ADDR: u8 = 0xF0;

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum Command {
    ReadBlock      = 0xA0,
    ReadAddress    = 0xA8,
    WriteBlock     = 0xB0,
    WriteAddress   = 0xB8,
    EcuInit        = 0xBF,
}

#[must_use]
pub fn build_request(cmd: Command, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 6);
    frame.push(HEADER);
    frame.push(ECU_ADDR);
    frame.push(TOOL_ADDR);
    frame.push(u8::try_from(payload.len() + 1).expect("SSM payload < 255"));
    frame.push(cmd as u8);
    frame.extend_from_slice(payload);
    frame.push(checksum(&frame));
    frame
}

#[must_use]
pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b))
}

#[instrument(skip(transport, addresses))]
pub fn read_addresses<T: Transport + ?Sized>(
    transport:   &mut T,
    addresses:   &[Address],
    pad_byte:    u8,
    timeout:     Duration,
) -> ProtocolResult<Vec<u8>> {
    let mut payload = Vec::with_capacity(1 + addresses.len() * 3);
    payload.push(pad_byte);
    for addr in addresses {
        let raw = addr.raw();
        payload.extend_from_slice(&[
            ((raw >> 16) & 0xFF) as u8,
            ((raw >>  8) & 0xFF) as u8,
            (raw & 0xFF) as u8,
        ]);
    }
    let frame = build_request(Command::ReadAddress, &payload);
    debug!(bytes = frame.len(), "SSM read-addresses request");

    transport.write_all(&frame, timeout)?;

    let mut resp = vec![0u8; 256];
    let n = transport.read_frame(&mut resp, timeout)?;
    resp.truncate(n);
    parse_response(&resp, addresses.len())
}

fn parse_response(frame: &[u8], expected_data: usize) -> ProtocolResult<Vec<u8>> {
    if frame.len() < expected_data + 6 {
        return Err(ProtocolError::ResponseTooShort {
            got: frame.len(),
            expected: expected_data + 6,
        });
    }
    let body = &frame[..frame.len() - 1];
    let got = frame[frame.len() - 1];
    let expected = checksum(body);
    if got != expected {
        return Err(ProtocolError::ChecksumMismatch { got, expected });
    }
    // 5 байт заголовка + 1 байт «echo cmd» уже в frame; данные — до контрольной суммы.
    Ok(frame[5..frame.len() - 1].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_wraps() {
        assert_eq!(checksum(&[0xFF, 0x01]), 0x00);
        assert_eq!(checksum(&[0x80, 0x10, 0xF0, 0x01, 0xBF]), 0x40);
    }

    #[test]
    fn ecu_init_request_matches_subaru_spec() {
        let frame = build_request(Command::EcuInit, &[]);
        assert_eq!(frame, vec![0x80, 0x10, 0xF0, 0x01, 0xBF, 0x40]);
    }
}
