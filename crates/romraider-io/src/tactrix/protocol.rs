//! Парсер Tactrix-фреймов.
//!
//! Чистая функция [`parse_frame`] разбирает один логический фрейм из начала
//! среза `raw`. Возвращает `(consumed_bytes, frame)` или [`ParseError`].

/// Протокол-байты, которые Tactrix вставляет в data-фреймы как маркер канала.
pub const PROTO_ISO9141:  u8 = 0x33;
pub const PROTO_ISO14230: u8 = 0x34;
pub const PROTO_CAN:      u8 = 0x35;
pub const PROTO_ISO15765: u8 = 0x36;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TactrixFrame {
    /// `aro\r\n` (просто ack) или `aro<digit>\r\n` (ack с channel-id, например для `ato`).
    Ack { channel: Option<u8> },
    /// `ari <firmware-info>\r\n` — ответ на identify.
    Identify { info: String },
    /// `are<details>\r\n` — error response (например, на пустую команду).
    /// Не считается фатальной — handshake читает дальше.
    Error { info: String },
    /// `arf<filter_id>\r\n` — ack для PassThruStartMsgFilter (`atf`).
    FilterAck { id: u32 },
    /// Бинарный фрейм с данными.
    Data {
        /// Маркер канала: `0x33`..`0x36`. Совпадает с протокол-байтом из `ato`.
        protocol_byte: u8,
        kind:          PacketKind,
        /// Микросекундный таймстамп от Tactrix (big-endian в wire-формате).
        timestamp_us:  u32,
        payload:       Vec<u8>,
    },
}

/// Тип бинарного фрейма (поле `pkt_type` в wire-формате).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketKind {
    /// `0x00` — нормальное сообщение с полезной нагрузкой.
    Normal,
    /// `0x10` — подтверждение завершения TX.
    TxDone,
    /// `0x20` — loopback переданного сообщения.
    TxLoopback,
    /// `0x40` — индикатор конца приёма.
    RxEnd,
    /// `0x44` — конец приёма с расширенным адресом.
    RxEndExtended,
    /// `0x60` — конец loopback-сообщения.
    LoopbackEnd,
    /// `0x80` — начало нормального сообщения (для длинных кадров).
    NormalStart,
    /// `0xA0` — начало TX-loopback.
    TxLoopbackStart,
    /// Любое другое значение.
    Other(u8),
}

