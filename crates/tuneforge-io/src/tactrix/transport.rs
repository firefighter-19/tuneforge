//! `Transport`-обёртка над USB-bulk каналом к Tactrix Openport 2.0.

use std::time::{Duration, Instant};

use rusb::{Device, DeviceHandle, GlobalContext};
use tracing::{debug, warn};

use crate::error::{IoError, IoResult};
use crate::transport::Transport;

use super::protocol::{
    parse_frame, PacketKind, ParseError, TactrixFrame, PROTO_CAN, PROTO_ISO15765, PROTO_ISO9141,
};

/// Стандартный VID Tactrix (унаследован от FTDI).
pub const TACTRIX_VID: u16 = 0x0403;
/// PID Openport 2.0. У других ревизий PID отличается (`0xCC48/49/4A` у OP 1.x).
pub const TACTRIX_OP2_PID: u16 = 0xCC4D;

#[derive(Debug, Clone)]
pub struct TactrixConfig {
    pub vid: u16,
    pub pid: u16,
    /// Протокол для `ato`: `0x33` ISO9141, `0x34` ISO14230, `0x35` CAN, `0x36` ISO15765.
    pub protocol: u8,
    /// Connect flags для `ato` (J2534 PassThruConnect flags). Для SSM K-line —
    /// `ISO9141_NO_CHECKSUM = 0x200`, т.к. SSM-фрейм сам содержит modular-checksum
    /// в последнем байте и не должен «дважды чекcумиться» драйвером Tactrix.
    pub flags: u32,
    pub baud: u32,
    /// Таймаут на handshake-команды при `open`.
    pub claim_timeout: Duration,
}

impl Default for TactrixConfig {
    fn default() -> Self {
        Self {
            vid: TACTRIX_VID,
            pid: TACTRIX_OP2_PID,
            // RomRaider Java также подключается к Subaru SSM как ISO9141 + NO_CHECKSUM:
            // J2534ConnectionISO9141.java → `api.connect(deviceId,
            // Flag.ISO9141_NO_CHECKSUM.getValue(), connectionProperties.getBaudRate())`.
            // ISO14230 (KWP-2000) применяет жёсткий handshake/timing, который SSM
            // не использует — устройство «слышит» начало RX но не получает payload.
            protocol: PROTO_ISO9141,
            flags: 0x0000_0200, // ISO9141_NO_CHECKSUM
            baud: 4800,
            claim_timeout: Duration::from_secs(2),
        }
    }
}

impl TactrixConfig {
    /// Preset для **CAN / ISO15765**-сессии с современными Subaru (Forester XT '07+
    /// и т.п.). 500 kbps стандартная automotive CAN-bus. flags=0 (sane defaults).
    /// Tactrix сам управляет ISO-TP framing'ом (First Frame, Consecutive Frame,
    /// Flow Control) когда установлен PASS-FILTER типа `FLOW_CONTROL_FILTER`
    /// (см. [`TactrixTransport::set_can_flow_control_filter`]).
    #[must_use]
    pub fn iso15765_500k() -> Self {
        Self {
            vid: TACTRIX_VID,
            pid: TACTRIX_OP2_PID,
            protocol: PROTO_ISO15765,
            flags: 0, // proprietary `1749696844` от EcuFlash — Tactrix-session-nonce,
            // dschultzca/j2534 use 0; mы тоже.
            baud: 500_000,
            claim_timeout: Duration::from_secs(2),
        }
    }
}

pub struct TactrixTransport {
    handle: DeviceHandle<GlobalContext>,
    ep_in: u8,
    ep_out: u8,
    channel: u8,
    desc: String,
    /// Буфер сырых USB-байт между bulk-read-вызовами (фрейм может прийти кусками).
    rx_buf: Vec<u8>,
    /// Текущая конфигурация (protocol, flags, baud, timeout). Хранится для
    /// [`Self::reset_channel`] и [`Self::set_baud`] — оба пересоздают K-Line
    /// channel и нуждаются в актуальных protocol/flags.
    cfg: TactrixConfig,
}

