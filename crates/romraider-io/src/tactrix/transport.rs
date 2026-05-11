//! `Transport`-обёртка над USB-bulk каналом к Tactrix Openport 2.0.

use std::time::{Duration, Instant};

use rusb::{Device, DeviceHandle, GlobalContext, UsbContext};
use tracing::{debug, warn};

use crate::error::{IoError, IoResult};
use crate::transport::Transport;

use super::protocol::{parse_frame, PacketKind, ParseError, TactrixFrame, PROTO_ISO14230};

/// Стандартный VID Tactrix (унаследован от FTDI).
pub const TACTRIX_VID: u16 = 0x0403;
/// PID Openport 2.0. У других ревизий PID отличается (`0xCC48/49/4A` у OP 1.x).
pub const TACTRIX_OP2_PID: u16 = 0xCC4D;

#[derive(Debug, Clone)]
pub struct TactrixConfig {
    pub vid:           u16,
    pub pid:           u16,
    /// Протокол для `ato`: `0x33` ISO9141, `0x34` ISO14230, `0x35` CAN, `0x36` ISO15765.
    pub protocol:      u8,
    pub baud:          u32,
    /// Таймаут на handshake-команды при `open`.
    pub claim_timeout: Duration,
}

impl Default for TactrixConfig {
    fn default() -> Self {
        Self {
            vid:           TACTRIX_VID,
            pid:           TACTRIX_OP2_PID,
            protocol:      PROTO_ISO14230, // SSM2 K-Line
            baud:          4800,
            claim_timeout: Duration::from_secs(2),
        }
    }
}

pub struct TactrixTransport {
    handle:  DeviceHandle<GlobalContext>,
    ep_in:   u8,
    ep_out:  u8,
    channel: u8,
    desc:    String,
    /// Буфер сырых USB-байт между bulk-read-вызовами (фрейм может прийти кусками).
    rx_buf:  Vec<u8>,
}

impl TactrixTransport {
    /// Найти Openport 2.0, claim-нуть interface, прогнать `ati`/`ata`/`ato`.
    ///
    /// На macOS дополнительной настройки не требуется (libusb через IOKit).
    /// На Linux может понадобиться udev-rule для прав, либо запуск из-под root.
    pub fn open(cfg: &TactrixConfig) -> IoResult<Self> {
        let handle = rusb::open_device_with_vid_pid(cfg.vid, cfg.pid)
            .ok_or(IoError::TactrixNotFound { vid: cfg.vid, pid: cfg.pid })?;

        // На Linux libusb может конкурировать за устройство с kernel-driver-ом.
        #[cfg(target_os = "linux")]
        {
            let _ = handle.set_auto_detach_kernel_driver(true);
        }

        let (ep_in, ep_out) = find_bulk_endpoints(&handle.device())?;

        // claim_interface ДОЛЖЕН быть после поиска эндпоинтов (на некоторых
        // платформах descriptor доступен только после claim — пробуем оба порядка).
        let mut handle = handle;
        handle.claim_interface(0).map_err(usb_err)?;

        let mut t = Self {
            handle,
            ep_in,
            ep_out,
            channel: 0,
            desc:    format!("Tactrix Openport (VID={:#06X} PID={:#06X})", cfg.vid, cfg.pid),
            rx_buf:  Vec::with_capacity(2048),
        };

        // Идентификация — устройство отвечает "ari <firmware>\r\n".
        t.send(b"\r\n\r\nati\r\n", cfg.claim_timeout)?;
        match t.read_one_frame(cfg.claim_timeout)? {
            TactrixFrame::Identify { info } => debug!(%info, "Tactrix identify"),
            other => {
                return Err(IoError::TactrixUnexpected(format!(
                    "expected identify, got {other:?}"
                )))
            }
        }

        // Attach.
        t.send(b"ata\r\n", cfg.claim_timeout)?;
        let _ = t.expect_ack(cfg.claim_timeout)?;

        // Open channel: `ato<protocol-decimal> 0 <baud> 0`.
        let cmd = format!("ato{} 0 {} 0\r\n", cfg.protocol, cfg.baud);
        t.send(cmd.as_bytes(), cfg.claim_timeout)?;
        if let Some(ch) = t.expect_ack(cfg.claim_timeout)? {
            t.channel = ch;
        }

        Ok(t)
    }

