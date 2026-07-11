//! End-to-end интеграционный тест: XML-определение → резолв → синтетический
//! ROM → разобранные значения с применённым scaling.

use tuneforge_defs::{parse_str, resolve};
use tuneforge_rom::RomImage;

/// XML с одним 2D-таблицей на адресе 0x10, 4 ячейки uint16 BE, scaling x*0.5.
const DEF: &str = r#"
<roms>
  <rom>
    <romid><xmlid>TEST</xmlid></romid>
    <table type="2D" name="boost" storagetype="uint16" endian="big" sizex="4" storageaddress="0x10">
      <scaling units="psi" expression="x*0.5" to_byte="x*2" format="0.0"/>
    </table>
  </rom>
</roms>
"#;

fn build_rom() -> RomImage {
    // 16 байт нулей, затем 4 × uint16 BE: 100, 200, 300, 400 → real (через *0.5): 50, 100, 150, 200
    let mut bytes = vec![0u8; 0x10];
    for v in [100u16, 200, 300, 400] {
        bytes.extend_from_slice(&v.to_be_bytes());
    }
    RomImage::from_bytes(bytes)
}

#[test]
fn full_pipeline_xml_to_real_values() {
    let doc = parse_str(DEF).unwrap();
    let resolved = resolve(&doc).unwrap();
    let rom_def = resolved.iter().find(|r| r.xml_id == "TEST").unwrap();
    let table = rom_def.tables.iter().find(|t| t.name == "boost").unwrap();

    let rom = build_rom();
    let raw = rom.read_table(table).unwrap();
    assert_eq!(raw, vec![100.0, 200.0, 300.0, 400.0]);

    let compiled = table.scalings[0].compile().unwrap();
    let scaled: Vec<f64> = raw.into_iter().map(|x| compiled.to_real(x)).collect();
    assert_eq!(scaled, vec![50.0, 100.0, 150.0, 200.0]);

    let back: Vec<f64> = scaled.iter().map(|x| compiled.to_byte(*x)).collect();
    assert_eq!(back, vec![100.0, 200.0, 300.0, 400.0]);
}

#[test]
fn missing_table_address_reports_typed_error() {
    let xml = r#"
        <roms>
          <rom>
            <romid><xmlid>T</xmlid></romid>
            <table type="2D" name="x" storagetype="uint8" endian="big" sizex="2"/>
          </rom>
        </roms>
    "#;
    let doc = parse_str(xml).unwrap();
    let resolved = resolve(&doc).unwrap();
    let table = &resolved[0].tables[0];

    let rom = RomImage::from_bytes(vec![0u8; 16]);
    let err = rom.read_table(table).unwrap_err();
    assert!(matches!(
        err,
        tuneforge_rom::RomError::TableMissingField {
            field: "storage_address",
            ..
        }
    ));
}

#[test]
fn edit_then_save_round_trip_via_scaling() {
    // Полный цикл редактирования: decode → отобразить как real → пользователь
    // правит → to_byte → encode → write. После повторного чтения видим
    // изменения, raw сериализуется в новые байты.
    let doc = parse_str(DEF).unwrap();
    let resolved = resolve(&doc).unwrap();
    let table = &resolved[0].tables[0];
    let scaling = table.scalings[0].compile().unwrap();

    let mut rom = build_rom();
    let raw = rom.read_table(table).unwrap();
    let real: Vec<f64> = raw.into_iter().map(|x| scaling.to_real(x)).collect();
    assert_eq!(real, vec![50.0, 100.0, 150.0, 200.0]);

    // «Пользователь» меняет 2-ю и 3-ю ячейки: 100 psi → 110, 150 → 175.
    let mut edited = real;
    edited[1] = 110.0;
    edited[2] = 175.0;

    // Конверсия обратно в байт-значения и запись.
    let raw_back: Vec<f64> = edited.iter().map(|x| scaling.to_byte(*x)).collect();
    rom.write_table(table, &raw_back).unwrap();
    assert!(rom.is_dirty());

    // Перечитываем — видим новые значения.
    let raw_after = rom.read_table(table).unwrap();
    let real_after: Vec<f64> = raw_after.into_iter().map(|x| scaling.to_real(x)).collect();
    assert_eq!(real_after, vec![50.0, 110.0, 175.0, 200.0]);

    // Под капотом: 110 psi → byte 220 (BE 0x00DC), 175 → byte 350 (BE 0x015E).
    assert_eq!(
        &rom.raw()[0x10..0x18],
        &[
            0x00, 0x64, // 100 (=50psi)  не тронут
            0x00, 0xDC, // 220 (=110psi) изменён
            0x01, 0x5E, // 350 (=175psi) изменён
            0x01, 0x90, // 400 (=200psi) не тронут
        ]
    );
}