impl TactrixTransport {
    /// Найти Openport 2.0, claim-нуть interface, прогнать `ati`/`ata`/`ato`.
    ///
    /// На macOS дополнительной настройки не требуется (libusb через IOKit).
    /// На Linux может понадобиться udev-rule для прав, либо запуск из-под root.
    pub fn open(cfg: &TactrixConfig) -> IoResult<Self> {
        let handle =
            rusb::open_device_with_vid_pid(cfg.vid, cfg.pid).ok_or(IoError::TactrixNotFound {
                vid: cfg.vid,
                pid: cfg.pid,
            })?;

        // libusb может конкурировать за устройство с kernel-driver-ом —
        // на macOS Apple-class-driver моментально захватывает интерфейс,
        // на Linux то же делает `ftdi_sio` для FTDI-PID. Best-effort: если
        // платформа не поддерживает — вызов вернёт ошибку, проигнорируем.
        if let Err(e) = handle.set_auto_detach_kernel_driver(true) {
            tracing::debug!(?e, "set_auto_detach_kernel_driver unsupported (ignored)");
        }

        // На macOS bulk-transfer-ы возвращают ENTNOTFOUND, если active
        // configuration не активирована. Спецификация USB говорит, что после
        // enumeration конфигурация должна быть выбрана, но macOS оставляет
        // это на хост-приложение в некоторых случаях.
        if let Err(e) = handle.set_active_configuration(1) {
            tracing::debug!(
                ?e,
                "set_active_configuration(1) — может быть уже active, игнорируем"
            );
        } else {
            tracing::debug!("set_active_configuration(1) ok");
        }

        // Openport 2.0 = CDC ACM: iface 0 = CDC Communications (control +
        // interrupt EP), iface 1 = CDC Data (bulk EP). Bulk transfers идут
        // на iface 1, поэтому claim его. Detach обоих kernel-driver-ов чтобы
        // AppleUSBCDC не мешал.
        let _ = handle.detach_kernel_driver(0);
        let _ = handle.detach_kernel_driver(1);
        if let Err(e) = handle.claim_interface(1) {
            tracing::warn!(?e, "claim_interface(1) failed");
            return Err(usb_err(e));
        }
        tracing::debug!("interface 1 (CDC Data) claimed");

        if let Err(e) = handle.set_alternate_setting(1, 0) {
            tracing::debug!(?e, "set_alternate_setting(1,0) ignored");
        }

        let (ep_in, ep_out) = find_bulk_endpoints(&handle.device())?;
        tracing::debug!(
            ep_in = format_args!("{ep_in:#04X}"),
            ep_out = format_args!("{ep_out:#04X}"),
            "found bulk endpoints",
        );

        let mut t = Self {
            handle,
            ep_in,
            ep_out,
            channel: 0,
            desc: format!(
                "Tactrix Openport (VID={:#06X} PID={:#06X})",
                cfg.vid, cfg.pid
            ),
            rx_buf: Vec::with_capacity(2048),
            cfg: cfg.clone(),
        };

        // Идентификация. Tactrix может вернуть несколько фреймов (ari + are),
        // поэтому ждём ИМЕННО Identify, остальное логируем.
        tracing::debug!("sending ati");
        t.send(b"ati\r\n", cfg.claim_timeout)?;
        let info = t.wait_for_identify(cfg.claim_timeout)?;
        debug!(%info, "Tactrix identify");

        // Attach.
        tracing::debug!("sending ata");
        t.send(b"ata\r\n", cfg.claim_timeout)?;
        let _ = t.wait_for_ack(cfg.claim_timeout)?;

        // Open channel: `ato<j2534-protocol-id> 0 <baud> 0`.
        // ВАЖНО: `cfg.protocol` — это байт-маркер бинарных кадров (0x33..0x36 = ASCII '3'..'6'),
        // а в `ato` идёт ИМЕННО голая цифра J2534 protocol ID (3..6). Это одно и то же число,
        // просто записанное по-разному: 0x34 == ASCII '4' → команда "ato4".
        let proto_id = cfg.protocol.wrapping_sub(b'0');
        let cmd = format!("ato{} {} {} 0\r\n", proto_id, cfg.flags, cfg.baud);
        tracing::debug!(cmd = %cmd.trim_end(), "sending ato");
        t.send(cmd.as_bytes(), cfg.claim_timeout)?;
        let _ = t.wait_for_ack(cfg.claim_timeout)?;
        // В Tactrix-wire-протоколе нет отдельного channel-id — в качестве
        // идентификатора канала используется сам protocol-id (см. dschultzca/j2534
        // `*pChannelID = protocolID;`). `aro` без digit — нормальный ack.
        t.channel = proto_id;
        tracing::debug!(
            channel = t.channel,
            "Tactrix channel opened (channel == protocol_id)"
        );

        // Фильтр для K-line: PASS-all (mask=00, pattern=00). Без фильтра Tactrix
        // молча выкидывает входящие пакеты. Для **CAN/ISO15765** PASS-all НЕ
        // подходит — нужен `FLOW_CONTROL_FILTER` со специфическими CAN-ID-ами;
        // вызывается отдельно через [`Self::set_can_flow_control_filter`] после
        // `open()`.
        let is_can = matches!(cfg.protocol, PROTO_CAN | PROTO_ISO15765);
        if !is_can {
            let mut atf = Vec::with_capacity(16);
            atf.extend_from_slice(format!("atf{} 1 0 1\r\n", t.channel).as_bytes());
            atf.extend_from_slice(&[0x00, 0x00]);
            tracing::debug!("sending atf (PASS-all filter, K-line)");
            t.send(&atf, cfg.claim_timeout)?;
            let filter_id = t.wait_for_filter_ack(cfg.claim_timeout)?;
            tracing::debug!(filter_id, "K-line filter installed");
        } else {
            tracing::debug!(
                "CAN-mode: skipping K-line PASS-filter; call set_can_flow_control_filter() next"
            );
        }

        Ok(t)
    }

