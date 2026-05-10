//! Модель ROM-образа: байты + таблицы + контрольные суммы.

#![forbid(unsafe_code)]

pub mod checksum;
pub mod error;
pub mod image;
pub mod table;

pub use error::{RomError, RomResult};
pub use image::RomImage;
pub use table::{Table, TableValues};
