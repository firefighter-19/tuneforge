//! Kernel-upload-путь для дампа Subaru SH7055/SH7058 ROM.
//!
//! См. crate-level README — здесь только публичный API.
//!
//! Высокоуровневый flow: [`dump::dump_rom_via_kernel`] делает всё в одной
//! функции от Transport-trait до полного ROM-байт-массива.
//!
//! Низкоуровневые модули доступны напрямую для отладки/тестов.

#![doc = include_str!("../README.md")]

pub mod dump;
pub mod error;
pub mod kernel_wire;
pub mod kernels;
pub mod kwp2000;
pub mod seed_key;
pub mod upload;
// CAN/ISO15765 path — для 2007+ Subaru (Forester XT, Impreza).
pub mod can_upload;
pub mod can_wire;

pub use dump::{dump_rom_via_kernel, KernelDumpConfig};
pub use error::{KernelError, KernelResult};
pub use kernels::{KernelBinary, McuFamily};
