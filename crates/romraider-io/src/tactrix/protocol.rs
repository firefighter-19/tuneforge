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
        b @ (PROTO_ISO9141 | PROTO_ISO14230 | PROTO_CAN | PROTO_ISO15765) => parse_binary(raw, b),
        other => Err(ParseError::UnknownDiscriminator(other)),
    }
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
    // 'a' 'r' <proto> <len> <type> <ts:4> <payload(len-5)>
    if raw.len() < 4 {
        return Err(ParseError::NeedMoreData);
    }
    let len = raw[3] as usize;
    if len < 5 {
        return Err(ParseError::InvalidLen);
    }
    let total = 4 + len;
    if raw.len() < total {
        return Err(ParseError::NeedMoreData);
    }
    let kind = PacketKind::from_byte(raw[4]);
    let ts = u32::from_be_bytes([raw[5], raw[6], raw[7], raw[8]]);
    let payload = raw[9..total].to_vec();
    Ok((total, TactrixFrame::Data {
        protocol_byte: proto_byte,
        kind,
        timestamp_us: ts,
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
    fn parses_binary_norm_msg() {
        // 'a' 'r' 0x34 (ISO14230) len=10 type=0x00 ts=0x00010203 payload= 80 F0 10 01 FF 40
        // len covers type(1)+ts(4)+payload(5) = 10
        let raw = &[
            b'a', b'r', 0x34, 0x0A, // header + len=10
            0x00,                    // pkt_type = Normal
            0x00, 0x01, 0x02, 0x03,  // ts BE
            0x80, 0xF0, 0x10, 0x01, 0xFF, // SSM payload (5 bytes)
        ];
        let (consumed, frame) = parse_frame(raw).unwrap();
        assert_eq!(consumed, raw.len());
        match frame {
            TactrixFrame::Data { protocol_byte, kind, timestamp_us, payload } => {
                assert_eq!(protocol_byte, 0x34);
                assert_eq!(kind,          PacketKind::Normal);
                assert_eq!(timestamp_us,  0x00010203);
                assert_eq!(payload,       vec![0x80, 0xF0, 0x10, 0x01, 0xFF]);
            }
            other => panic!("expected Data, got {other:?}"),
        }
    }

    #[test]
    fn parses_rx_end_indication() {
        // type=0x40, no payload
        let raw = &[
            b'a', b'r', 0x34, 0x05,    // header + len=5
            0x40,                       // RX end
            0x00, 0x00, 0x10, 0x00,     // ts
        ];
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
    fn invalid_len_too_short() {
        let raw = &[b'a', b'r', 0x34, 0x02, 0x00];
        assert_eq!(parse_frame(raw).unwrap_err(), ParseError::InvalidLen);
    }

    #[test]
    fn multiple_frames_back_to_back() {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"aro\r\n");
        raw.extend_from_slice(&[
            b'a', b'r', 0x34, 0x06, 0x00, 0x00, 0x00, 0x00, 0x01, 0xDE,
        ]);
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
