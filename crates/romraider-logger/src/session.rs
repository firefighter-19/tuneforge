use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::datalog::DatalogWriter;
use crate::error::LoggerResult;
use crate::sample::Sample;

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub poll_interval: Duration,
    pub timeout:       Duration,
    pub datalog_dir:   Option<PathBuf>,
    pub channel_capacity: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(15),  // ~66 Hz, как у Java-логгера на SSM
            timeout:       Duration::from_millis(500),
            datalog_dir:   None,
            channel_capacity: 1024,
        }
    }
}

/// Управляет циклом опроса. Подписчики (GUI, datalog) получают семплы по
/// broadcast-каналу и не блокируют опрос: если потребитель отстаёт,
/// он получает `Lagged` от tokio и должен сам решить, переподписаться или нет.
pub struct LoggerSession {
    cfg: SessionConfig,
    tx:  broadcast::Sender<Sample>,
    datalog: Option<DatalogWriter>,
}

impl LoggerSession {
    #[must_use]
    pub fn new(cfg: SessionConfig) -> Self {
        let (tx, _rx) = broadcast::channel(cfg.channel_capacity);
        Self { cfg, tx, datalog: None }
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Sample> {
        self.tx.subscribe()
    }

    pub fn enable_datalog(&mut self, file_name: &str) -> LoggerResult<()> {
        let Some(dir) = self.cfg.datalog_dir.clone() else {
            warn!("datalog dir not configured");
            return Ok(());
        };
        std::fs::create_dir_all(&dir)?;
        self.datalog = Some(DatalogWriter::create(dir.join(file_name))?);
        info!(?self.datalog, "datalog enabled");
        Ok(())
    }

    /// Главный цикл. Заглушка: пока без реальных запросов в `Transport` — это
    /// прилетит, когда будет готов протокольный фасад над списком параметров.
    pub async fn run(&mut self) -> LoggerResult<()> {
        let mut tick = tokio::time::interval(self.cfg.poll_interval);
        loop {
            tick.tick().await;
            // TODO: build SSM request from active subscriptions, send, parse.
            // Сейчас цикл вырождается в ожидание — точка расширения.
        }
    }
}
