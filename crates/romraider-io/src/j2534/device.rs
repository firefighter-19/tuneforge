use std::sync::Arc;
use std::time::Duration;

use crate::error::{IoError, IoResult};
use crate::transport::Transport;

use super::api::{ChannelId, DeviceId};
use super::library::Library;

/// Высокоуровневая обёртка над одним устройством PassThru.
///
/// Скрывает unsafe-вызовы C-API и реализует [`Transport`], так что
/// устройство можно подавать в `romraider-protocol` без `cfg`-плясок.
///
/// TODO: подключить реальные channel/filters/ioctl при первом задействовании.
/// Сейчас это каркас, чтобы зафиксировать публичный контракт.
pub struct Device {
    lib: Arc<Library>,
    device_id: DeviceId,
    channel_id: Option<ChannelId>,
    desc: String,
}

impl Device {
    pub fn open(_lib: Arc<Library>) -> IoResult<Self> {
        // Реальный вызов: lib.vtable().PassThruOpen(name_ptr, &mut id)
        // Пока возвращаем-каркас — заполнить, когда появится тестовое железо.
        unimplemented!("PassThruOpen wiring — see docs/ARCHITECTURE.md §J2534");
    }
}

impl Transport for Device {
    fn write_all(&mut self, _data: &[u8], _timeout: Duration) -> IoResult<()> {
        let _ = self.channel_id.ok_or(IoError::NotConnected)?;
        unimplemented!("PassThruWriteMsgs wiring");
    }

    fn read_frame(&mut self, _buf: &mut [u8], _timeout: Duration) -> IoResult<usize> {
        let _ = self.channel_id.ok_or(IoError::NotConnected)?;
        unimplemented!("PassThruReadMsgs wiring");
    }

    fn purge(&mut self) -> IoResult<()> {
        unimplemented!("PassThruIoctl(CLEAR_TX_BUFFER|CLEAR_RX_BUFFER)");
    }

    fn description(&self) -> &str {
        &self.desc
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: device_id выдан этой же библиотекой.
        unsafe {
            (self.lib.vtable().PassThruClose)(self.device_id);
        }
    }
}
