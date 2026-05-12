//! KWP2000 (ISO-14230) wire-framing для общения с Subaru ECU в programming-режиме.
//!
//! Формат кадра идентичен SSM2 на уровне байтов, но семантически другой
//! (KWP2000 SID-ы вместо SSM-команд):
//!
//! ```text
//! 80 <target> <source> <length> <SID> [data ...] <checksum>
//! ```
//!
//! - `80` — header byte (всегда `0x80` для address-based + separate length)
//! - `target` — `0x10` (ECU)
//! - `source` — `0xF0` (tester / diag tool)
//! - `length` — число байт после length-байта, не включая checksum
//! - `checksum` — `sum(all bytes before chksm) & 0xFF`
//!
//! Negative response: ECU отвечает фреймом с **SID = 0x7F** + `<request_sid>`
//! + `<NRC>`. Положительный ответ — `<request_sid> | 0x40` + payload.

use crate::error::{KernelError, KernelResult};

/// Декодированный response-фрейм.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub target:   u8,
    pub source:   u8,
    /// SID ответа: для положительного — `request_sid | 0x40`, для негативного — `0x7F`.
    pub sid:      u8,
    /// Payload **без** SID-байта и без checksum.
    pub data:     Vec<u8>,
}

pub const HEADER:     u8 = 0x80;
pub const ECU_ADDR:   u8 = 0x10;
pub const TOOL_ADDR:  u8 = 0xF0;

// SID-ы (request side). Response side = SID | 0x40.
pub mod sid {
    pub const START_COMMUNICATION:     u8 = 0x81;
    pub const STOP_COMMUNICATION:      u8 = 0x82;
    pub const ACCESS_TIMING_PARAMETERS: u8 = 0x83;
    pub const START_DIAGNOSTIC_SESSION: u8 = 0x10;
    pub const TESTER_PRESENT:          u8 = 0x3E;
    pub const ECU_RESET:               u8 = 0x11;
    pub const READ_DATA_BY_ID:         u8 = 0x22;
    pub const READ_MEMORY_BY_ADDRESS:  u8 = 0x23;
    pub const SECURITY_ACCESS:         u8 = 0x27;
    pub const WRITE_DATA_BY_ID:        u8 = 0x2E;
    pub const WRITE_MEMORY_BY_ADDRESS: u8 = 0x3D;
    pub const REQUEST_DOWNLOAD:        u8 = 0x34;
    pub const TRANSFER_DATA:           u8 = 0x36;
    pub const REQUEST_TRANSFER_EXIT:   u8 = 0x37;
    pub const START_ROUTINE_BY_ADDRESS: u8 = 0x31;
}

/// Negative Response Code (тип ответа `0x7F <sid> <nrc>`).
pub mod nrc {
    pub const GENERAL_REJECT:                      u8 = 0x10;
    pub const SERVICE_NOT_SUPPORTED:               u8 = 0x11;
    pub const SUB_FUNCTION_NOT_SUPPORTED:          u8 = 0x12;
    pub const BUSY_REPEAT_REQUEST:                 u8 = 0x21;
    pub const CONDITIONS_NOT_CORRECT:              u8 = 0x22;
    pub const REQUEST_OUT_OF_RANGE:                u8 = 0x31;
    pub const SECURITY_ACCESS_DENIED:              u8 = 0x33;
    pub const INVALID_KEY:                         u8 = 0x35;
    pub const EXCEEDED_NUMBER_OF_ATTEMPTS:         u8 = 0x36;
    pub const REQUIRED_TIME_DELAY_NOT_EXPIRED:     u8 = 0x37;
    pub const RESPONSE_PENDING:                    u8 = 0x78;
}

/// Маркер «negative response» в SID-байте ответа.
pub const NEGATIVE_RESPONSE_SID: u8 = 0x7F;

/// Положительный ответ всегда имеет SID = `request_sid | 0x40`.
pub const POSITIVE_RESPONSE_MASK: u8 = 0x40;

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b))
}

/// Собрать KWP2000-запрос: `80 <target> <source> <length> <sid> [data..] <chksm>`.
///
/// Использует стандартные адреса `ECU_ADDR`=0x10 (target) и `TOOL_ADDR`=0xF0 (source).
pub fn build_request(sid: u8, data: &[u8]) -> Vec<u8> {
    build_request_to(ECU_ADDR, TOOL_ADDR, sid, data)
}

/// Тот же [`build_request`], но с настраиваемыми target/source — нужно для
/// общения с не-engine-модулями (TCU, ABS и т.п.) и для тестов.
pub fn build_request_to(target: u8, source: u8, sid: u8, data: &[u8]) -> Vec<u8> {
    // length = SID(1) + data.len()
    let length = 1 + data.len();
    debug_assert!(length <= 0xFF, "KWP2000 frame length must fit in 1 byte");

    let mut frame = Vec::with_capacity(5 + data.len() + 1);
    frame.push(HEADER);
    frame.push(target);
    frame.push(source);
    frame.push(length as u8);
    frame.push(sid);
    frame.extend_from_slice(data);
    let chksm = checksum(&frame);
    frame.push(chksm);
    frame
}

