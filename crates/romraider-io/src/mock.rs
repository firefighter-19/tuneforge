//! In-memory `Transport` for tests and CLI smoke-runs.
//!
//! Поведение: запись складывается в [`MockTransport::writes`], а при чтении
//! отдаётся следующий заранее поставленный кадр из FIFO-очереди ответов.
//! Если очередь пуста — `read_frame` возвращает [`IoError::ReadTimeout`],
//! что воспроизводит поведение реального транспорта при «тишине на проводе».

use std::collections::VecDeque;
use std::time::Duration;

use crate::error::{IoError, IoResult};
use crate::transport::Transport;

#[derive(Debug, Default)]
pub struct MockTransport {
    desc: String,
    written: Vec<Vec<u8>>,
    pending: VecDeque<Vec<u8>>,
}

impl MockTransport {
    #[must_use]
    pub fn new() -> Self {
        Self {
            desc: "mock".to_string(),
            written: Vec::new(),
            pending: VecDeque::new(),
        }
    }

    /// Создать мок, в котором для каждой будущей операции `read_frame` уже
    /// заготовлен ответ (в порядке очереди).
    #[must_use]
    pub fn with_responses<I, B>(responses: I) -> Self
    where
        I: IntoIterator<Item = B>,
        B: Into<Vec<u8>>,
    {
        Self {
            desc: "mock".to_string(),
            written: Vec::new(),
            pending: responses.into_iter().map(Into::into).collect(),
        }
    }

    pub fn enqueue_response(&mut self, response: impl Into<Vec<u8>>) {
        self.pending.push_back(response.into());
    }

    /// Все кадры, что были записаны в этот транспорт (в порядке записи).
    #[must_use]
    pub fn writes(&self) -> &[Vec<u8>] {
        &self.written
    }

    /// Последний записанный кадр (удобно для одношаговых тестов).
    #[must_use]
    pub fn last_write(&self) -> Option<&[u8]> {
        self.written.last().map(Vec::as_slice)
    }

    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

impl Transport for MockTransport {
    fn write_all(&mut self, data: &[u8], _timeout: Duration) -> IoResult<()> {
        self.written.push(data.to_vec());
        Ok(())
    }

    fn read_frame(&mut self, buf: &mut [u8], timeout: Duration) -> IoResult<usize> {
        let mut response = self
            .pending
            .pop_front()
            .ok_or(IoError::ReadTimeout(timeout))?;
        let n = response.len().min(buf.len());
        buf[..n].copy_from_slice(&response[..n]);
        // Если в `response` остались байты — кладём остаток обратно в начало
        // очереди, чтобы следующий `read_frame` подобрал их. Это имитирует
        // поведение реального serial-порта, который доставляет кадр по
        // частям из ядра.
        if response.len() > n {
            let remainder = response.split_off(n);
            self.pending.push_front(remainder);
        }
        Ok(n)
    }

    fn purge(&mut self) -> IoResult<()> {
        self.pending.clear();
        Ok(())
    }

    fn description(&self) -> &str {
        &self.desc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_writes_and_replays_responses_in_order() {
        let mut t = MockTransport::with_responses([vec![0xAA, 0xBB], vec![0xCC]]);
        t.write_all(&[0x01, 0x02], Duration::from_millis(10))
            .unwrap();

        let mut buf = [0u8; 4];
        let n = t.read_frame(&mut buf, Duration::from_millis(10)).unwrap();
        assert_eq!(&buf[..n], &[0xAA, 0xBB]);
        let n = t.read_frame(&mut buf, Duration::from_millis(10)).unwrap();
        assert_eq!(&buf[..n], &[0xCC]);

        assert_eq!(t.last_write(), Some(&[0x01u8, 0x02][..]));
    }

    #[test]
    fn empty_queue_returns_timeout() {
        let mut t = MockTransport::new();
        let mut buf = [0u8; 4];
        let err = t
            .read_frame(&mut buf, Duration::from_millis(5))
            .unwrap_err();
        assert!(matches!(err, IoError::ReadTimeout(_)));
    }
}
