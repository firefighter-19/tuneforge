use std::time::Duration;

use tracing::{debug, trace};

use crate::error::{IoError, IoResult};
use crate::serial::{SerialConfig, SerialTransport};
use crate::transport::Transport;

/// Тонкая обёртка над serial-каналом, говорящая на AT-командах ELM327.
///
/// Слой кадрирования: ELM327 завершает каждый ответ символом `>` (приглашение).
pub struct Elm327 {
    inner: SerialTransport,
}

impl Elm327 {
    pub fn open(cfg: &SerialConfig) -> IoResult<Self> {
        let inner = SerialTransport::open(cfg)?;
        Ok(Self { inner })
    }

    pub fn at(&mut self, cmd: &str, timeout: Duration) -> IoResult<String> {
        debug!(%cmd, "AT command");
        let mut line = String::with_capacity(cmd.len() + 2);
        line.push_str(cmd);
        line.push('\r');
        self.inner.write_all(line.as_bytes(), timeout)?;
        self.read_until_prompt(timeout)
    }

    fn read_until_prompt(&mut self, timeout: Duration) -> IoResult<String> {
        let mut acc = Vec::with_capacity(64);
        let mut tmp = [0u8; 64];
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(IoError::ReadTimeout(timeout));
            }
            let n = self.inner.read_frame(&mut tmp, remaining)?;
            acc.extend_from_slice(&tmp[..n]);
            if acc.last() == Some(&b'>') {
                acc.pop();
                trace!(bytes = acc.len(), "got prompt");
                return String::from_utf8(acc)
                    .map_err(|e| IoError::ElmUnexpectedResponse(format!("non-utf8: {e}")))
                    .map(|s| s.trim().to_string());
            }
        }
    }
}

impl Transport for Elm327 {
    fn write_all(&mut self, data: &[u8], timeout: Duration) -> IoResult<()> {
        self.inner.write_all(data, timeout)
    }

    fn read_frame(&mut self, buf: &mut [u8], timeout: Duration) -> IoResult<usize> {
        let response = self.read_until_prompt(timeout)?;
        let bytes = response.as_bytes();
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }

    fn purge(&mut self) -> IoResult<()> {
        self.inner.purge()
    }

    fn description(&self) -> &str {
        "elm327"
    }
}