/// Распарсить полный KWP2000-кадр. Возвращает `Frame` + сколько байт съели
/// (чтобы вызывающий мог обрабатывать back-to-back-кадры в буфере).
///
/// Возможные ошибки:
/// - `Framing` — короткий буфер, плохой header, неверный checksum или длина.
/// - `NegativeResponse` — успешно распарсили `7F <sid> <nrc>`-ответ.
pub fn parse_response(raw: &[u8]) -> KernelResult<(usize, Frame)> {
    if raw.len() < 6 {
        return Err(KernelError::Framing(format!(
            "frame too short: need ≥6 bytes, got {}", raw.len()
        )));
    }
    if raw[0] != HEADER {
        return Err(KernelError::Framing(format!(
            "expected header 0x80, got 0x{:02X}", raw[0]
        )));
    }
    let target = raw[1];
    let source = raw[2];
    let length = raw[3] as usize;
    if length == 0 {
        return Err(KernelError::Framing("length byte is zero".into()));
    }
    let total_len = 4 + length + 1; // header(1) + target(1) + source(1) + length-byte(1) + payload + checksum(1)
    if raw.len() < total_len {
        return Err(KernelError::Framing(format!(
            "frame truncated: need {total_len} bytes, got {}", raw.len()
        )));
    }

    let sid     = raw[4];
    let data    = raw[5..4 + length].to_vec();
    let actual_chksm   = raw[4 + length];
    let expected_chksm = checksum(&raw[..4 + length]);
    if actual_chksm != expected_chksm {
        return Err(KernelError::Framing(format!(
            "checksum mismatch: expected 0x{expected_chksm:02X}, got 0x{actual_chksm:02X}"
        )));
    }

    // Negative-response detect:
    if sid == NEGATIVE_RESPONSE_SID && data.len() >= 2 {
        let req_sid = data[0];
        let nrc     = data[1];
        return Err(KernelError::NegativeResponse {
            request: req_sid,
            nrc,
            nrc_meaning: nrc_meaning(nrc),
        });
    }

    Ok((total_len, Frame { target, source, sid, data }))
}

/// Высокоуровневая обёртка: отправить запрос, прочитать кадр, распарсить.
///
/// Используется в [`crate::upload`] и [`crate::kernel_wire`] как единая
/// единица request/response-обмена. На NRC возвращает `KernelError::NegativeResponse`.
pub fn request_response(
    transport: &mut dyn romraider_io::transport::Transport,
    sid:       u8,
    data:      &[u8],
    timeout:   std::time::Duration,
) -> KernelResult<Frame> {
    let req = build_request(sid, data);
    tracing::trace!(sid = format!("0x{sid:02X}"), bytes = req.len(), "KWP2000 request");
    transport.write_all(&req, timeout)?;

    let mut buf = [0u8; 512];
    let n = transport.read_frame(&mut buf, timeout)?;
    let (_consumed, frame) = parse_response(&buf[..n])?;

    // Sanity: положительный SID должен быть `request | 0x40`. Если что-то
    // другое — кричим. (NRC-случай уже обработан в parse_response.)
    let expected = sid | POSITIVE_RESPONSE_MASK;
    if frame.sid != expected {
        return Err(KernelError::UnexpectedSid { expected: sid, got: frame.sid });
    }
    tracing::trace!(sid = format!("0x{:02X}", frame.sid), data_len = frame.data.len(), "KWP2000 response");
    Ok(frame)
}

