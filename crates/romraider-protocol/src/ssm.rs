//! Subaru Select Monitor (SSM2/SSM3) protocol.
//!
//! Кадр запроса (тула → ECU): `80 10 F0 N CMD [PAYLOAD] CK`.
//! Кадр ответа  (ECU → тула): `80 F0 10 N (CMD|0x40) [DATA] CK`.
//! Length-байт `N` — счётчик байт после length и до контрольной суммы
//! (включая CMD). Контрольная сумма — простое сложение всех предшествующих
//! байт по модулю 256. См. `com.romraider.io.protocol.ssm.SSMProtocol`.

use std::time::Duration;

use romraider_core::Address;
use romraider_io::Transport;
use tracing::{debug, instrument};

use crate::error::{ProtocolError, ProtocolResult};

pub const HEADER:    u8 = 0x80;
pub const ECU_ADDR:  u8 = 0x10;
pub const TOOL_ADDR: u8 = 0xF0;

/// Бит, который ECU добавляет к командному байту в ответе.
pub const RESPONSE_FLAG: u8 = 0x40;

/// Минимальный размер SSM-кадра: header (3) + length (1) + cmd (1) + checksum (1).
pub const MIN_FRAME_LEN: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Command {
    ReadBlock    = 0xA0,
    ReadAddress  = 0xA8,
    WriteBlock   = 0xB0,
    WriteAddress = 0xB8,
    EcuInit      = 0xBF,
}

impl Command {
    /// Командный байт, который ECU вернёт в ответе на этот запрос.
    #[must_use]
    pub const fn response_byte(self) -> u8 {
        (self as u8) | RESPONSE_FLAG
    }
}

#[must_use]
pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b))
}

#[must_use]
pub fn build_request(cmd: Command, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + MIN_FRAME_LEN);
    frame.push(HEADER);
    frame.push(ECU_ADDR);
    frame.push(TOOL_ADDR);
    frame.push(u8::try_from(payload.len() + 1).expect("SSM payload < 255"));
    frame.push(cmd as u8);
    frame.extend_from_slice(payload);
    frame.push(checksum(&frame));
    frame
}

/// Распарсенный SSM-кадр от ECU. Слайсы заимствуют из исходного буфера.
#[derive(Debug, Clone)]
pub struct Response<'a> {
    pub command_echo: u8,
    pub data:         &'a [u8],
}

/// Разобрать кадр от ECU.
///
/// Проверяет: заголовок `80 F0 10`, length-байт ≥ 1, точное совпадение
/// `frame.len()` с `4 + length + 1`, контрольную сумму.
pub fn parse_response(frame: &[u8]) -> ProtocolResult<Response<'_>> {
    if frame.len() < MIN_FRAME_LEN {
        return Err(ProtocolError::ResponseTooShort {
            got: frame.len(),
            expected: MIN_FRAME_LEN,
        });
    }
    if frame[0] != HEADER || frame[1] != TOOL_ADDR || frame[2] != ECU_ADDR {
        return Err(ProtocolError::UnexpectedResponse(frame[0]));
    }
    let len = frame[3] as usize;
    if len < 1 {
        return Err(ProtocolError::ResponseTooShort { got: len, expected: 1 });
    }
    let total = 4 + len + 1;
    if frame.len() != total {
        return Err(ProtocolError::ResponseTooShort {
            got: frame.len(),
            expected: total,
        });
    }
    let body = &frame[..total - 1];
    let got  = frame[total - 1];
    let exp  = checksum(body);
    if got != exp {
        return Err(ProtocolError::ChecksumMismatch { got, expected: exp });
    }
    Ok(Response {
        command_echo: frame[4],
        data:         &frame[5..total - 1],
    })
}

/// Жадно прочитать ровно один SSM-кадр из транспорта, опираясь на length-байт.
///
/// Реальный serial-порт может вернуть данные кусками; этот хелпер дочитывает
/// до полного кадра. Лишние байты после кадра отбрасываются — для SSM
/// мульти-кадровый поток не предусмотрен.
pub fn read_complete_frame<T: Transport + ?Sized>(
    transport: &mut T,
    timeout:   Duration,
) -> ProtocolResult<Vec<u8>> {
    let mut frame = Vec::with_capacity(64);
    let mut buf   = [0u8; 256];

    loop {
        let n = transport.read_frame(&mut buf, timeout)?;
        frame.extend_from_slice(&buf[..n]);
        if frame.len() < 4 {
            continue;
        }
        let total = 4 + frame[3] as usize + 1;
        if frame.len() >= total {
            frame.truncate(total);
            return Ok(frame);
        }
    }
}