    fn send(&self, bytes: &[u8], timeout: Duration) -> IoResult<()> {
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
                self.rx_buf.extend_from_slice(&tmp[..n]);
            }
        }
    }

    fn expect_ack(&mut self, timeout: Duration) -> IoResult<Option<u8>> {
        match self.read_one_frame(timeout)? {
            TactrixFrame::Ack { channel } => Ok(channel),
            other => Err(IoError::TactrixUnexpected(format!("expected ack, got {other:?}"))),
        }
    }
}

impl Transport for TactrixTransport {
    /// Отправить «сырые» SSM-байты — обёртываем в `att<ch> <len> 0\r\n<data>`.
    fn write_all(&mut self, data: &[u8], timeout: Duration) -> IoResult<()> {
        let header = format!("att{} {} 0\r\n", self.channel, data.len());
        let mut frame = Vec::with_capacity(header.len() + data.len());
        frame.extend_from_slice(header.as_bytes());
        frame.extend_from_slice(data);
        self.send(&frame, timeout)
    }

    /// Прочитать «сырые» SSM-байты — игнорируем все служебные фреймы (TxDone,
    /// RxEnd, и т.п.), пока не получим первый `NORM_MSG`.
    fn read_frame(&mut self, buf: &mut [u8], timeout: Duration) -> IoResult<usize> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(IoError::ReadTimeout(timeout));
            }
            match self.read_one_frame(remaining)? {
                TactrixFrame::Data { kind: PacketKind::Normal, payload, .. } => {
                    let n = payload.len().min(buf.len());
                    buf[..n].copy_from_slice(&payload[..n]);
                    return Ok(n);
                }
                TactrixFrame::Data { kind, .. } => {
                    debug!(?kind, "discarding non-Normal data frame");
                }
                TactrixFrame::Ack { .. } => {
                    debug!("stray ack while reading data");
                }
                TactrixFrame::Identify { .. } => {
                    debug!("stray identify while reading data");
                }
            }
        }
    }

    fn purge(&mut self) -> IoResult<()> {
        self.rx_buf.clear();
        let mut tmp = [0u8; 512];
        // Сливаем всё, что висит в kernel-buf.
        loop {
            match self.handle.read_bulk(self.ep_in, &mut tmp, Duration::from_millis(20)) {
                Ok(0) | Err(_) => break,
                Ok(_) => continue,
            }
        }
        Ok(())
    }

    fn description(&self) -> &str {
        &self.desc
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
        if let Err(e) = self.handle.release_interface(0) {
            warn!(?e, "tactrix release_interface failed");
        }
    }
}

fn find_bulk_endpoints(device: &Device<GlobalContext>) -> IoResult<(u8, u8)> {
    let cfg = device.active_config_descriptor().map_err(usb_err)?;
    for iface in cfg.interfaces() {
        for descr in iface.descriptors() {
            let mut in_ep  = None;
            let mut out_ep = None;
            for ep in descr.endpoint_descriptors() {
                if ep.transfer_type() == rusb::TransferType::Bulk {
                    match ep.direction() {
                        rusb::Direction::In  => in_ep  = Some(ep.address()),
                        rusb::Direction::Out => out_ep = Some(ep.address()),
                    }
                }
            }
            if let (Some(i), Some(o)) = (in_ep, out_ep) {
                return Ok((i, o));
            }
        }
    }
    Err(IoError::TactrixNoBulkEndpoints)
}

fn usb_err(e: rusb::Error) -> IoError {
    IoError::TactrixUsb(e.to_string())
}
