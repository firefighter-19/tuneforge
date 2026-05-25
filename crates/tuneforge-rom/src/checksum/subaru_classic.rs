//! Subaru-классический алгоритм контрольных сумм («checksum fix»-таблицы).
//!
//! В Java живёт в `com.romraider.maps.RomChecksum.java`. Алгоритм:
//!
//! 1. Найти все таблицы в резолвнутом ROM-определении, имя которых начинается
//!    с `"checksum fix"`. Каждая такая таблица — массив `(start, end, diff)`
//!    32-битных big-endian троек (12 байт на каждую).
//! 2. Для каждой тройки: `diff_new = CHECK_TOTAL - sum(rom[start..end])`, где
//!    сумма берётся по 4-байтным BE-словам с 32-битным wrapping.
//! 3. Запись/проверка: при сохранении ROM мы пересчитываем все `diff`'ы и
//!    записываем; при проверке — сравниваем stored vs computed.
//!
//! Тройка `(start=0, end=0)` — «slot disabled», пропускается.

use tuneforge_core::Address;
use tuneforge_defs::{ResolvedRom, ResolvedTable, StorageType};

use crate::error::{RomError, RomResult};
use crate::image::RomImage;

/// Магическая константа — см. `Settings.CHECK_TOTAL` в апстриме.
pub const CHECK_TOTAL: u32 = 0x5AA5_A55A;

/// Результат проверки одной `(start, end, diff)`-записи checksum-fix-таблицы.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryStatus {
    pub table_name: String,
    pub entry_idx: usize,
    pub start: Address,
    pub end: Address,
    pub stored_diff: u32,
    pub computed_diff: u32,
    pub valid: bool,
    pub disabled: bool,
}

/// Пересчитать и записать `diff` для всех checksum-fix-таблиц в `def`.
/// Возвращает количество обновлённых записей (slot-disabled пропускаются).
pub fn fix(rom: &mut RomImage, def: &ResolvedRom) -> RomResult<usize> {
    // Сначала прочитать все записи и посчитать новые diff (immutable phase),
    // потом записать (mutable phase) — иначе borrow checker не пропустит.
    let mut writes: Vec<(Address, [u8; 4])> = Vec::new();

    for entry in iter_entries(rom, def)? {
        if entry.start == 0 && entry.end == 0 {
            continue; // slot disabled
        }
        let new_diff = calculate_diff(rom.raw(), entry.start, entry.end)?;
        writes.push((Address::new(entry.diff_addr), new_diff.to_be_bytes()));
    }

    let count = writes.len();
    for (addr, bytes) in writes {
        rom.write(addr, &bytes)?;
    }
    Ok(count)
}

/// Проверить все checksum-fix-таблицы и вернуть статус каждой записи.
pub fn verify(rom: &RomImage, def: &ResolvedRom) -> RomResult<Vec<EntryStatus>> {
    let mut out = Vec::new();
    for entry in iter_entries(rom, def)? {
        let disabled = entry.start == 0 && entry.end == 0;
        let computed = if disabled {
            entry.stored_diff
        } else {
            calculate_diff(rom.raw(), entry.start, entry.end)?
        };
        out.push(EntryStatus {
            table_name: entry.table_name,
            entry_idx: entry.entry_idx,
            start: Address::new(entry.start),
            end: Address::new(entry.end),
            stored_diff: entry.stored_diff,
            computed_diff: computed,
            valid: disabled || entry.stored_diff == computed,
            disabled,
        });
    }
    Ok(out)
}

/// Сумма 4-байтных BE-слов в `bytes[start..end]` с 32-битным wrapping,
/// потом `CHECK_TOTAL - sum`.
///
/// `end` должен быть >= `start` и не выходить за пределы буфера.
/// `(end - start)` должен делиться на 4 (иначе хвостовые байты игнорируются).
pub fn calculate_diff(bytes: &[u8], start: u32, end: u32) -> RomResult<u32> {
    let s = start as usize;
    let e = end as usize;
    if e > bytes.len() || s > e {
        return Err(RomError::AddressOutOfRange {
            addr: end,
            size: bytes.len(),
        });
    }
    let mut sum: u32 = 0;
    let mut i = s;
    while i + 4 <= e {
        let arr: [u8; 4] = bytes[i..i + 4].try_into().expect("4");
        sum = sum.wrapping_add(u32::from_be_bytes(arr));
        i += 4;
    }
    Ok(CHECK_TOTAL.wrapping_sub(sum))
}