    /// Установить `FLOW_CONTROL_FILTER` для ISO15765/CAN-сессии.
    ///
    /// J2534 требует один Flow Control filter на каждый ECU-target с которым
    /// общается ISO-TP. Subaru OBD-II ECU отвечает по `0x7E8`, мы шлём по `0x7E0`
    /// (стандартная 11-bit functional addressing).
    ///
    /// Wire format: `atf<ch> 3 64 4\r\n <MASK_4B> <PATTERN_4B> <FLOWCTRL_4B>`
    /// где `3` = FLOW_CONTROL_FILTER, `64` = `TxFlags=ISO15765_FRAME_PAD`, `4` = data_size.
    /// Mask/Pattern фильтруют incoming CAN frames; Flow Control задаёт TX CAN-ID для
    /// outgoing Flow Control frames (требуется ISO-TP spec).
    pub fn set_can_flow_control_filter(&mut self, rx_id: u32, tx_id: u32) -> IoResult<u32> {
        const FILTER_TYPE_FLOW_CONTROL: u8 = 3;
        const TX_FLAG_ISO15765_FRAME_PAD: u8 = 64;
        const ADDR_BYTES: usize = 4;

        let mut atf = Vec::with_capacity(40);
        atf.extend_from_slice(
            format!(
                "atf{} {FILTER_TYPE_FLOW_CONTROL} {TX_FLAG_ISO15765_FRAME_PAD} {ADDR_BYTES}\r\n",
                self.channel,
            )
            .as_bytes(),
        );
        // mask: 0xFF FF FF FF (match all 32 bits of CAN ID)
        atf.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        // pattern: rx_id (CAN ID we want to receive)
        atf.extend_from_slice(&rx_id.to_be_bytes());
        // flow control: tx_id (CAN ID we use to send Flow Control frames)
        atf.extend_from_slice(&tx_id.to_be_bytes());

        tracing::debug!(
            rx = format!("0x{rx_id:08X}"),
            tx = format!("0x{tx_id:08X}"),
            "sending atf (FLOW_CONTROL filter, CAN)",
        );
        self.send(&atf, self.cfg.claim_timeout)?;
        let filter_id = self.wait_for_filter_ack(self.cfg.claim_timeout)?;
        tracing::debug!(filter_id, "CAN flow-control filter installed");
        Ok(filter_id)
    }

