//! Интеграционные тесты на эталонных фикстурах из апстрим-репозитория.
//!
//! Файлы в `tests/fixtures/` — копия `RomRaider/src/test/definitions/*.xml`.
//! Если апстрим-формат меняется — синхронизируем фикстуры и обновляем эти тесты.

use std::path::PathBuf;

use romraider_defs::{parse_file, RomsDocument};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn load(name: &str) -> RomsDocument {
    let path = fixtures_dir().join(name);
    parse_file(&path).unwrap_or_else(|e| panic!("parsing {path:?}: {e}"))
}

#[test]
fn scalingbase_test_fixture_has_expected_shape() {
    let doc = load("scalingbase_test.xml");

    assert_eq!(doc.scaling_bases.len(), 5, "scalingbase count");

    // Один из ожидаемых scaling-base — rpm.
    let rpm = doc.find_scaling_base("rpm").expect("rpm base present");
    assert_eq!(rpm.expression, "x");
    assert_eq!(rpm.to_byte, "x");

    // 2 ROM в этой фикстуре: 32BITBASE (шаблон) и A2WC522S (наследник).
    assert_eq!(doc.roms.len(), 2, "rom count");

    let base = doc
        .find_rom_by_xml_id("32BITBASE")
        .expect("32BITBASE present");
    assert!(base.base.is_none(), "base ROM should not inherit");
    assert_eq!(base.tables.len(), 4, "base ROM tables");

    let child = doc
        .find_rom_by_xml_id("A2WC522S")
        .expect("A2WC522S present");
    assert_eq!(child.base.as_deref(), Some("32BITBASE"));
    assert_eq!(
        child.romid.as_ref().unwrap().ecu_id.as_deref(),
        Some("2F12795606")
    );
    // Дочерний ROM переопределяет только адреса — 3 таблицы, у каждой по 2 оси.
    assert_eq!(child.tables.len(), 3);
    for t in &child.tables {
        assert_eq!(t.nested.len(), 2, "each table has X and Y axis");
    }
}

#[test]
fn scalingbase_test_target_boost_a_has_all_metadata() {
    let doc = load("scalingbase_test.xml");
    let base = doc.find_rom_by_xml_id("32BITBASE").unwrap();
    let tb_a = base
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("Target Boost A"))
        .expect("Target Boost A in base");

    assert_eq!(tb_a.kind.as_deref(), Some("3D"));
    assert_eq!(tb_a.storage_type.as_deref(), Some("uint16"));
    assert_eq!(tb_a.endian.as_deref(), Some("big"));
    assert_eq!(tb_a.size_x.as_deref(), Some("8"));
    assert_eq!(tb_a.size_y.as_deref(), Some("12"));
    assert_eq!(tb_a.log_param.as_deref(), Some("E52"));

    // Две scaling-ссылки по имени + 2 оси с собственным scaling.
    assert_eq!(tb_a.scalings.len(), 2);
    assert!(tb_a
        .scalings
        .iter()
        .any(|s| s.base.as_deref() == Some("BoostTarget_barabsolute")));
    assert!(tb_a
        .scalings
        .iter()
        .any(|s| s.base.as_deref() == Some("BoostTarget_psirelativesealevel")));

    assert!(tb_a
        .description
        .as_deref()
        .unwrap()
        .contains("desired boost targets"));
}

#[test]
fn scalingbase_test_target_boost_b_inherits_from_a() {
    let doc = load("scalingbase_test.xml");
    let base = doc.find_rom_by_xml_id("32BITBASE").unwrap();
    let tb_b = base
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("Target Boost B"))
        .expect("Target Boost B in base");

    assert_eq!(tb_b.base.as_deref(), Some("Target Boost A"));
    assert!(tb_b.kind.is_none(), "type не задан — наследуется");
    assert!(tb_b.description.as_deref().unwrap().contains("Inherited"));
}

#[test]
fn learning_airflow_ranges_fixture_parses() {
    let doc = load("LearningAirflowRanges.xml");
    assert_eq!(doc.roms.len(), 3);

    let base = doc.find_rom_by_xml_id("32BITBASE").unwrap();
    // База содержит 2 «полноценные» таблицы со static Y axis.
    assert_eq!(base.tables.len(), 2);

    for t in &base.tables {
        assert_eq!(t.kind.as_deref(), Some("2D"));
        let static_axis = &t.nested[0];
        assert_eq!(static_axis.kind.as_deref(), Some("Static Y Axis"));
        assert_eq!(static_axis.data.len(), 3);
    }

    // Цепочка наследования: A2WC510S → A2WC510N → 32BITBASE.
    let a2wc510s = doc.find_rom_by_xml_id("A2WC510S").unwrap();
    assert_eq!(a2wc510s.base.as_deref(), Some("A2WC510N"));
    assert!(a2wc510s.tables.is_empty(), "наследует таблицы целиком");
}
