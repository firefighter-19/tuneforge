//! Декодирование байтов ROM в `f64`-значения по [`StorageType`] и [`Endian`].
//!
//! Все ECU-параметры в итоге сводятся к одному из 9 типов хранения
//! (uint8/int8/uint16/int16/uint32/int32/float/hex/char) — это покрывает
//! и таблицы, и оси, и одиночные параметры. Возврат сразу в `f64` — потому
//! что scaling-формулы всегда работают в плавающей точке.

use romraider_core::Endian;
use romraider_defs::StorageType;

use crate::error::{RomError, RomResult};

/// Декодировать `count` ячеек начиная с `bytes[0]`. Размер буфера должен
/// быть ровно `count * storage_type.byte_size()`.
pub fn decode_cells(
    bytes:        &[u8],
    storage_type: StorageType,
    endian:       Endian,
    count:        usize,
) -> RomResult<Vec<f64>> {
    let stride = storage_type.byte_size();
    let expected = stride
        .checked_mul(count)
        .ok_or(RomError::DecodeOverflow { count, stride })?;
    if bytes.len() != expected {
        return Err(RomError::DecodeSizeMismatch {
            got:      bytes.len(),
            expected,
        });
    }
    let mut out = Vec::with_capacity(count);
    for chunk in bytes.chunks_exact(stride) {
        out.push(decode_one(chunk, storage_type, endian));
    }
    Ok(out)
}

/// Декодировать одну ячейку. Длина `bytes` должна точно совпадать с
/// `storage_type.byte_size()` (вызывается из `decode_cells` после нарезки).
#[must_use]
fn decode_one(bytes: &[u8], storage_type: StorageType, endian: Endian) -> f64 {
    match storage_type {
        StorageType::UInt8  | StorageType::Hex | StorageType::Char => f64::from(bytes[0]),
        StorageType::Int8 => f64::from(bytes[0] as i8),
        StorageType::UInt16 => {
            let arr: [u8; 2] = bytes.try_into().expect("uint16 chunk == 2 bytes");
            f64::from(endian.read_u16(&arr))
        }
        StorageType::Int16 => {
            let arr: [u8; 2] = bytes.try_into().expect("int16 chunk == 2 bytes");
            f64::from(endian.read_u16(&arr) as i16)
        }
        StorageType::UInt32 => {
            let arr: [u8; 4] = bytes.try_into().expect("uint32 chunk == 4 bytes");
            f64::from(endian.read_u32(&arr))
        }
        StorageType::Int32 => {
            let arr: [u8; 4] = bytes.try_into().expect("int32 chunk == 4 bytes");
            f64::from(endian.read_u32(&arr) as i32)
        }
        StorageType::Float => {
            let arr: [u8; 4] = bytes.try_into().expect("float chunk == 4 bytes");
            f64::from(f32::from_bits(endian.read_u32(&arr)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uint8_passthrough() {
        let v = decode_cells(&[0x00, 0xFF, 0x80, 0x01], StorageType::UInt8, Endian::Big, 4).unwrap();
        assert_eq!(v, vec![0.0, 255.0, 128.0, 1.0]);
    }

    #[test]
    fn int8_sign_extended() {
        let v = decode_cells(&[0x7F, 0x80, 0xFF, 0x00], StorageType::Int8, Endian::Big, 4).unwrap();
        assert_eq!(v, vec![127.0, -128.0, -1.0, 0.0]);
    }

    #[test]
    fn uint16_big_vs_little_endian() {
        let bytes = [0x00, 0x10, 0xFF, 0xFF];
        let big = decode_cells(&bytes, StorageType::UInt16, Endian::Big, 2).unwrap();
        assert_eq!(big, vec![16.0, 65535.0]);
        let lil = decode_cells(&bytes, StorageType::UInt16, Endian::Little, 2).unwrap();
        assert_eq!(lil, vec![4096.0, 65535.0]);
    }

    #[test]
    fn int16_negative_big_endian() {
        let v = decode_cells(&[0xFF, 0xFF], StorageType::Int16, Endian::Big, 1).unwrap();
        assert_eq!(v, vec![-1.0]);
    }

    #[test]
    fn float_little_endian_round_trip() {
        // 1.5 в IEEE-754 single: 0x3FC00000 → LE = 00 00 C0 3F
        let v = decode_cells(&[0x00, 0x00, 0xC0, 0x3F], StorageType::Float, Endian::Little, 1).unwrap();
        assert_eq!(v, vec![1.5]);
    }

    #[test]
    fn float_big_endian_round_trip() {
        // 1.5 BE: 3F C0 00 00
        let v = decode_cells(&[0x3F, 0xC0, 0x00, 0x00], StorageType::Float, Endian::Big, 1).unwrap();
        assert_eq!(v, vec![1.5]);
    }

    #[test]
    fn wrong_buffer_size_is_an_error() {
        let err = decode_cells(&[0x00, 0x10, 0x00], StorageType::UInt16, Endian::Big, 2).unwrap_err();
        assert!(matches!(err, RomError::DecodeSizeMismatch { .. }));
    }

    #[test]
    fn zero_count_returns_empty() {
        let v = decode_cells(&[], StorageType::UInt16, Endian::Big, 0).unwrap();
        assert!(v.is_empty());
    }
}