impl PacketKind {
    #[must_use]
    pub const fn from_byte(b: u8) -> Self {
        match b {
            0x00 => Self::Normal,
            0x10 => Self::TxDone,
            0x20 => Self::TxLoopback,
            0x40 => Self::RxEnd,
            0x44 => Self::RxEndExtended,
            0x60 => Self::LoopbackEnd,
            0x80 => Self::NormalStart,
            0xA0 => Self::TxLoopbackStart,
            b    => Self::Other(b),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Недостаточно байт для парсинга — нужно подождать ещё.
    NeedMoreData,
    /// Не нашёлся стартовый маркер `'a' 'r'` в начале.
    BadStartMarker,
    /// `len` < 5 — невозможно (нужны type + 4 байта timestamp).
    InvalidLen,
    /// ASCII-фрейм не UTF-8.
    InvalidUtf8,
    /// Третий байт не из {`'o'`, `'i'`, `0x33`..`0x36`}.
    UnknownDiscriminator(u8),
}

/// Распарсить один фрейм из начала `raw`. Возвращает сколько байт съели
/// и собранный [`TactrixFrame`]. При [`ParseError::NeedMoreData`] вызывайте
/// снова после дочитывания.
pub fn parse_frame(raw: &[u8]) -> Result<(usize, TactrixFrame), ParseError> {
    if raw.len() < 3 {
        return Err(ParseError::NeedMoreData);
    }
    if raw[0] != b'a' || raw[1] != b'r' {
        return Err(ParseError::BadStartMarker);
    }
    match raw[2] {
        b'o' => parse_ascii_ack(raw),
        b'i' => parse_ascii_identify(raw),
        b'e' => parse_ascii_error(raw),
        b'f' => parse_ascii_filter_ack(raw),
        b @ (PROTO_ISO9141 | PROTO_ISO14230 | PROTO_CAN | PROTO_ISO15765) => parse_binary(raw, b),
        other => Err(ParseError::UnknownDiscriminator(other)),
    }
}

fn parse_ascii_error(raw: &[u8]) -> Result<(usize, TactrixFrame), ParseError> {
    let lf = raw.iter().position(|&b| b == b'\n').ok_or(ParseError::NeedMoreData)?;
    let line = std::str::from_utf8(&raw[..lf]).map_err(|_| ParseError::InvalidUtf8)?;
    let trimmed = line.trim_end_matches('\r').trim_end();
    let info = trimmed.strip_prefix("are").unwrap_or(trimmed).trim().to_string();
    Ok((lf + 1, TactrixFrame::Error { info }))
}

fn parse_ascii_filter_ack(raw: &[u8]) -> Result<(usize, TactrixFrame), ParseError> {
    let lf = raw.iter().position(|&b| b == b'\n').ok_or(ParseError::NeedMoreData)?;
    let line = std::str::from_utf8(&raw[..lf]).map_err(|_| ParseError::InvalidUtf8)?;
    let trimmed = line.trim_end_matches('\r').trim_end();
    let body = trimmed.strip_prefix("arf").unwrap_or(trimmed).trim();
    let id = body.parse::<u32>().unwrap_or(0);
    Ok((lf + 1, TactrixFrame::FilterAck { id }))
}

fn parse_ascii_ack(raw: &[u8]) -> Result<(usize, TactrixFrame), ParseError> {
    let lf = raw.iter().position(|&b| b == b'\n').ok_or(ParseError::NeedMoreData)?;
    let line = std::str::from_utf8(&raw[..lf]).map_err(|_| ParseError::InvalidUtf8)?;
    let trimmed = line.trim_end_matches('\r');
    let after = trimmed.strip_prefix("aro").unwrap_or("");
    let channel = if after.is_empty() {
        None
    } else {
        after.parse::<u8>().ok()
    };
    Ok((lf + 1, TactrixFrame::Ack { channel }))
}

fn parse_ascii_identify(raw: &[u8]) -> Result<(usize, TactrixFrame), ParseError> {
    let lf = raw.iter().position(|&b| b == b'\n').ok_or(ParseError::NeedMoreData)?;
    let line = std::str::from_utf8(&raw[..lf]).map_err(|_| ParseError::InvalidUtf8)?;
    let trimmed = line.trim_end_matches('\r').trim_end();
    let info = trimmed.strip_prefix("ari").unwrap_or(trimmed).trim().to_string();
    Ok((lf + 1, TactrixFrame::Identify { info }))
}

fn parse_binary(raw: &[u8], proto_byte: u8) -> Result<(usize, TactrixFrame), ParseError> {
    // Layout: `'a' 'r' <proto> <len> <kind> <data: len-1 bytes>`.
    //
    // Для **CAN / ISO15765** первые 4 байта `data` — big-endian timestamp (μs),
    // далее идёт user-payload (см. dschultzca/j2534 `parse_ts`).
    //
    // Для **K-line (ISO9141 / ISO14230)** Tactrix-firmware timestamp НЕ
    // вставляет — payload идёт сразу после kind. Соответственно payload-size
    // = len - 1, и для SSM2 первые байты payload — это сам SSM-фрейм с
    // заголовком (`80 F0 10 <len> ...`).
    if raw.len() < 4 {
        return Err(ParseError::NeedMoreData);
    }
    let len = raw[3] as usize;
    if len < 1 {
        return Err(ParseError::InvalidLen);
    }
    let total = 4 + len;
    if raw.len() < total {
        return Err(ParseError::NeedMoreData);
    }
    let kind = PacketKind::from_byte(raw[4]);

    let is_can = matches!(proto_byte, PROTO_CAN | PROTO_ISO15765);
    let (timestamp_us, payload) = if is_can {
        if len < 5 {
            return Err(ParseError::InvalidLen);
        }
        let ts = u32::from_be_bytes([raw[5], raw[6], raw[7], raw[8]]);
        (ts, raw[9..total].to_vec())
    } else {
        (0u32, raw[5..total].to_vec())
    };

    Ok((total, TactrixFrame::Data {
        protocol_byte: proto_byte,
        kind,
        timestamp_us,
        payload,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_ack() {
        let raw = b"aro\r\n";
        let (consumed, frame) = parse_frame(raw).unwrap();
        assert_eq!(consumed, 5);
        assert_eq!(frame, TactrixFrame::Ack { channel: None });
    }

    #[test]
    fn parses_ack_with_channel() {
        let raw = b"aro3\r\n";
        let (consumed, frame) = parse_frame(raw).unwrap();
        assert_eq!(consumed, 6);
        assert_eq!(frame, TactrixFrame::Ack { channel: Some(3) });
    }

    #[test]
    fn parses_identify() {
        let raw = b"ari OpenPort 2.0 v1.30\r\n";
        let (consumed, frame) = parse_frame(raw).unwrap();
        assert_eq!(consumed, raw.len());
        assert_eq!(frame, TactrixFrame::Identify { info: "OpenPort 2.0 v1.30".into() });
    }

    #[test]
    fn parses_k_line_norm_msg_without_timestamp() {
        // K-line (ISO9141/14230): payload идёт сразу после kind, без timestamp.
        // len=6 → kind(1) + payload(5).
        let raw = &[
            b'a', b'r', 0x33, 0x06, // header + len=6
            0x00,                    // kind = NORM_MSG
            0x80, 0xF0, 0x10, 0x01, 0xFF, // SSM payload (5 bytes)
        ];
        let (consumed, frame) = parse_frame(raw).unwrap();
        assert_eq!(consumed, raw.len());
        match frame {
            TactrixFrame::Data { protocol_byte, kind, timestamp_us, payload } => {
                assert_eq!(protocol_byte, 0x33);
                assert_eq!(kind,          PacketKind::Normal);
                assert_eq!(timestamp_us,  0); // нет timestamp в K-line
                assert_eq!(payload,       vec![0x80, 0xF0, 0x10, 0x01, 0xFF]);
            }
            other => panic!("expected Data, got {other:?}"),
        }
    }

    #[test]
    fn parses_can_norm_msg_with_timestamp() {
        // CAN/ISO15765: первые 4 байта после kind — timestamp.
        let raw = &[
            b'a', b'r', 0x35, 0x0A, // header + len=10
            0x00,                    // kind = NORM_MSG
            0x00, 0x01, 0x02, 0x03,  // ts BE
            0x80, 0xF0, 0x10, 0x01, 0xFF, // payload (5 bytes)
        ];
        let (consumed, frame) = parse_frame(raw).unwrap();
        assert_eq!(consumed, raw.len());
        match frame {
            TactrixFrame::Data { protocol_byte, timestamp_us, payload, .. } => {
                assert_eq!(protocol_byte, 0x35);
                assert_eq!(timestamp_us,  0x00010203);
                assert_eq!(payload,       vec![0x80, 0xF0, 0x10, 0x01, 0xFF]);
            }
            other => panic!("expected Data, got {other:?}"),
        }
    }

    #[test]
    fn parses_k_line_rx_end_no_payload() {
        // K-line RX end: len=1, только kind, никакого ts/payload.
        let raw = &[b'a', b'r', 0x33, 0x01, 0x40];
        let (consumed, frame) = parse_frame(raw).unwrap();
        assert_eq!(consumed, raw.len());
        match frame {
            TactrixFrame::Data { kind: PacketKind::RxEnd, payload, .. } => {
                assert!(payload.is_empty());
            }
            other => panic!("expected RxEnd, got {other:?}"),
        }
    }

    #[test]
    fn need_more_data_when_incomplete() {
        assert_eq!(parse_frame(b"ar").unwrap_err(), ParseError::NeedMoreData);
        assert_eq!(parse_frame(b"aro").unwrap_err(), ParseError::NeedMoreData);
        assert_eq!(parse_frame(b"aro\r").unwrap_err(), ParseError::NeedMoreData);
        // Бинарный — len говорит 10, дали 8
        let partial = &[b'a', b'r', 0x34, 0x0A, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(parse_frame(partial).unwrap_err(), ParseError::NeedMoreData);
    }

    #[test]
    fn bad_start_marker() {
        assert_eq!(parse_frame(b"xx\r\n").unwrap_err(), ParseError::BadStartMarker);
    }

    #[test]
    fn unknown_discriminator() {
        assert_eq!(
            parse_frame(b"arX\r\n").unwrap_err(),
            ParseError::UnknownDiscriminator(b'X'),
        );
    }

    #[test]
    fn parses_error_frame() {
        let raw = b"are\r\n";
        let (consumed, frame) = parse_frame(raw).unwrap();
        assert_eq!(consumed, 5);
        assert_eq!(frame, TactrixFrame::Error { info: String::new() });
    }

    #[test]
    fn parses_error_with_info() {
        let raw = b"are bad command\r\n";
        let (consumed, frame) = parse_frame(raw).unwrap();
        assert_eq!(consumed, raw.len());
        assert_eq!(frame, TactrixFrame::Error { info: "bad command".into() });
    }

    #[test]
    fn invalid_len_too_short() {
        // len=0 запрещено (минимум 1 байт kind).
        let raw = &[b'a', b'r', 0x34, 0x00];
        assert_eq!(parse_frame(raw).unwrap_err(), ParseError::InvalidLen);
    }

    #[test]
    fn can_requires_len_at_least_5() {
        // CAN/ISO15765: len < 5 невалиден, т.к. ts занимает 4 байта.
        let raw = &[b'a', b'r', 0x35, 0x02, 0x00, 0xAA];
        assert_eq!(parse_frame(raw).unwrap_err(), ParseError::InvalidLen);
    }

    #[test]
    fn multiple_frames_back_to_back() {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"aro\r\n");
        // K-line NORM_MSG: len=2 → kind(1) + payload(1)
        raw.extend_from_slice(&[b'a', b'r', 0x34, 0x02, 0x00, 0xDE]);
        let (n1, f1) = parse_frame(&raw).unwrap();
        assert_eq!(f1, TactrixFrame::Ack { channel: None });
        let (n2, f2) = parse_frame(&raw[n1..]).unwrap();
        assert!(matches!(f2, TactrixFrame::Data { .. }));
        assert_eq!(n1 + n2, raw.len());
    }

    #[test]
    fn pkt_kind_from_byte_matches_known_constants() {
        assert_eq!(PacketKind::from_byte(0x00), PacketKind::Normal);
        assert_eq!(PacketKind::from_byte(0x40), PacketKind::RxEnd);
        assert_eq!(PacketKind::from_byte(0x10), PacketKind::TxDone);
        assert_eq!(PacketKind::from_byte(0x77), PacketKind::Other(0x77));
    }
}
