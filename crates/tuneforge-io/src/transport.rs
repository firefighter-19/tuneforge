use std::time::Duration;

use crate::error::{IoError, IoResult};

/// Двунаправленный байтовый канал к ECU или к посреднику (J2534/ELM).
///
/// Реализации обязаны быть **полудуплексными**: записать запрос,
/// потом прочитать ответ. Параллельных вызовов не предполагается.
pub trait Transport: Send {
    /// Отправить полный буфер (с учётом таймаута).
    fn write_all(&mut self, data: &[u8], timeout: Duration) -> IoResult<()>;

    /// Прочитать ответ. Реализация сама решает, как разделять кадры —
    /// например, ELM327 читает до `>` , SSM до контрольной суммы.
    fn read_frame(&mut self, buf: &mut [u8], timeout: Duration) -> IoResult<usize>;

    /// Сбросить буферы транспорта (после ошибки или таймаута).
    fn purge(&mut self) -> IoResult<()>;

    /// Чем-то описанный человеку идентификатор канала (для логов и UI).
    fn description(&self) -> &str;

    /// Переключить wire-baud-rate (для kernel-upload-сценария — нужно
    /// после programming session и kernel handshake, чтобы ускорить дамп
    /// в 10-13 раз). По умолчанию операция **не поддерживается**:
    /// transport не умеет — caller должен решать что делать (например,
    /// продолжить на текущем baud-е).
    ///
    /// Реализации (Tactrix, Serial) override-ят и физически перенастраивают
    /// SCI / FTDI. После успешного вызова все будущие read/write идут
    /// на новом baud-е.
    fn set_baud(&mut self, _new_baud: u32) -> IoResult<()> {
        Err(IoError::UnsupportedOperation("set_baud"))
    }
}
