//! Shared primitives for the `romraider-rs` workspace.

#![forbid(unsafe_code)]

pub mod address;
pub mod bytes;
pub mod endian;
pub mod error;

pub use address::Address;
pub use endian::Endian;
pub use error::{CoreError, CoreResult};
