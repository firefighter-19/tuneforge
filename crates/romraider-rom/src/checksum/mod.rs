//! Алгоритмы пересчёта контрольных сумм.
//!
//! У апстрима алгоритмы живут в двух местах:
//! - `com.romraider.maps.checksum.*` — Nissan-style (STD/ALT/ALT2), BMW
//!   Motronic, GM E38, BYTEXOR, COPY. Конфигурируются через `<checksum>`
//!   элемент в ROM-определении и регистрируются через [`ChecksumModule`].
//! - `com.romraider.maps.RomChecksum.java` — Subaru-style: жёстко зашитый
//!   алгоритм, ищущий **таблицы с именем `"checksum fix*"`**, в которых
//!   лежат `(start, end, diff)`-тройки. Реализован у нас как free-функции
//!   в [`subaru_classic`] — без конфигурации через `<checksum>`.
//!
//! Trait [`ChecksumModule`] зарезервирован под Nissan/BMW/GM (будут
//! отдельными слайсами). Сейчас регистр пуст.

use crate::error::{RomError, RomResult};
use crate::image::RomImage;

pub mod subaru_classic;

/// Контракт «конфигурируемого» checksum-модуля (Nissan/BMW/GM family).
///
/// Subaru-classic под него не подходит — там вообще нет конфигурации,
/// см. [`subaru_classic::fix`].
pub trait ChecksumModule: Send + Sync {
    fn name(&self) -> &'static str;
    fn verify(&self, rom: &RomImage) -> RomResult<()>;
    fn fix(&self, rom: &mut RomImage) -> RomResult<()>;
}

/// Реестр известных модулей (пока пуст — заполняется по мере добавления семей).
#[must_use]
pub fn by_name(_name: &str) -> Option<Box<dyn ChecksumModule>> {
    None
}

#[allow(dead_code)]
fn unsupported(name: &str) -> RomError {
    RomError::UnsupportedChecksum(name.to_string())
}