    /// «Перецикловать» K-Line канал на той же USB-сессии: закрыть текущий
    /// channel (`atc`), снова открыть (`ato`) c **текущим `self.cfg`**,
    /// переустановить PASS-фильтр (`atf`). Полезно между запросами при
    /// анти-fuzz-защите Subaru SSM2 и для смены baud-rate (см. [`Self::set_baud`]).
    pub fn reset_channel(&mut self) -> IoResult<()> {
        let timeout = self.cfg.claim_timeout;
        // Старый канал — закрываем (без `atz`, чтобы USB-сессия осталась живой).
        let _ = self.send(format!("atc{}\r\n", self.channel).as_bytes(), timeout);
        // Опционально дать K-Line время "успокоиться".
        std::thread::sleep(Duration::from_millis(50));
        self.rx_buf.clear();

        let proto_id = self.cfg.protocol.wrapping_sub(b'0');
        let ato = format!("ato{} {} {} 0\r\n", proto_id, self.cfg.flags, self.cfg.baud);
        self.send(ato.as_bytes(), timeout)?;
        let _ = self.wait_for_ack(timeout)?;
        self.channel = proto_id;

        let mut atf = Vec::with_capacity(16);
        atf.extend_from_slice(format!("atf{} 1 0 1\r\n", self.channel).as_bytes());
        atf.extend_from_slice(&[0x00, 0x00]);
        self.send(&atf, timeout)?;
        let _ = self.wait_for_filter_ack(timeout)?;
        tracing::debug!(
            channel = self.channel,
            baud = self.cfg.baud,
            "Tactrix K-Line channel reset"
        );
        Ok(())
    }

    fn send(&self, bytes: &[u8], timeout: Duration) -> IoResult<()> {
        tracing::trace!(
            n = bytes.len(),
            hex = %hex_dump(bytes),
            ascii = %ascii_dump(bytes),
            "tactrix bulk write",
        );
        let mut written = 0;
        while written < bytes.len() {
            let n = self
                .handle
                .write_bulk(self.ep_out, &bytes[written..], timeout)
                .map_err(usb_err)?;
            if n == 0 {
                return Err(IoError::WriteTimeout(timeout));
            }
            written += n;
        }
        Ok(())
    }

