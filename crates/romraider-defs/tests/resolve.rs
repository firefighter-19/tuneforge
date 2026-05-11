//! Резолв на эталонных фикстурах из апстрима.

use std::path::PathBuf;

use romraider_core::{Address, Endian};
use romraider_defs::{parse_file, resolve, ResolvedRom, StorageType, TableKind};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn load(name: &str) -> Vec<ResolvedRom> {
    let path = fixtures_dir().join(name);
    let doc  = parse_file(&path).expect("parse fixture");
    resolve(&doc).expect("resolve fixture")
}

#[test]
fn scalingbase_a2wc522s_target_boost_a_fully_resolved() {
    let resolved = load("scalingbase_test.xml");

    let child = resolved
        .iter()
        .find(|r| r.xml_id == "A2WC522S")
        .expect("A2WC522S present");
    assert_eq!(child.romid.ecu_id.as_deref(), Some("2F12795606"));
    // 4 таблицы после резолва: 3 переопределены в child + 1 (CL Fueling)
    // унаследована полностью из BASE без оверрайдов.
    assert_eq!(child.tables.len(), 4);
    for required in [
        "Target Boost A",
        "Target Boost B",
        "Initial Wastegate Duty A",
        "CL Fueling Target Compensation A (ECT)",
    ] {
        assert!(
            child.tables.iter().any(|t| t.name == required),
            "expected resolved table `{required}`"
        );
    }

    let tb_a = child.tables.iter().find(|t| t.name == "Target Boost A").unwrap();
    assert_eq!(tb_a.kind,            Some(TableKind::ThreeD));
    assert_eq!(tb_a.storage_type,    Some(StorageType::UInt16));
    assert_eq!(tb_a.endian,          Some(Endian::Big));
    assert_eq!(tb_a.size_x,          Some(8));
    assert_eq!(tb_a.size_y,          Some(12));
    assert_eq!(tb_a.user_level,      Some(1));
    assert_eq!(tb_a.log_param.as_deref(), Some("E52"));
    assert_eq!(tb_a.category.as_deref(),  Some("Boost Control - Target"));
    assert_eq!(tb_a.storage_address, Some(Address::new(0xC11D4)));

    // Две scaling-ссылки разрешены до полных формул.
    assert_eq!(tb_a.scalings.len(), 2);
    let has_bar = tb_a.scalings.iter().any(|s| s.name.as_deref() == Some("BoostTarget_barabsolute"));
    let has_psi = tb_a.scalings.iter().any(|s| s.name.as_deref() == Some("BoostTarget_psirelativesealevel"));
    assert!(has_bar && has_psi, "both scalings resolved by name");
    for s in &tb_a.scalings {
        assert!(s.expression.is_some(), "expression filled from scaling base");
        assert!(s.to_byte.is_some());
    }

    // Две оси, child переопределил только адреса.
    assert_eq!(tb_a.axes.len(), 2);
    let x = tb_a.axes.iter().find(|a| a.kind == Some(TableKind::XAxis)).unwrap();
    assert_eq!(x.name, "Throttle Plate Opening Angle");
    assert_eq!(x.storage_type,    Some(StorageType::Float));
    assert_eq!(x.endian,          Some(Endian::Little));
    assert_eq!(x.storage_address, Some(Address::new(0xC1184)));
    let y = tb_a.axes.iter().find(|a| a.kind == Some(TableKind::YAxis)).unwrap();
    assert_eq!(y.name, "Engine Speed");
    assert_eq!(y.storage_address, Some(Address::new(0xC11A4)));
}

#[test]
fn scalingbase_a2wc522s_target_boost_b_inherits_via_two_levels() {
    // Цепочка: A2WC522S.tables["Target Boost B"] (только storageaddress)
    //          ↑ override-ит BASE.tables["Target Boost B"] (description + base="Target Boost A")
    //          ↑ который наследует BASE.tables["Target Boost A"].
    let resolved = load("scalingbase_test.xml");
    let child    = resolved.iter().find(|r| r.xml_id == "A2WC522S").unwrap();
    let tb_b     = child.tables.iter().find(|t| t.name == "Target Boost B").unwrap();

    assert_eq!(tb_b.kind,            Some(TableKind::ThreeD));
    assert_eq!(tb_b.storage_type,    Some(StorageType::UInt16));
    assert_eq!(tb_b.size_x,          Some(8));
    assert_eq!(tb_b.size_y,          Some(12));
    assert_eq!(tb_b.storage_address, Some(Address::new(0xC12E4)));
    assert!(tb_b
        .description
        .as_deref()
        .map(|d| d.contains("Inherited"))
        .unwrap_or(false));
    assert_eq!(tb_b.scalings.len(), 2);
    assert_eq!(tb_b.axes.len(), 2);
}