// ---------------------------------------------------------- внутреннее ---

struct ParsedEntry {
    table_name: String,
    entry_idx: usize,
    start: u32,
    end: u32,
    stored_diff: u32,
    /// Адрес 4-байтного слова diff в ROM — куда писать обновлённое значение.
    diff_addr: u32,
}

fn iter_entries(rom: &RomImage, def: &ResolvedRom) -> RomResult<Vec<ParsedEntry>> {
    let mut entries = Vec::new();
    for table in &def.tables {
        if !is_checksum_fix_table(table) {
            continue;
        }
        let Some(table_addr) = table.storage_address else {
            continue;
        };
        let Some(total_bytes) = byte_count(table) else {
            continue;
        };
        if total_bytes == 0 || total_bytes % 12 != 0 {
            continue;
        }
        let n = total_bytes / 12;
        for entry_idx in 0..n {
            let entry_offset = (entry_idx * 12) as u32;
            let entry_addr = table_addr.raw() + entry_offset;
            let raw: [u8; 12] = rom
                .read(Address::new(entry_addr), 12)?
                .try_into()
                .expect("12");
            entries.push(ParsedEntry {
                table_name: table.name.clone(),
                entry_idx,
                start: u32::from_be_bytes(raw[0..4].try_into().unwrap()),
                end: u32::from_be_bytes(raw[4..8].try_into().unwrap()),
                stored_diff: u32::from_be_bytes(raw[8..12].try_into().unwrap()),
                diff_addr: entry_addr + 8,
            });
        }
    }
    Ok(entries)
}

fn is_checksum_fix_table(t: &ResolvedTable) -> bool {
    // Java: `table.getName().startsWith("checksum fix")` — case-sensitive.
    t.name.starts_with("checksum fix")
}

/// Полный размер таблицы в байтах: `byte_size(storage_type) * size_x * size_y`
/// (отсутствующие размерности трактуем как 1).
fn byte_count(t: &ResolvedTable) -> Option<usize> {
    let st = t.storage_type?;
    let sx = t.size_x.unwrap_or(1) as usize;
    let sy = t.size_y.unwrap_or(1) as usize;
    Some(st.byte_size() * sx * sy)
}

// «Helper» для пользователей: подавит linter-warning, если кто-то импортирует
// `StorageType` ради `byte_size` через этот модуль.
#[allow(dead_code)]
const _STORAGE_TYPE_USED: Option<StorageType> = None;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_diff_known_sum() {
        // 4 BE-слова: 1, 2, 3, 4 → sum=10. diff = CHECK_TOTAL - 10
        let bytes = [
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00,
            0x00, 0x04,
        ];
        let diff = calculate_diff(&bytes, 0, 16).unwrap();
        assert_eq!(diff, CHECK_TOTAL.wrapping_sub(10));
    }

    #[test]
    fn calculate_diff_wraps_on_overflow() {
        // 0xFFFFFFFF + 0x00000001 = 0 (wrap) → diff = CHECK_TOTAL
        let bytes = [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x01];
        let diff = calculate_diff(&bytes, 0, 8).unwrap();
        assert_eq!(diff, CHECK_TOTAL);
    }

    #[test]
    fn calculate_diff_empty_range() {
        let bytes = [0u8; 4];
        let diff = calculate_diff(&bytes, 0, 0).unwrap();
        assert_eq!(diff, CHECK_TOTAL); // CHECK_TOTAL - 0
    }

    #[test]
    fn calculate_diff_out_of_range_errors() {
        let bytes = [0u8; 4];
        let err = calculate_diff(&bytes, 0, 8).unwrap_err();
        assert!(matches!(err, RomError::AddressOutOfRange { .. }));
    }

    #[test]
    fn calculate_diff_inverse_relationship() {
        // Главное свойство: sum + diff == CHECK_TOTAL для любого региона.
        let bytes = [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE];
        let diff = calculate_diff(&bytes, 0, 8).unwrap();
        let s1 = u32::from_be_bytes([0xDE, 0xAD, 0xBE, 0xEF]);
        let s2 = u32::from_be_bytes([0xCA, 0xFE, 0xBA, 0xBE]);
        let sum = s1.wrapping_add(s2);
        assert_eq!(sum.wrapping_add(diff), CHECK_TOTAL);
    }
}