/// Распарсенный ответ ECU на `EcuInit`.
///
/// Формат данных (после байта-эха `0xFF`):
/// - SSM ID: 3 байта (идентификатор семейства Diagnostic CPU);
/// - ROM ID: 5 байт (calibration ID — обычно ASCII);
/// - capabilities: остальные байты — bitmap поддерживаемых параметров логгера.
///
/// Точная разметка: см. `com.romraider.io.protocol.ssm.iface.SSMEcuInit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EcuInitResponse {
    pub ssm_id:       [u8; 3],
    pub rom_id:       [u8; 5],
    pub capabilities: Vec<u8>,
}

const ECU_INIT_MIN_DATA: usize = 8; // 3 (SSM ID) + 5 (ROM ID)

pub fn decode_ecu_init(data: &[u8]) -> ProtocolResult<EcuInitResponse> {
    if data.len() < ECU_INIT_MIN_DATA {
        return Err(ProtocolError::ResponseTooShort {
            got: data.len(),
            expected: ECU_INIT_MIN_DATA,
        });
    }
    let mut ssm_id = [0u8; 3];
    ssm_id.copy_from_slice(&data[0..3]);
    let mut rom_id = [0u8; 5];
    rom_id.copy_from_slice(&data[3..8]);
    Ok(EcuInitResponse {
        ssm_id,
        rom_id,
        capabilities: data[8..].to_vec(),
    })
}

/// High-level: послать `EcuInit` и вернуть распарсенный ответ.
#[instrument(skip(transport))]
pub fn ecu_init<T: Transport + ?Sized>(
    transport: &mut T,
    timeout:   Duration,
) -> ProtocolResult<EcuInitResponse> {
    let frame = build_request(Command::EcuInit, &[]);
    debug!(bytes = frame.len(), "SSM ecu-init request");
    transport.write_all(&frame, timeout)?;

    let raw      = read_complete_frame(transport, timeout)?;
    let response = parse_response(&raw)?;
    if response.command_echo != Command::EcuInit.response_byte() {
        return Err(ProtocolError::UnexpectedResponse(response.command_echo));
    }
    decode_ecu_init(response.data)
}

