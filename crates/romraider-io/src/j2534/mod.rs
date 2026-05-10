//! Поддержка J2534 PassThru — стандарта SAE для USB-программаторов ECU.
//!
//! Динамическая загрузка DLL/`.so` устройства через [`libloading`]; высокоуровневая
//! обёртка [`device::Device`] предоставляет безопасный API. Каждое устройство
//! поставляется со своей реализацией библиотеки (Tactrix Openport, DrewTech,
//! Op2.0 и т. д.) — путь к ней читается из реестра/конфига и подаётся в [`Library::open`].

mod api;
mod device;
mod library;

pub use api::{ConnectFlags, IoctlId, ProtocolId};
pub use device::Device;
pub use library::Library;