#[test]
fn learning_airflow_a2wc510s_inherits_via_chain() {
    // A2WC510S → A2WC510N → 32BITBASE; собственных таблиц нет, всё наследует.
    let resolved = load("LearningAirflowRanges.xml");
    let leaf     = resolved.iter().find(|r| r.xml_id == "A2WC510S").unwrap();

    // 2 таблицы из 32BITBASE.
    assert_eq!(leaf.tables.len(), 2);
    let t0 = leaf.tables.iter().find(|t| t.name == "A/F Learning #1 Airflow Ranges").unwrap();
    assert_eq!(t0.kind,            Some(TableKind::TwoD));
    assert_eq!(t0.storage_type,    Some(StorageType::Float));
    assert_eq!(t0.endian,          Some(Endian::Little));
    // A2WC510N задаёт storageaddress для этой таблицы, A2WC510S наследует.
    assert_eq!(t0.storage_address, Some(Address::new(0xC7078)));
    // Static Y Axis с 3 метками.
    assert_eq!(t0.axes.len(), 1);
    let static_y = &t0.axes[0];
    assert_eq!(static_y.kind, Some(TableKind::StaticYAxis));
    assert_eq!(static_y.data.len(), 3);
}

#[test]
fn all_roms_resolve_without_errors() {
    // Простой smoke-test: каждая фикстура полностью резолвится, никаких panic.
    let r1 = load("scalingbase_test.xml");
    let r2 = load("LearningAirflowRanges.xml");
    assert_eq!(r1.len(), 2);
    assert_eq!(r2.len(), 3);
}

#[test]
fn psi_relative_scaling_compiles_and_evaluates_end_to_end() {
    // Полный путь: parse XML → resolve inheritance → compile expression → eval.
    let resolved = load("scalingbase_test.xml");
    let child    = resolved.iter().find(|r| r.xml_id == "A2WC522S").unwrap();
    let tb_a     = child.tables.iter().find(|t| t.name == "Target Boost A").unwrap();

    let psi = tb_a
        .scalings
        .iter()
        .find(|s| s.name.as_deref() == Some("BoostTarget_psirelativesealevel"))
        .expect("psi scaling resolved by name");
    let compiled = psi.compile().expect("scaling compiles");

    // Формула: (x-760) * 0.01933677.
    assert!(compiled.to_real(760.0).abs() < 1e-9, "atmospheric → 0");
    let at_800 = compiled.to_real(800.0);
    assert!((at_800 - 40.0 * 0.01933677).abs() < 1e-9);

    // Обратная проверка round-trip.
    let real = compiled.to_real(900.0);
    let back = compiled.to_byte(real);
    assert!((back - 900.0).abs() < 1e-6);
}

#[test]
fn x_axis_throttle_scaling_compiles_via_inline_definition() {
    // X Axis "Throttle Plate Opening Angle" определён inline (без scalingbase).
    // Цепочка: child A2WC522S наследует ось от 32BITBASE.
    let resolved = load("scalingbase_test.xml");
    let child    = resolved.iter().find(|r| r.xml_id == "A2WC522S").unwrap();
    let tb_a     = child.tables.iter().find(|t| t.name == "Target Boost A").unwrap();

    let x_axis = tb_a
        .axes
        .iter()
        .find(|a| a.kind == Some(romraider_defs::TableKind::XAxis))
        .unwrap();
    let scaling  = &x_axis.scalings[0]; // inline scaling: x/.84
    let compiled = scaling.compile().expect("compiles");

    // 0.84 в стиле throttle: byte=84 → real ≈ 100.
    let real = compiled.to_real(84.0);
    assert!((real - 100.0).abs() < 1e-9);
}
