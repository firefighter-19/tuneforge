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
            // ВНИМАНИЕ: для `float` игнорируем явный `endian` из defs и всегда
            // читаем big-endian. Это эмпирически совпадает с Subaru SH7058 ROM:
            // байты `41 3B 33 33` декодируются как 11.7 (BE), а defs от RomRaider
            // часто помечают axis как `endian="little"`, что даёт мусор. Java
            // RomRaider ведёт себя так же (de-facto), поэтому совпадаем с ним.
            let _ = endian;
            let arr: [u8; 4] = bytes.try_into().expect("float chunk == 4 bytes");
            f64::from(f32::from_bits(Endian::Big.read_u32(&arr)))
        }
    }
}

/// Закодировать `values` обратно в байты по [`StorageType`] и [`Endian`].
///
/// Значения вне диапазона целого типа clamp-аются (`u8::MAX` для uint8,
/// `i16::MIN..=i16::MAX` для int16 и т.п.) и **округляются** до ближайшего
/// целого. Это совпадает с поведением Java-RomRaider при ручном вводе.
#[must_use]
pub fn encode_cells(values: &[f64], storage_type: StorageType, endian: Endian) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * storage_type.byte_size());
    for &v in values {
        encode_one(v, storage_type, endian, &mut out);
    }
    out
}

fn encode_one(value: f64, storage_type: StorageType, endian: Endian, out: &mut Vec<u8>) {
    match storage_type {
        StorageType::UInt8 | StorageType::Hex | StorageType::Char => {
            let n = value.clamp(0.0, f64::from(u8::MAX)).round() as u8;
            out.push(n);
        }
        StorageType::Int8 => {
            let n = value
                .clamp(f64::from(i8::MIN), f64::from(i8::MAX))
                .round() as i8;
            out.push(n as u8);
        }
        StorageType::UInt16 => {
            let n = value.clamp(0.0, f64::from(u16::MAX)).round() as u16;
            out.extend_from_slice(&endian.write_u16(n));
        }
        StorageType::Int16 => {
            let n = value
                .clamp(f64::from(i16::MIN), f64::from(i16::MAX))
                .round() as i16;
            out.extend_from_slice(&endian.write_u16(n as u16));
        }
        StorageType::UInt32 => {
            let n = value.clamp(0.0, f64::from(u32::MAX)).round() as u32;
            out.extend_from_slice(&endian.write_u32(n));
        }
        StorageType::Int32 => {
            let n = value
                .clamp(f64::from(i32::MIN), f64::from(i32::MAX))
                .round() as i32;
            out.extend_from_slice(&endian.write_u32(n as u32));
        }
        StorageType::Float => {
            // Симметрия с `decode_one`: всегда пишем big-endian, чтобы
            // round-trip read→edit→save сохранял те же байты.
            let _ = endian;
            let bits = (value as f32).to_bits();
            out.extend_from_slice(&Endian::Big.write_u32(bits));
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
    fn float_always_big_endian_regardless_of_attr() {
        // Subaru SH7058 хранит float как BE независимо от того, что defs
        // объявляет `endian` атрибутом. И с `Endian::Little`, и с `Endian::Big`
        // декодер должен трактовать байты как big-endian.
        // 1.5 BE: 3F C0 00 00
        let bytes = [0x3F, 0xC0, 0x00, 0x00];
        assert_eq!(decode_cells(&bytes, StorageType::Float, Endian::Big,    1).unwrap(), vec![1.5]);
        assert_eq!(decode_cells(&bytes, StorageType::Float, Endian::Little, 1).unwrap(), vec![1.5]);
    }

    #[test]
    fn float_decodes_subaru_throttle_axis_bytes() {
        // Реальные байты из 2007 Forester XT (ROM 4E42504007) @ 0xC0C78:
        // 41 3B 33 33 → 11.7 (BE float), 41 9C 00 00 → 19.5.
        let bytes = [0x41, 0x3B, 0x33, 0x33, 0x41, 0x9C, 0x00, 0x00];
        // defs объявляет `endian="little"` — мы должны его игнорировать.
        let v = decode_cells(&bytes, StorageType::Float, Endian::Little, 2).unwrap();
        assert!((v[0] - 11.7).abs() < 1e-3, "got {}", v[0]);
        assert!((v[1] - 19.5).abs() < 1e-3, "got {}", v[1]);
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

    #[test]
    fn encode_uint16_big_endian_round_trip() {
        let original = [0x00, 0x10, 0xFF, 0xFF, 0x80, 0x00];
        let decoded  = decode_cells(&original, StorageType::UInt16, Endian::Big, 3).unwrap();
        let encoded  = encode_cells(&decoded, StorageType::UInt16, Endian::Big);
        assert_eq!(encoded, original);
    }

    #[test]
    fn encode_int8_negative_values() {
        let v = encode_cells(&[-1.0, -128.0, 127.0], StorageType::Int8, Endian::Big);
        assert_eq!(v, vec![0xFF, 0x80, 0x7F]);
    }

    #[test]
    fn encode_clamps_out_of_range_values() {
        // uint8 принимает 0..=255; -10 → 0, 300 → 255
        let v = encode_cells(&[-10.0, 300.0, 128.0], StorageType::UInt8, Endian::Big);
        assert_eq!(v, vec![0x00, 0xFF, 0x80]);
    }

    #[test]
    fn encode_rounds_to_nearest() {
        // 0.4 → 0, 0.6 → 1, 1.5 → 2 (banker's? нет, .round() — away-from-zero)
        let v = encode_cells(&[0.4, 0.6, 1.5, 2.5], StorageType::UInt8, Endian::Big);
        assert_eq!(v, vec![0, 1, 2, 3]);
    }

    #[test]
    fn encode_float_round_trip_big_endian() {
        // Float всегда BE при decode/encode: round-trip даёт идентичные байты,
        // даже если передан Endian::Little.
        let original = [0x3F, 0xC0, 0x00, 0x00]; // 1.5 BE
        let decoded  = decode_cells(&original, StorageType::Float, Endian::Little, 1).unwrap();
        let encoded  = encode_cells(&decoded, StorageType::Float, Endian::Little);
        assert_eq!(encoded, original);
    }

    #[test]
    fn encode_decode_full_cycle_int16() {
        let original = [-32_768.0, -1.0, 0.0, 1.0, 32_767.0];
        let bytes    = encode_cells(&original, StorageType::Int16, Endian::Big);
        let decoded  = decode_cells(&bytes, StorageType::Int16, Endian::Big, 5).unwrap();
        assert_eq!(decoded, original);
    }
}
