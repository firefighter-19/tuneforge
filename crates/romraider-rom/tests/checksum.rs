//! Интеграция Subaru-классического checksum: edit → fix → verify cycle на
//! синтетическом ROM с известной структурой.

use romraider_defs::{parse_str, resolve};
use romraider_rom::{subaru_classic, RomImage};

/// Синтетический ROM:
/// - `0x00..0x10` (16 байт) — «карта», данные которой защищены checksum-ом.
/// - `0x10..0x1C` (12 байт) — одна checksum-fix-запись: start=0x00, end=0x10, diff.
/// - `0x1C..0x30` — паддинг, не важно.
const DEF: &str = r#"
<roms>
  <rom>
    <romid><xmlid>TEST</xmlid></romid>
    <table type="2D" name="map" storagetype="uint16" endian="big" sizex="8" storageaddress="0x00">
      <scaling units="raw" expression="x" to_byte="x"/>
    </table>
    <table type="2D" name="checksum fix region" storagetype="uint8" endian="big"
           sizey="12" storageaddress="0x10"/>
  </rom>
</roms>
"#;

fn build_rom_with_correct_checksum() -> RomImage {
    // Карта: 8 × uint16 BE = 16 байт. Заполняем последовательно 1..8.
    let mut bytes = vec![0u8; 0x30];
    for (i, v) in (1u16..=8).enumerate() {
        let arr = v.to_be_bytes();
        bytes[i * 2] = arr[0];
        bytes[i * 2 + 1] = arr[1];
    }
    // Checksum-fix-запись: start=0x00, end=0x10, diff=CHECK_TOTAL - sum.
    let sum: u32 = (0..4)
        .map(|i| {
            u32::from_be_bytes([
                bytes[i * 4],
                bytes[i * 4 + 1],
                bytes[i * 4 + 2],
                bytes[i * 4 + 3],
            ])
        })
        .fold(0u32, u32::wrapping_add);
    let diff = subaru_classic::CHECK_TOTAL.wrapping_sub(sum);

    bytes[0x10..0x14].copy_from_slice(&0x0000_0000u32.to_be_bytes()); // start
    bytes[0x14..0x18].copy_from_slice(&0x0000_0010u32.to_be_bytes()); // end
    bytes[0x18..0x1C].copy_from_slice(&diff.to_be_bytes()); // diff
    RomImage::from_bytes(bytes)
}

#[test]
fn verify_passes_on_correct_rom() {
    let doc = parse_str(DEF).unwrap();
    let resolved = resolve(&doc).unwrap();
    let rom = build_rom_with_correct_checksum();

    let results = subaru_classic::verify(&rom, &resolved[0]).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].valid, "должна быть валидной свежесобранная ROM");
    assert!(!results[0].disabled);
    assert_eq!(results[0].start.raw(), 0x0);
    assert_eq!(results[0].end.raw(), 0x10);
}

#[test]
fn verify_detects_edit_without_fix() {
    let doc = parse_str(DEF).unwrap();
    let resolved = resolve(&doc).unwrap();
    let mut rom = build_rom_with_correct_checksum();

    // Изменяем первый байт карты — checksum уже не сходится.
    rom.write(romraider_core::Address::new(0), &[0xFF]).unwrap();

    let results = subaru_classic::verify(&rom, &resolved[0]).unwrap();
    assert!(!results[0].valid, "verify должен поймать рассогласование");
    assert_ne!(results[0].stored_diff, results[0].computed_diff);
}

#[test]
fn fix_recomputes_diff_to_make_verify_pass() {
    let doc = parse_str(DEF).unwrap();
    let resolved = resolve(&doc).unwrap();
    let mut rom = build_rom_with_correct_checksum();

    rom.write(romraider_core::Address::new(0), &[0xFF]).unwrap();
    let before_fix = subaru_classic::verify(&rom, &resolved[0]).unwrap();
    assert!(!before_fix[0].valid);

    let updated = subaru_classic::fix(&mut rom, &resolved[0]).unwrap();
    assert_eq!(updated, 1, "одна запись обновлена");

    let after_fix = subaru_classic::verify(&rom, &resolved[0]).unwrap();
    assert!(after_fix[0].valid, "после fix должна быть валидной");
}

#[test]
fn disabled_slots_are_left_alone() {
    let doc = parse_str(DEF).unwrap();
    let resolved = resolve(&doc).unwrap();
    let mut rom = build_rom_with_correct_checksum();

    // Превращаем slot в «disabled»: start=0, end=0, diff=значение не важно.
    rom.write(romraider_core::Address::new(0x10), &[0; 8])
        .unwrap();
    rom.write(
        romraider_core::Address::new(0x18),
        &[0xCA, 0xFE, 0xBA, 0xBE],
    )
    .unwrap();

    let results = subaru_classic::verify(&rom, &resolved[0]).unwrap();
    assert!(results[0].valid);
    assert!(results[0].disabled);

    // fix не должен ничего обновлять
    let updated = subaru_classic::fix(&mut rom, &resolved[0]).unwrap();
    assert_eq!(updated, 0);

    // diff не изменился
    let raw = rom.raw();
    assert_eq!(&raw[0x18..0x1C], &[0xCA, 0xFE, 0xBA, 0xBE]);
}

#[test]
fn multiple_entries_in_one_fix_table() {
    // 24 байта = 2 записи в одной checksum-fix таблице.
    let def = r#"
    <roms>
      <rom>
        <romid><xmlid>T</xmlid></romid>
        <table type="2D" name="region_a" storagetype="uint16" endian="big" sizex="4" storageaddress="0x00"/>
        <table type="2D" name="region_b" storagetype="uint16" endian="big" sizex="4" storageaddress="0x08"/>
        <table type="2D" name="checksum fix double" storagetype="uint8" endian="big"
               sizey="24" storageaddress="0x10"/>
      </rom>
    </roms>
    "#;
    let doc = parse_str(def).unwrap();
    let resolved = resolve(&doc).unwrap();

    // Карты + две записи. Изначально оба diff'а нулевые — verify провалится,
    // потом fix починит обе.
    let mut bytes = vec![0u8; 0x40];
    for (i, v) in (1u16..=8).enumerate() {
        let arr = v.to_be_bytes();
        bytes[i * 2] = arr[0];
        bytes[i * 2 + 1] = arr[1];
    }
    bytes[0x10..0x14].copy_from_slice(&0x0000_0000u32.to_be_bytes()); // start
    bytes[0x14..0x18].copy_from_slice(&0x0000_0008u32.to_be_bytes()); // end (regiom_a)
    bytes[0x1C..0x20].copy_from_slice(&0x0000_0008u32.to_be_bytes()); // start (region_b)
    bytes[0x20..0x24].copy_from_slice(&0x0000_0010u32.to_be_bytes()); // end
    let mut rom = RomImage::from_bytes(bytes);

    let before = subaru_classic::verify(&rom, &resolved[0]).unwrap();
    assert_eq!(before.len(), 2);
    assert!(!before[0].valid && !before[1].valid);

    let updated = subaru_classic::fix(&mut rom, &resolved[0]).unwrap();
    assert_eq!(updated, 2);

    let after = subaru_classic::verify(&rom, &resolved[0]).unwrap();
    assert!(after.iter().all(|r| r.valid));
}