    fn read_one_frame(&mut self, timeout: Duration) -> IoResult<TactrixFrame> {
        let deadline = Instant::now() + timeout;
        let mut tmp = [0u8; 512];
        loop {
            match parse_frame(&self.rx_buf) {
                Ok((consumed, frame)) => {
                    self.rx_buf.drain(..consumed);
                    return Ok(frame);
                }
                Err(ParseError::NeedMoreData) => {} // read more
                Err(e) => {
                    warn!(?e, "tactrix parse error; discarding 1 byte");
                    if self.rx_buf.is_empty() {
                        // нечего отбросить — придётся ждать новых байт
                    } else {
                        self.rx_buf.remove(0);
                        continue;
                    }
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(IoError::ReadTimeout(timeout));
            }
            let n = self
                .handle
                .read_bulk(self.ep_in, &mut tmp, remaining)
                .map_err(usb_err)?;
            if n > 0 {
                tracing::trace!(
                    n,
                    hex = %hex_dump(&tmp[..n]),
                    ascii = %ascii_dump(&tmp[..n]),
                    "tactrix bulk read",
                );
                self.rx_buf.extend_from_slice(&tmp[..n]);
            }
        }
    }

    /// Ждать `Ack`-фрейм. Если приходит `Error`-фрейм во время handshake —
    /// это значит, что Tactrix отверг команду; пробрасываем как ошибку.
    fn wait_for_ack(&mut self, timeout: Duration) -> IoResult<Option<u8>> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(IoError::ReadTimeout(timeout));
            }
            match self.read_one_frame(remaining)? {
                TactrixFrame::Ack { channel } => return Ok(channel),
                TactrixFrame::Error { info } => {
                    return Err(IoError::TactrixUnexpected(format!(
                        "Tactrix rejected command (are {info})"
                    )));
                }
                TactrixFrame::FilterAck { id } => {
                    tracing::warn!(id, "unexpected filter ack while waiting for ack");
                }
                other => {
                    tracing::warn!(?other, "unexpected frame while waiting for ack");
                }
            }
        }
    }

    /// Ждать `FilterAck`-фрейм (`arf<id>`).
    fn wait_for_filter_ack(&mut self, timeout: Duration) -> IoResult<u32> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(IoError::ReadTimeout(timeout));
            }
            match self.read_one_frame(remaining)? {
                TactrixFrame::FilterAck { id } => return Ok(id),
                TactrixFrame::Error { info } => {
                    return Err(IoError::TactrixUnexpected(format!(
                        "Tactrix rejected filter (are {info})"
                    )));
                }
                other => {
                    tracing::warn!(?other, "unexpected frame while waiting for filter ack");
                }
            }
        }
    }

    /// Ждать `Identify`-фрейм. Поведение на `Error` — как у `wait_for_ack`.
    fn wait_for_identify(&mut self, timeout: Duration) -> IoResult<String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(IoError::ReadTimeout(timeout));
            }
            match self.read_one_frame(remaining)? {
                TactrixFrame::Identify { info } => return Ok(info),
                TactrixFrame::Error { info } => {
                    return Err(IoError::TactrixUnexpected(format!(
                        "Tactrix rejected identify (are {info})"
                    )));
                }
                TactrixFrame::FilterAck { id } => {
                    tracing::warn!(id, "unexpected filter ack while waiting for identify");
                }
                other => {
                    tracing::warn!(?other, "unexpected frame while waiting for identify");
                }
            }
        }
    }
}

impl Transport for TactrixTransport {
    /// Отправить wire-байты, обёрнутые в Tactrix `att`-команду.
    ///
    /// **K-line (ISO9141/ISO14230)**: `att<ch> <len> 0\r\n<data>` — txflags=0,
    /// data это полный SSM/KWP-frame.
    ///
    /// **CAN (CAN/ISO15765)**: `att<ch> <len> 64\r\n<CAN_ID 4B BE><UDS data>` —
    /// txflags=64 (`ISO15765_FRAME_PAD`), data **должна начинаться с 4-байтного
    /// CAN-ID** (big-endian), за которым идёт UDS-payload. Tactrix сам ISO-TP-
    /// фреймит длинные сообщения (First Frame + Consecutive + Flow Control).
    fn write_all(&mut self, data: &[u8], timeout: Duration) -> IoResult<()> {
        let is_can = matches!(self.cfg.protocol, PROTO_CAN | PROTO_ISO15765);
        let tx_flags: u32 = if is_can { 64 } else { 0 };
        let header = format!("att{} {} {}\r\n", self.channel, data.len(), tx_flags);
        let mut frame = Vec::with_capacity(header.len() + data.len());
        frame.extend_from_slice(header.as_bytes());
        frame.extend_from_slice(data);
        self.send(&frame, timeout)
    }