#[instrument(skip(transport, addresses))]
pub fn read_addresses<T: Transport + ?Sized>(
    transport: &mut T,
    addresses: &[Address],
    pad_byte:  u8,
    timeout:   Duration,
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

    let raw      = read_complete_frame(transport, timeout)?;
    let response = parse_response(&raw)?;
    if response.command_echo != Command::ReadAddress.response_byte() {
        return Err(ProtocolError::UnexpectedResponse(response.command_echo));
    }
    if response.data.len() != addresses.len() {
        return Err(ProtocolError::ResponseTooShort {
            got: response.data.len(),
            expected: addresses.len(),
        });
    }
    Ok(response.data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use romraider_io::mock::MockTransport;

    /// Собрать корректный кадр-ответ ECU вокруг `inner` (cmd-echo + data).
    fn pack_response(inner: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(inner.len() + MIN_FRAME_LEN);
        frame.push(HEADER);
        frame.push(TOOL_ADDR);
        frame.push(ECU_ADDR);
        frame.push(u8::try_from(inner.len()).expect("inner < 255"));
        frame.extend_from_slice(inner);
        frame.push(checksum(&frame));
        frame
    }

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

    #[test]
    fn response_byte_sets_bit6() {
        assert_eq!(Command::EcuInit.response_byte(), 0xFF);
        assert_eq!(Command::ReadAddress.response_byte(), 0xE8);
    }

    #[test]
    fn parse_response_validates_a_known_good_frame() {
        let mut inner = vec![Command::EcuInit.response_byte()];
        inner.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        inner.extend_from_slice(b"AB123");
        let frame = pack_response(&inner);

        let r = parse_response(&frame).unwrap();
        assert_eq!(r.command_echo, 0xFF);
        assert_eq!(
            r.data,
            &[0xAA, 0xBB, 0xCC, b'A', b'B', b'1', b'2', b'3']
        );
    }

    #[test]
    fn parse_response_rejects_bad_checksum() {
        let mut frame = pack_response(&[Command::EcuInit.response_byte()]);
        let last = frame.len() - 1;
        frame[last] = frame[last].wrapping_add(1);
        match parse_response(&frame) {
            Err(ProtocolError::ChecksumMismatch { .. }) => {}
            other => panic!("expected ChecksumMismatch, got {other:?}"),
        }
    }

    #[test]
    fn parse_response_rejects_wrong_header() {
        let frame = [0x81, TOOL_ADDR, ECU_ADDR, 0x01, 0xFF, 0x00];
        assert!(matches!(
            parse_response(&frame),
            Err(ProtocolError::UnexpectedResponse(0x81))
        ));
    }

    #[test]
    fn parse_response_rejects_too_short_buffer() {
        let frame = [HEADER, TOOL_ADDR, ECU_ADDR];
        assert!(matches!(
            parse_response(&frame),
            Err(ProtocolError::ResponseTooShort { .. })
        ));
    }

    #[test]
    fn parse_response_rejects_length_mismatch() {
        // Length-байт говорит «3», а реально payload + checksum уложатся в 6 байт.
        let frame = [HEADER, TOOL_ADDR, ECU_ADDR, 0x03, 0xFF, 0x00];
        assert!(matches!(
            parse_response(&frame),
            Err(ProtocolError::ResponseTooShort { .. })
        ));
    }

    #[test]
    fn decode_ecu_init_extracts_ids_and_caps() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        data.extend_from_slice(b"AB123");
        data.extend_from_slice(&[0xFF, 0x00, 0xFF, 0x00]);
        let init = decode_ecu_init(&data).unwrap();
        assert_eq!(init.ssm_id, [0xAA, 0xBB, 0xCC]);
        assert_eq!(&init.rom_id, b"AB123");
        assert_eq!(init.capabilities, vec![0xFF, 0x00, 0xFF, 0x00]);
    }

    #[test]
    fn decode_ecu_init_rejects_short_data() {
        assert!(matches!(
            decode_ecu_init(&[0xAA; 7]),
            Err(ProtocolError::ResponseTooShort { .. })
        ));
    }

    #[test]
    fn ecu_init_round_trip_via_mock() {
        let mut inner = vec![Command::EcuInit.response_byte()];
        inner.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        inner.extend_from_slice(b"AB123");
        let response = pack_response(&inner);

        let mut t = MockTransport::with_responses([response]);
        let init  = ecu_init(&mut t, Duration::from_millis(100)).unwrap();

        assert_eq!(init.ssm_id, [0xAA, 0xBB, 0xCC]);
        assert_eq!(&init.rom_id, b"AB123");
        assert!(init.capabilities.is_empty());

        let expected_request = build_request(Command::EcuInit, &[]);
        assert_eq!(t.last_write(), Some(expected_request.as_slice()));
    }

    #[test]
    fn read_addresses_round_trip_via_mock() {
        let address  = Address::new(0x12_3456);
        let response = pack_response(&[Command::ReadAddress.response_byte(), 0x42]);
        let mut t    = MockTransport::with_responses([response]);

        let data = read_addresses(&mut t, &[address], 0x00, Duration::from_millis(100)).unwrap();
        assert_eq!(data, vec![0x42]);

        let expected_request = build_request(Command::ReadAddress, &[0x00, 0x12, 0x34, 0x56]);
        assert_eq!(t.last_write(), Some(expected_request.as_slice()));
    }

    #[test]
    fn ecu_init_rejects_wrong_command_echo() {
        // ECU отвечает с echo для другой команды → UnexpectedResponse.
        let mut inner = vec![Command::ReadAddress.response_byte()];
        inner.extend_from_slice(&[0u8; 8]); // 8 байт после cmd, чтобы пройти длину
        let response = pack_response(&inner);

        let mut t = MockTransport::with_responses([response]);
        let err   = ecu_init(&mut t, Duration::from_millis(100)).unwrap_err();
        assert!(matches!(err, ProtocolError::UnexpectedResponse(0xE8)));
    }
}