/// Текстовое описание NRC-кода для error-message.
pub const fn nrc_meaning(nrc: u8) -> &'static str {
    match nrc {
        nrc::GENERAL_REJECT                  => "general reject",
        nrc::SERVICE_NOT_SUPPORTED           => "service not supported",
        nrc::SUB_FUNCTION_NOT_SUPPORTED      => "sub-function not supported",
        nrc::BUSY_REPEAT_REQUEST             => "busy, repeat request",
        nrc::CONDITIONS_NOT_CORRECT          => "conditions not correct",
        nrc::REQUEST_OUT_OF_RANGE            => "request out of range",
        nrc::SECURITY_ACCESS_DENIED          => "security access denied",
        nrc::INVALID_KEY                     => "invalid key",
        nrc::EXCEEDED_NUMBER_OF_ATTEMPTS     => "exceeded number of attempts (10-sec lockout)",
        nrc::REQUIRED_TIME_DELAY_NOT_EXPIRED => "required time delay not expired",
        nrc::RESPONSE_PENDING                => "response pending (keep waiting)",
        _                                    => "unknown NRC",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_simple_no_payload() {
        // SID 0x81 (startCommunication) без данных — самый простой кадр.
        let frame = build_request(sid::START_COMMUNICATION, &[]);
        // 80 10 F0 01 81 <chksm>; chksm = (0x80+0x10+0xF0+0x01+0x81) & 0xFF
        // = 0x80+0x10=0x90, +0xF0=0x180→0x80, +0x01=0x81, +0x81=0x102→0x02
        assert_eq!(frame, vec![0x80, 0x10, 0xF0, 0x01, 0x81, 0x02]);
    }

    #[test]
    fn build_request_with_data() {
        // SID 0x10 startDiagnosticSession sub=0x85 (programming-mode).
        let frame = build_request(sid::START_DIAGNOSTIC_SESSION, &[0x85]);
        // 80 10 F0 02 10 85 <chksm>; sum = 80+10+F0+02+10+85 = 0x217 → 0x17
        assert_eq!(frame, vec![0x80, 0x10, 0xF0, 0x02, 0x10, 0x85, 0x17]);
    }

    #[test]
    fn parse_positive_response() {
        // Эмулируем ECU-ответ на `startCommunication`: SID 0x81 | 0x40 = 0xC1.
        // 80 F0 10 03 C1 EA 8F <chksm>
        // sum: 80+F0+10+03+C1+EA+8F = 0x3BD → 0xBD
        let raw = [0x80, 0xF0, 0x10, 0x03, 0xC1, 0xEA, 0x8F, 0xBD];
        let (consumed, frame) = parse_response(&raw).unwrap();
        assert_eq!(consumed, raw.len());
        assert_eq!(frame.target, 0xF0);
        assert_eq!(frame.source, 0x10);
        assert_eq!(frame.sid,    0xC1);
        assert_eq!(frame.data,   vec![0xEA, 0x8F]); // KB1 KB2 (key bytes ISO-14230)
    }

    #[test]
    fn parse_negative_response_yields_nrc_error() {
        // ECU отвечает 7F <sid> <nrc> на запрос securityAccess: invalid key (NRC 0x35).
        // 80 F0 10 03 7F 27 35 <chksm>
        // sum: 80+F0+10+03+7F+27+35 = 0x25E → 0x5E
        let raw = [0x80, 0xF0, 0x10, 0x03, 0x7F, 0x27, 0x35, 0x5E];
        let err = parse_response(&raw).unwrap_err();
        match err {
            KernelError::NegativeResponse { request, nrc, .. } => {
                assert_eq!(request, 0x27);
                assert_eq!(nrc,     0x35);
            }
            other => panic!("expected NegativeResponse, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_bad_checksum() {
        let raw = [0x80, 0x10, 0xF0, 0x01, 0x81, 0xFF]; // wrong chksm (should be 0x02)
        let err = parse_response(&raw).unwrap_err();
        assert!(matches!(err, KernelError::Framing(_)), "got {err:?}");
    }

    #[test]
    fn parse_rejects_bad_header() {
        let raw = [0x81, 0xF0, 0x10, 0x01, 0xC1, 0x42]; // header = 0x81 (invalid)
        let err = parse_response(&raw).unwrap_err();
        assert!(matches!(err, KernelError::Framing(_)));
    }

    #[test]
    fn parse_rejects_truncated_frame() {
        let raw = [0x80, 0x10, 0xF0, 0x05, 0x10]; // length=5 but no data
        let err = parse_response(&raw).unwrap_err();
        assert!(matches!(err, KernelError::Framing(_)));
    }

    #[test]
    fn build_and_parse_round_trip() {
        // Соберём запрос, потом «эмулируем» ECU-ответ положительным эхо.
        let req = build_request(sid::TESTER_PRESENT, &[]);
        let (n_req, _) = parse_response(&req).unwrap();
        assert_eq!(n_req, req.len());
    }

    #[test]
    fn request_response_via_mock_transport() {
        use romraider_io::mock::MockTransport;
        use std::time::Duration;

        // ECU замокаем так, чтобы он на любой запрос отвечал `<sid|0x40> AA BB`.
        // На SID 0x3E (testerPresent) положительный ответ = 0x7E.
        // 80 F0 10 03 7E AA BB <chksm>; sum = 80+F0+10+03+7E+AA+BB = 0x366 → 0x66
        let response = vec![0x80, 0xF0, 0x10, 0x03, 0x7E, 0xAA, 0xBB, 0x66];
        let mut mock = MockTransport::with_responses(vec![response]);

        let frame = request_response(
            &mut mock,
            sid::TESTER_PRESENT,
            &[],
            Duration::from_millis(50),
        ).unwrap();

        assert_eq!(frame.sid,  0x7E);
        assert_eq!(frame.data, vec![0xAA, 0xBB]);
    }

    #[test]
    fn request_response_propagates_nrc() {
        use romraider_io::mock::MockTransport;
        use std::time::Duration;

        // ECU отвечает 7F 27 35 (security access invalid key) на наш 0x27.
        // 80 F0 10 03 7F 27 35 <chksm = 0x5E>
        let response = vec![0x80, 0xF0, 0x10, 0x03, 0x7F, 0x27, 0x35, 0x5E];
        let mut mock = MockTransport::with_responses(vec![response]);

        let err = request_response(
            &mut mock,
            sid::SECURITY_ACCESS,
            &[0x02, 0xDE, 0xAD, 0xBE, 0xEF],
            Duration::from_millis(50),
        ).unwrap_err();

        match err {
            KernelError::NegativeResponse { request, nrc, .. } => {
                assert_eq!(request, 0x27);
                assert_eq!(nrc,     0x35);
            }
            other => panic!("expected NegativeResponse, got {other:?}"),
        }
    }
}