    /// Прочитать одно SSM-сообщение целиком.
    ///
    /// Tactrix-firmware дробит длинный K-line-ответ на цепочку фреймов:
    /// `NORM_MSG_START_IND` → `NORM_MSG` (один или несколько чанков) →
    /// `RX_MSG_END_IND`. Нам нужно склеить все `NORM_MSG`-чанки до
    /// конца-индикации, чтобы получить полный SSM-фрейм (`80 F0 10 <len>
    /// <data> <chksum>`).
    fn read_frame(&mut self, buf: &mut [u8], timeout: Duration) -> IoResult<usize> {
        let deadline = Instant::now() + timeout;
        let mut acc: Vec<u8> = Vec::new();
        // Для **CAN/ISO15765** Tactrix дробит длинный response на несколько
        // `Normal`-кадров и каждый префиксует 4-байтным CAN-ID (без ISO-TP
        // sequence-номеров — Tactrix их strip-ает сам). Caller хочет получить
        // `<CAN_ID 4B><UDS bytes>` целиком — поэтому CAN_ID мы keep-аем только
        // на первом chunk-е, а у subsequent его **отрезаем** перед accumulation-ом.
        // На K-line эта логика не используется (там нет CAN_ID-префиксов).
        let is_can = matches!(self.cfg.protocol, PROTO_CAN | PROTO_ISO15765);
        let mut got_first_data = false;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                if !acc.is_empty() {
                    // Таймаут после частичной сборки — отдаём что есть, чтобы
                    // верхний слой мог хотя бы попробовать распарсить.
                    tracing::warn!(
                        bytes = acc.len(),
                        "read_frame timed out mid-message, returning partial"
                    );
                    break;
                }
                return Err(IoError::ReadTimeout(timeout));
            }
            match self.read_one_frame(remaining)? {
                TactrixFrame::Data {
                    kind: PacketKind::NormalStart,
                    ..
                } => {
                    tracing::trace!("RX message start");
                }
                TactrixFrame::Data {
                    kind: PacketKind::Normal,
                    payload,
                    ..
                } => {
                    let data: &[u8] = if is_can && got_first_data && payload.len() >= 4 {
                        &payload[4..]
                    } else {
                        &payload
                    };
                    acc.extend_from_slice(data);
                    got_first_data = true;
                    tracing::trace!(chunk = data.len(), total = acc.len(), "RX chunk");
                }
                TactrixFrame::Data {
                    kind: PacketKind::RxEnd | PacketKind::RxEndExtended,
                    payload,
                    ..
                } => {
                    // K-line: RxEnd обычно пустой → пропускаем.
                    // CAN single-frame: payload = `<CAN_ID 4B><UDS data>` целиком → keep.
                    // CAN multi-frame: RxEnd-payload — последний фрагмент с CAN_ID prefix →
                    // strip CAN_ID если уже накопили первый chunk.
                    if !payload.is_empty() {
                        let data: &[u8] = if is_can && got_first_data && payload.len() >= 4 {
                            &payload[4..]
                        } else {
                            &payload
                        };
                        acc.extend_from_slice(data);
                    }
                    tracing::trace!(total = acc.len(), "RX message end");
                    break;
                }
                TactrixFrame::Data {
                    kind: PacketKind::TxDone,
                    ..
                } => {
                    tracing::trace!("TX_DONE indication (CAN-mode), continuing read");
                }
                TactrixFrame::Data { kind, .. } => {
                    debug!(?kind, "discarding non-data frame mid-stream");
                }
                TactrixFrame::Ack { .. } => debug!("stray ack while reading data"),
                TactrixFrame::Identify { .. } => debug!("stray identify while reading data"),
                TactrixFrame::Error { info } => {
                    tracing::warn!(%info, "Tactrix error frame mid-stream; ignoring");
                }
                TactrixFrame::FilterAck { id } => {
                    debug!(id, "stray filter ack while reading data");
                }
            }
        }
        let n = acc.len().min(buf.len());
        buf[..n].copy_from_slice(&acc[..n]);
        Ok(n)
    }

    fn purge(&mut self) -> IoResult<()> {
        self.rx_buf.clear();
        let mut tmp = [0u8; 512];
        // Сливаем всё, что висит в kernel-buf.
        loop {
            match self
                .handle
                .read_bulk(self.ep_in, &mut tmp, Duration::from_millis(20))
            {
                Ok(0) | Err(_) => break,
                Ok(_) => continue,
            }
        }
        Ok(())
    }

    fn description(&self) -> &str {
        &self.desc
    }

    /// Переключить wire-baud-rate. Использует `atc`/`ato`/`atf` cycle через
    /// [`Self::reset_channel`] — USB сессия остаётся открытой, K-Line канал
    /// пересоздаётся с новым baud-rate. ECU должен быть готов общаться на
    /// этом baud-rate **в момент вызова** (это caller-side ответственность —
    /// сначала kernel-side `SID_CONF 0x01 <brrdiv>`, потом наш `set_baud`).
    fn set_baud(&mut self, new_baud: u32) -> IoResult<()> {
        let old_baud = self.cfg.baud;
        self.cfg.baud = new_baud;
        match self.reset_channel() {
            Ok(()) => {
                tracing::info!(old_baud, new_baud, "Tactrix wire-baud switched");
                Ok(())
            }
            Err(e) => {
                // Rollback config так чтобы повторный вызов не глюкнул.
                self.cfg.baud = old_baud;
                Err(e)
            }
        }
    }
}

