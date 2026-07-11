use std::path::Path;

use tracing::debug;
use tuneforge_core::{bytes, Address};
use tuneforge_defs::{ResolvedTable, TableKind};

use crate::decode::{decode_cells, encode_cells};
use crate::error::{RomError, RomResult};

/// ROM-образ — простой буфер с операциями чтения/записи по адресу.
///
/// Адресация **flat**: смещение в файле = адрес из определения. Если у конкретной
/// прошивки иной memory-model (страничная, mapping и т.п.), логика маппинга
/// должна жить выше — в `EcuDefinition::memory_model`.
pub struct RomImage {
    bytes: Vec<u8>,
    path: Option<std::path::PathBuf>,
    dirty: bool,
}

impl RomImage {
    pub fn open(path: impl AsRef<Path>) -> RomResult<Self> {
        let bytes = std::fs::read(path.as_ref())?;
        debug!(size = bytes.len(), path = %path.as_ref().display(), "loaded ROM");
        Ok(Self {
            bytes,
            path: Some(path.as_ref().to_path_buf()),
            dirty: false,
        })
    }

    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            path: None,
            dirty: false,
        }
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
            tuneforge_core::CoreError::OutOfBounds { .. } => RomError::AddressOutOfRange {
                addr: at.raw(),
                size: self.bytes.len(),
            },
            other => other.into(),
        })
    }

    pub fn write(&mut self, at: Address, data: &[u8]) -> RomResult<()> {
        let offset = at.raw() as usize;
        let dst = bytes::slice_mut(&mut self.bytes, offset, data.len()).map_err(|e| match e {
            tuneforge_core::CoreError::OutOfBounds { .. } => RomError::AddressOutOfRange {
                addr: at.raw(),
                size: data.len(),
            },
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

    /// Прочитать `count` ячеек по адресу/типу/endian-у из таблицы. Возвращает
    /// «сырые» f64 — без применения scaling-формул.
    ///
    /// Подходит и для основной таблицы, и для осей (для оси `count` обычно
    /// берётся из `size_x`/`size_y` родителя — этот метод не делает выводов
    /// о размере сам, а принимает явный аргумент).
    pub fn read_cells(&self, table: &ResolvedTable, count: usize) -> RomResult<Vec<f64>> {
        let storage_type = table
            .storage_type
            .ok_or_else(|| RomError::TableMissingField {
                name: table.name.clone(),
                field: "storage_type",
            })?;
        let endian = table.endian.ok_or_else(|| RomError::TableMissingField {
            name: table.name.clone(),
            field: "endian",
        })?;
        let address = table
            .storage_address
            .ok_or_else(|| RomError::TableMissingField {
                name: table.name.clone(),
                field: "storage_address",
            })?;
        let stride = storage_type.byte_size();
        let total = stride
            .checked_mul(count)
            .ok_or(RomError::DecodeOverflow { count, stride })?;
        let bytes = self.read(address, total)?;
        decode_cells(bytes, storage_type, endian, count)
    }

    /// Записать `values` (как байт-значения, **до** scaling — то, что
    /// получается через `CompiledScaling::to_byte`) обратно в ROM по адресу
    /// таблицы. Кол-во ячеек == `values.len()`. Помечает образ как `dirty`.
    pub fn write_cells(&mut self, table: &ResolvedTable, values: &[f64]) -> RomResult<()> {
        let storage_type = table
            .storage_type
            .ok_or_else(|| RomError::TableMissingField {
                name: table.name.clone(),
                field: "storage_type",
            })?;
        let endian = table.endian.ok_or_else(|| RomError::TableMissingField {
            name: table.name.clone(),
            field: "endian",
        })?;
        let address = table
            .storage_address
            .ok_or_else(|| RomError::TableMissingField {
                name: table.name.clone(),
                field: "storage_address",
            })?;
        let bytes = encode_cells(values, storage_type, endian);
        self.write(address, &bytes)
    }

    /// Записать таблицу целиком — кол-во ячеек выводится из `kind` и
    /// `size_x`/`size_y` (как в [`Self::read_table`]). Длина `values`
    /// должна совпадать с ожидаемым числом.
    pub fn write_table(&mut self, table: &ResolvedTable, values: &[f64]) -> RomResult<()> {
        let expected = table_cell_count(table).ok_or_else(|| RomError::TableMissingField {
            name: table.name.clone(),
            field: "size_x or size_y",
        })?;
        if values.len() != expected {
            return Err(RomError::DecodeSizeMismatch {
                got: values.len(),
                expected,
            });
        }
        self.write_cells(table, values)
    }

    /// Записать таблицу целиком из РЕАЛЬНЫХ (engineering) значений: применяет
    /// обратный scaling первой шкалы (`to_byte`), затем пишет байты. Если у
    /// таблицы нет scaling с `to_byte`, значения пишутся как есть (identity) —
    /// так же, как это делает GUI-редактор. Длину проверяет [`Self::write_table`].
    pub fn write_table_real(
        &mut self,
        table: &ResolvedTable,
        real_values: &[f64],
    ) -> RomResult<()> {
        let byte_values: Vec<f64> = match table.scalings.first().and_then(|s| s.compile().ok()) {
            Some(compiled) => real_values.iter().map(|&v| compiled.to_byte(v)).collect(),
            None => real_values.to_vec(),
        };
        self.write_table(table, &byte_values)
    }

    /// Прочитать таблицу целиком — count выводится из `kind` и `size_x`/`size_y`:
    /// - `3D`: `size_x * size_y`
    /// - `2D`/`X Axis`/`Y Axis`: `size_x` или `size_y` (что задано)
    /// - `1D`: 1
    /// - `Static *`: 0 (нет байтов в ROM)
    ///
    /// Для осей внутри `3D`-таблицы это API часто бесполезно (их размер живёт
    /// у родителя); в таком случае используйте [`Self::read_cells`] с явным
    /// `count` из parent's `size_x`/`size_y`.
    pub fn read_table(&self, table: &ResolvedTable) -> RomResult<Vec<f64>> {
        let count = table_cell_count(table).ok_or_else(|| RomError::TableMissingField {
            name: table.name.clone(),
            field: "size_x or size_y",
        })?;
        self.read_cells(table, count)
    }
}

/// Сколько ячеек ожидается прочитать из таблицы исходя из `kind` и `size_*`.
/// Возвращает `Some(0)` для статических осей (нет байт), `None` — если данных
/// для расчёта недостаточно.
fn table_cell_count(table: &ResolvedTable) -> Option<usize> {
    let sx = table.size_x.map(|v| v as usize);
    let sy = table.size_y.map(|v| v as usize);
    match table.kind {
        Some(TableKind::ThreeD) => Some(sx? * sy?),
        Some(TableKind::TwoD | TableKind::XAxis | TableKind::YAxis) => sx.or(sy),
        Some(TableKind::OneD) => Some(1),
        Some(TableKind::StaticXAxis | TableKind::StaticYAxis) => Some(0),
        // Switch/BitwiseSwitch не читаются через read_table — у них своя
        // схема, см. `editor::render_switch`/`render_bitwise_switch`.
        Some(TableKind::Switch | TableKind::BitwiseSwitch) => None,
        None => sx.or(sy),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuneforge_core::Endian;
    use tuneforge_defs::{ResolvedScaling, StorageType};

    fn table(
        kind: TableKind,
        st: StorageType,
        endian: Endian,
        addr: u32,
        sx: Option<u32>,
        sy: Option<u32>,
    ) -> ResolvedTable {
        ResolvedTable {
            name: "t".into(),
            kind: Some(kind),
            category: None,
            storage_type: Some(st),
            endian: Some(endian),
            storage_address: Some(Address::new(addr)),
            size_x: sx,
            size_y: sy,
            user_level: None,
            log_param: None,
            scalings: Vec::<ResolvedScaling>::new(),
            axes: Vec::new(),
            data: Vec::new(),
            states: Vec::new(),
            bits: Vec::new(),
            description: None,
        }
    }

    #[test]
    fn read_2d_uint16_big_endian() {
        let rom = RomImage::from_bytes(vec![
            0x00, 0x00, // padding
            0x00, 0x10, 0x00, 0x20, 0x00, 0x40, 0x00, 0x80, // 4 cells
        ]);
        let t = table(
            TableKind::TwoD,
            StorageType::UInt16,
            Endian::Big,
            2,
            Some(4),
            None,
        );
        let cells = rom.read_table(&t).unwrap();
        assert_eq!(cells, vec![16.0, 32.0, 64.0, 128.0]);
    }

    #[test]
    fn read_3d_table_size_is_x_times_y() {
        let rom = RomImage::from_bytes(vec![0x00u8, 0x01, 0x02, 0x03, 0x04, 0x05]);
        let t = table(
            TableKind::ThreeD,
            StorageType::UInt8,
            Endian::Big,
            0,
            Some(3),
            Some(2),
        );
        let cells = rom.read_table(&t).unwrap();
        assert_eq!(cells, vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn missing_storage_type_returns_typed_error() {
        let rom = RomImage::from_bytes(vec![0u8; 8]);
        let mut t = table(
            TableKind::TwoD,
            StorageType::UInt8,
            Endian::Big,
            0,
            Some(4),
            None,
        );
        t.storage_type = None;
        let err = rom.read_table(&t).unwrap_err();
        assert!(matches!(
            err,
            RomError::TableMissingField {
                field: "storage_type",
                ..
            }
        ));
    }

    #[test]
    fn read_table_address_out_of_range() {
        let rom = RomImage::from_bytes(vec![0u8; 4]);
        let t = table(
            TableKind::TwoD,
            StorageType::UInt8,
            Endian::Big,
            0,
            Some(8),
            None,
        );
        let err = rom.read_table(&t).unwrap_err();
        assert!(matches!(err, RomError::AddressOutOfRange { .. }));
    }

    #[test]
    fn read_cells_axis_with_explicit_count() {
        let rom = RomImage::from_bytes(vec![
            0x3F, 0xC0, 0x00, 0x00, // 1.5
            0x40, 0x00, 0x00, 0x00, // 2.0
            0x40, 0x40, 0x00, 0x00, // 3.0
        ]);
        let axis = table(
            TableKind::XAxis,
            StorageType::Float,
            Endian::Big,
            0,
            None,
            None,
        );
        let cells = rom.read_cells(&axis, 3).unwrap();
        assert_eq!(cells, vec![1.5, 2.0, 3.0]);
    }

    #[test]
    fn write_table_round_trip_preserves_values() {
        let mut rom = RomImage::from_bytes(vec![0u8; 16]);
        let t = table(
            TableKind::TwoD,
            StorageType::UInt16,
            Endian::Big,
            0,
            Some(4),
            None,
        );
        rom.write_table(&t, &[10.0, 200.0, 3000.0, 65535.0])
            .unwrap();
        assert!(rom.is_dirty());
        assert_eq!(
            rom.read_table(&t).unwrap(),
            vec![10.0, 200.0, 3000.0, 65535.0]
        );
    }

    #[test]
    fn write_table_size_mismatch_returns_error() {
        let mut rom = RomImage::from_bytes(vec![0u8; 16]);
        let t = table(
            TableKind::TwoD,
            StorageType::UInt16,
            Endian::Big,
            0,
            Some(4),
            None,
        );
        let err = rom.write_table(&t, &[1.0, 2.0]).unwrap_err();
        assert!(matches!(err, RomError::DecodeSizeMismatch { .. }));
    }

    #[test]
    fn write_cells_writes_through_to_underlying_bytes() {
        let mut rom = RomImage::from_bytes(vec![0u8; 8]);
        let t = table(
            TableKind::TwoD,
            StorageType::UInt8,
            Endian::Big,
            2,
            Some(3),
            None,
        );
        rom.write_cells(&t, &[1.0, 2.0, 3.0]).unwrap();
        assert_eq!(&rom.raw()[2..5], &[1, 2, 3]);
    }
}
