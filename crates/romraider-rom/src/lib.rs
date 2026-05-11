//! Модель ROM-образа: байты + таблицы + контрольные суммы.

#![forbid(unsafe_code)]

pub mod checksum;
pub mod decode;
pub mod error;
pub mod image;
pub mod table;

pub use decode::decode_cells;
pub use error::{RomError, RomResult};
pub use image::RomImage;
pub use table::{Table, TableValues};
