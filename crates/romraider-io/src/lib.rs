//! Hardware transports.
//!
//! Каждый транспорт (serial, ELM327, J2534) реализует [`Transport`].
//! Высокоуровневые протоколы (`romraider-protocol`) работают только через этот
//! трейт, что позволяет подменять физический канал моками в тестах.

#![cfg_attr(not(feature = "j2534"), forbid(unsafe_code))]

pub mod error;
pub mod transport;

#[cfg(feature = "serial")]
pub mod serial;

#[cfg(feature = "elm327")]
pub mod elm327;

#[cfg(feature = "j2534")]
pub mod j2534;

pub use error::{IoError, IoResult};
pub use transport::Transport;
