use std::path::Path;
use std::sync::Arc;

use libloading::{Library as DynLib, Symbol};

use super::api::PassThruVtable;
use crate::error::{IoError, IoResult};

/// Динамически загруженная J2534-библиотека (`.dll` под Windows, `.so` под Linux).
///
/// Хранит и `DynLib`, и заполненный `vtable`. Объекты `Device` держат
/// `Arc<Library>` — пока живо хоть одно устройство, библиотека не выгружается.
pub struct Library {
    _lib:   DynLib,                 // должна жить дольше vtable
    vtable: PassThruVtable,
    path:   String,
}

impl Library {
    /// # Safety
    ///
    /// Загружает динамическую библиотеку — это `unsafe` потому, что rustc не может
    /// верифицировать соответствие сигнатур. Сигнатуры проверены на соответствие
    /// J2534-1 v04.04, но любая «прошитая» сторонняя `.dll` может оказаться
    /// несовместимой. Не загружайте библиотеки с непроверенных путей.
    pub unsafe fn open(path: impl AsRef<Path>) -> IoResult<Arc<Self>> {
        let path_str = path.as_ref().display().to_string();
        // SAFETY: см. контракт метода.
        let lib = unsafe {
            DynLib::new(path.as_ref()).map_err(|source| IoError::J2534LoadFailed {
                path: path_str.clone(),
                source,
            })?
        };

        // SAFETY: сигнатуры взяты из J2534-1 v04.04. См. api.rs.
        let vtable = unsafe {
            PassThruVtable {
                PassThruOpen:           *lookup(&lib, b"PassThruOpen\0")?,
                PassThruClose:          *lookup(&lib, b"PassThruClose\0")?,
                PassThruConnect:        *lookup(&lib, b"PassThruConnect\0")?,
                PassThruDisconnect:     *lookup(&lib, b"PassThruDisconnect\0")?,
                PassThruReadMsgs:       *lookup(&lib, b"PassThruReadMsgs\0")?,
                PassThruWriteMsgs:      *lookup(&lib, b"PassThruWriteMsgs\0")?,
                PassThruIoctl:          *lookup(&lib, b"PassThruIoctl\0")?,
                PassThruGetLastError:   *lookup(&lib, b"PassThruGetLastError\0")?,
            }
        };

        Ok(Arc::new(Self { _lib: lib, vtable, path: path_str }))
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub(super) fn vtable(&self) -> &PassThruVtable {
        &self.vtable
    }
}

unsafe fn lookup<'lib, T>(lib: &'lib DynLib, name: &[u8]) -> IoResult<Symbol<'lib, T>> {
    // SAFETY: вызывающий гарантирует корректный type-erased сигнатурный тип T.
    unsafe {
        lib.get::<T>(name).map_err(|source| IoError::J2534LoadFailed {
            path: String::from_utf8_lossy(&name[..name.len() - 1]).into_owned(),
            source,
        })
    }
}