#[test]
fn three_d_with_axes_via_explicit_count() {
    // 3D-таблица 3x2 uint8 + X axis (3 float LE) + Y axis (2 float LE).
    let xml = r#"
        <roms>
          <rom>
            <romid><xmlid>T</xmlid></romid>
            <table type="3D" name="t" storagetype="uint8" endian="big" sizex="3" sizey="2" storageaddress="0x00">
              <scaling units="raw" expression="x" to_byte="x"/>
              <table type="X Axis" name="x" storagetype="float" endian="little" storageaddress="0x10"/>
              <table type="Y Axis" name="y" storagetype="float" endian="little" storageaddress="0x20"/>
            </table>
          </rom>
        </roms>
    "#;
    let doc = parse_str(xml).unwrap();
    let resolved = resolve(&doc).unwrap();
    let table = &resolved[0].tables[0];

    // Сборка ROM: 6 байт основной (3*2), затем X-ось и Y-ось как **big-endian** float
    // — наш decoder/encoder для StorageType::Float игнорирует `endian` атрибут
    // из defs (он часто врёт для Subaru: указано `little`, но реально BE).
    let mut bytes = vec![10, 20, 30, 40, 50, 60]; // 0x00..0x06
    bytes.resize(0x10, 0); // pad to 0x10
    bytes.extend_from_slice(&1.0f32.to_be_bytes());
    bytes.extend_from_slice(&2.0f32.to_be_bytes());
    bytes.extend_from_slice(&3.0f32.to_be_bytes());
    bytes.resize(0x20, 0); // pad to 0x20
    bytes.extend_from_slice(&500.0f32.to_be_bytes());
    bytes.extend_from_slice(&1000.0f32.to_be_bytes());
    let rom = RomImage::from_bytes(bytes);

    let data = rom.read_table(table).unwrap();
    assert_eq!(data, vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0]);

    // Оси: размер берём из родителя — sizex=3 для X Axis, sizey=2 для Y Axis.
    let x_axis = table.axes.iter().find(|a| a.name == "x").unwrap();
    let y_axis = table.axes.iter().find(|a| a.name == "y").unwrap();
    let xs = rom.read_cells(x_axis, 3).unwrap();
    let ys = rom.read_cells(y_axis, 2).unwrap();
    assert_eq!(xs, vec![1.0, 2.0, 3.0]);
    assert_eq!(ys, vec![500.0, 1000.0]);
}

#[test]
fn write_table_real_applies_inverse_scaling() {
    // `write_table_real` принимает РЕАЛЬНЫЕ значения и сам применяет `to_byte`
    // (то, что редактор делает вручную) — это ядро headless-правки для CLI.
    let doc = parse_str(DEF).unwrap();
    let resolved = resolve(&doc).unwrap();
    let table = &resolved[0].tables[0];
    let scaling = table.scalings[0].compile().unwrap();

    let mut rom = build_rom();
    rom.write_table_real(table, &[10.0, 20.0, 30.0, 40.0])
        .unwrap();
    assert!(rom.is_dirty());

    // Читаем обратно и де-скейлим — те же реальные значения.
    let raw_after = rom.read_table(table).unwrap();
    let real_after: Vec<f64> = raw_after.into_iter().map(|x| scaling.to_real(x)).collect();
    assert_eq!(real_after, vec![10.0, 20.0, 30.0, 40.0]);

    // Под капотом to_byte = x*2 → байты 20,40,60,80 (BE uint16).
    assert_eq!(
        &rom.raw()[0x10..0x18],
        &[0x00, 0x14, 0x00, 0x28, 0x00, 0x3C, 0x00, 0x50],
    );
}

#[test]
fn write_table_real_without_scaling_is_identity() {
    // Таблица без <scaling> → значения пишутся как сырые байты (как в GUI).
    let xml = r#"
        <roms><rom><romid><xmlid>T</xmlid></romid>
        <table type="2D" name="raw" storagetype="uint8" endian="big" sizex="3" storageaddress="0x00"/>
        </rom></roms>
    "#;
    let doc = parse_str(xml).unwrap();
    let resolved = resolve(&doc).unwrap();
    let table = &resolved[0].tables[0];

    let mut rom = RomImage::from_bytes(vec![0u8; 8]);
    rom.write_table_real(table, &[1.0, 2.0, 3.0]).unwrap();
    assert_eq!(&rom.raw()[0..3], &[1, 2, 3]);
}
