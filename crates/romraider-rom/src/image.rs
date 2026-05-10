use std::path::Path;

use romraider_core::{bytes, Address};
use tracing::debug;

use crate::error::{RomError, RomResult};

/// ROM-образ — простой буфер с операциями чтения/записи по адресу.
///
/// Адресация **flat**: смещение в файле = адрес из определения. Если у конкретной
/// прошивки иной memory-model (страничная, mapping и т.п.), логика маппинга
/// должна жить выше — в `EcuDefinition::memory_model`.
pub struct RomImage {
    bytes: Vec<u8>,
    path:  Option<std::path::PathBuf>,
    dirty: bool,
}

impl RomImage {
    pub fn open(path: impl AsRef<Path>) -> RomResult<Self> {
        let bytes = std::fs::read(path.as_ref())?;
        debug!(size = bytes.len(), path = %path.as_ref().display(), "loaded ROM");
        Ok(Self { bytes, path: Some(path.as_ref().to_path_buf()), dirty: false })
    }

    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { bytes, path: None, dirty: false }
    }

    pub fn save_as(&mut self, path: impl AsRef<Path>) -> RomResult<()> {
        std::fs::write(path.as_ref(), &self.bytes)?;
        self.path = Some(path.as_ref().to_path_buf());
        self.dirty = false;
        Ok(())
    }

    #[must_use]
    pub fn size(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn read(&self, at: Address, len: usize) -> RomResult<&[u8]> {
        let offset = at.raw() as usize;
        bytes::slice(&self.bytes, offset, len).map_err(|e| match e {
            romraider_core::CoreError::OutOfBounds { .. } => {
                RomError::AddressOutOfRange { addr: at.raw(), size: self.bytes.len() }
            }
            other => other.into(),
        })
    }

    pub fn write(&mut self, at: Address, data: &[u8]) -> RomResult<()> {
        let offset = at.raw() as usize;
        let dst = bytes::slice_mut(&mut self.bytes, offset, data.len()).map_err(|e| match e {
            romraider_core::CoreError::OutOfBounds { .. } => {
                RomError::AddressOutOfRange { addr: at.raw(), size: data.len() }
            }
            other => other.into(),
        })?;
        dst.copy_from_slice(data);
        self.dirty = true;
        Ok(())
    }

    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.bytes
    }
}