impl Drop for TactrixTransport {
    fn drop(&mut self) {
        // Best-effort cleanup; ошибки логируем и идём дальше.
        let _ = self.send(
            format!("atc{}\r\n", self.channel).as_bytes(),
            Duration::from_millis(500),
        );
        let _ = self.send(b"atz\r\n", Duration::from_millis(500));
        if let Err(e) = self.handle.release_interface(1) {
            warn!(?e, "tactrix release_interface(1) failed");
        }
    }
}

fn find_bulk_endpoints(device: &Device<GlobalContext>) -> IoResult<(u8, u8)> {
    let cfg = device.active_config_descriptor().map_err(usb_err)?;
    tracing::debug!(
        cfg_value = cfg.number(),
        num_interfaces = cfg.num_interfaces(),
        max_power_ma = cfg.max_power(),
        "active config descriptor",
    );
    let mut found: Option<(u8, u8)> = None;
    for iface in cfg.interfaces() {
        for descr in iface.descriptors() {
            tracing::debug!(
                iface = descr.interface_number(),
                alt = descr.setting_number(),
                class = format_args!("{:#04X}", descr.class_code()),
                sub = format_args!("{:#04X}", descr.sub_class_code()),
                proto = format_args!("{:#04X}", descr.protocol_code()),
                num_eps = descr.num_endpoints(),
                "interface descriptor",
            );
            let mut in_ep = None;
            let mut out_ep = None;
            for ep in descr.endpoint_descriptors() {
                tracing::debug!(
                    addr  = format_args!("{:#04X}", ep.address()),
                    dir   = ?ep.direction(),
                    xfer  = ?ep.transfer_type(),
                    max_size = ep.max_packet_size(),
                    "  endpoint",
                );
                if ep.transfer_type() == rusb::TransferType::Bulk {
                    match ep.direction() {
                        rusb::Direction::In => in_ep = Some(ep.address()),
                        rusb::Direction::Out => out_ep = Some(ep.address()),
                    }
                }
            }
            // Берём только bulk endpoints с alt-setting 0 на data-интерфейсе
            // (iface 1 для Openport 2.0; control interface 0 у CDC имеет
            // только interrupt EP). Первый найденный bulk-пара побеждает.
            if descr.setting_number() == 0 && found.is_none() {
                if let (Some(i), Some(o)) = (in_ep, out_ep) {
                    found = Some((i, o));
                }
            }
        }
    }
    found.ok_or(IoError::TactrixNoBulkEndpoints)
}

fn usb_err(e: rusb::Error) -> IoError {
    IoError::TactrixUsb(e.to_string())
}

fn hex_dump(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 3);
    for (i, byte) in b.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

fn ascii_dump(b: &[u8]) -> String {
    b.iter()
        .map(|&c| {
            if (0x20..0x7f).contains(&c) {
                c as char
            } else {
                '.'
            }
        })
        .collect()
}
