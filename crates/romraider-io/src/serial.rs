use std::io::{Read, Write};
use std::time::Duration;

use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};
use tracing::{debug, trace};

use crate::error::{IoError, IoResult};
use crate::transport::Transport;

/// Конфигурация последовательного порта.
///
/// Значения по умолчанию совпадают с теми, что использует Java-версия для SSM
/// (см. `com.romraider.io.connection.ConnectionProperties`).
#[derive(Debug, Clone)]
pub struct SerialConfig {
    pub path: String,
    pub baud_rate: u32,
    pub data_bits: DataBits,
    pub parity: Parity,
    pub stop_bits: StopBits,
    pub flow_control: FlowControl,
}

impl SerialConfig {
    #[must_use]
    pub fn ssm(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            baud_rate: 4800,
            data_bits: DataBits::Eight,
            parity: Parity::None,
            stop_bits: StopBits::One,
            flow_control: FlowControl::None,
        }
    }
}

pub struct SerialTransport {
    port: Box<dyn SerialPort>,
    desc: String,
}

impl SerialTransport {
    pub fn open(cfg: &SerialConfig) -> IoResult<Self> {
        debug!(?cfg, "opening serial port");
        let port = serialport::new(&cfg.path, cfg.baud_rate)
            .data_bits(cfg.data_bits)
            .parity(cfg.parity)
            .stop_bits(cfg.stop_bits)
            .flow_control(cfg.flow_control)
            .timeout(Duration::from_millis(50))
            .open()?;
        Ok(Self {
            desc: format!("serial:{}@{}", cfg.path, cfg.baud_rate),
            port,
        })
    }
}

impl Transport for SerialTransport {
    fn write_all(&mut self, data: &[u8], timeout: Duration) -> IoResult<()> {
        self.port.set_timeout(timeout)?;
        self.port.write_all(data)?;
        self.port.flush()?;
        trace!(bytes = data.len(), "wrote frame");
        Ok(())
    }

    fn read_frame(&mut self, buf: &mut [u8], timeout: Duration) -> IoResult<usize> {
        self.port.set_timeout(timeout)?;
        match self.port.read(buf) {
            Ok(0) => Err(IoError::ReadTimeout(timeout)),
            Ok(n) => Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                Err(IoError::ReadTimeout(timeout))
            }
            Err(e) => Err(e.into()),
        }
    }

    fn purge(&mut self) -> IoResult<()> {
        self.port.clear(serialport::ClearBuffer::All)?;
        Ok(())
    }

    fn description(&self) -> &str {
        &self.desc
    }
}
