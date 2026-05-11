//! Интеграционные тесты на реальном `log_defs.xml` из апстрима (79 KB,
//! ~470 ECU + ~150 параметров шаблона + 67 alt-адресов).

use std::path::PathBuf;

use romraider_defs::{parse_log_file, LoggerDocument};

fn load() -> LoggerDocument {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/log_defs.xml");
    parse_log_file(&path).unwrap_or_else(|e| panic!("parsing {path:?}: {e}"))
}

#[test]
fn top_level_counts_match_upstream_snapshot() {
    let doc = load();
    // 5 базовых конверсий из ecu_tools.
    assert_eq!(doc.ecu_tools.convert_factors.len(), 5);
    // Один протокол: SSM.
    assert_eq!(doc.logprotocols.logprotocols.len(), 1);
    let p = &doc.logprotocols.logprotocols[0];
    assert_eq!(p.kind, "SSM");
    assert_eq!(p.default.as_deref(), Some("ssmbase"));
    // Множество ECU: 3 шаблона + ~много концертных.
    assert!(p.ecus.len() > 100, "expected many ECUs, got {}", p.ecus.len());
}

#[test]
fn convert_factors_have_expected_metrics() {
    let doc = load();
    let factors: Vec<_> = doc
        .ecu_tools
        .convert_factors
        .iter()
        .map(|f| (f.kind.as_str(), f.metric.as_str()))
        .collect();
    // Совпадает с шапкой апстрим-файла:
    // afr/AFR, temp/F, press/Bar, speed/mph, inj/%
    assert!(factors.iter().any(|(k, m)| *k == "afr"   && *m == "AFR"));
    assert!(factors.iter().any(|(k, m)| *k == "temp"  && *m == "F"));
    assert!(factors.iter().any(|(k, m)| *k == "press" && *m == "Bar"));
    assert!(factors.iter().any(|(k, m)| *k == "speed" && *m == "mph"));
    assert!(factors.iter().any(|(k, m)| *k == "inj"   && *m == "%"));
}

#[test]
fn ssmbase_template_has_engine_speed_parameter() {
    let doc = load();
    let proto = &doc.logprotocols.logprotocols[0];
    let ssmbase = proto
        .ecus
        .iter()
        .find(|e| e.kind.as_deref() == Some("ssmbase"))
        .expect("ssmbase template present");
    assert!(ssmbase.is_template());

    let rpm = ssmbase
        .find_parameter("Engine Speed")
        .expect("Engine Speed in ssmbase");
    assert_eq!(rpm.offset,               "#000E");
    assert_eq!(rpm.storage_type.as_deref(), Some("uint16"));
    assert_eq!(rpm.expr.as_deref(),      Some("[value]/4"));
    assert_eq!(rpm.metric.as_deref(),    Some("RPM"));
    assert_eq!(rpm.bit.as_deref(),       Some("1"));
    assert_eq!(rpm.byte.as_deref(),      Some("1"));
}

#[test]
fn parameter_with_type_links_to_convert_factor() {
    // "Manifold Absolute Pressure" имеет type="press" — должно бить с
    // <convert_factor type="press">.
    let doc   = load();
    let proto = &doc.logprotocols.logprotocols[0];
    let base  = proto.ecus.iter().find(|e| e.kind.as_deref() == Some("ssmbase")).unwrap();
    let map   = base.find_parameter("Manifold Absolute Pressure").unwrap();
    assert_eq!(map.kind.as_deref(), Some("press"));

    let factor = doc.find_convert_factor("press").expect("press convert_factor");
    assert_eq!(factor.metric, "Bar");
    assert!(factor.expr.contains("[value]"));
}

#[test]
fn ssmbase16_extends_ssmbase_and_has_alts() {
    // ssmbase16 шаблон с include="ssmbase" + параметры с <alt>.
    let doc   = load();
    let proto = &doc.logprotocols.logprotocols[0];
    let ssmbase16 = proto
        .ecus
        .iter()
        .find(|e| e.kind.as_deref() == Some("ssmbase16"))
        .expect("ssmbase16 template present");
    assert_eq!(ssmbase16.include.as_deref(), Some("ssmbase"));

    let am = ssmbase16
        .find_parameter("Advance Multiplier")
        .expect("Advance Multiplier in ssmbase16");
    // У этого параметра в апстриме 5 альтернативных адресов.
    assert_eq!(am.alts.len(), 5);
    assert!(am.alts.iter().all(|a| a.id.starts_with("Advance Multiplier (")));
}

#[test]
fn ssmbase32_has_float_alternates() {
    // ssmbase32 содержит float-альты — проверим, что storage_type сохраняется.
    let doc   = load();
    let proto = &doc.logprotocols.logprotocols[0];
    let ssmbase32 = proto
        .ecus
        .iter()
        .find(|e| e.kind.as_deref() == Some("ssmbase32"))
        .expect("ssmbase32 template present");

    let am = ssmbase32.find_parameter("Advance Multiplier").unwrap();
    let float_count = am.alts.iter().filter(|a| a.storage_type.as_deref() == Some("float")).count();
    assert!(float_count > 5, "expected several float alternates, got {float_count}");
}

#[test]
fn concrete_ecu_can_be_found_by_hex_id() {
    let doc = load();
    let ecu = doc.find_ecu("1644500305").expect("MY99 Impreza AE800 present");
    assert_eq!(ecu.name,                "MY99/00 Impreza 2.0 Turbo/WRX/GT (EURO)");
    assert_eq!(ecu.kind.as_deref(),     Some("AE800"));
    assert_eq!(ecu.include.as_deref(),  Some("ssmbase16"));
    assert!(ecu.parameters.is_empty());
}

#[test]
fn resolve_ecu_merges_include_chain() {
    // На реальном log_defs: A2WC522S-эквивалент → ssmbase16 → ssmbase.
    // Берём первый concrete-ECU (есть `include="ssmbase16"`) и проверяем,
    // что после резолва он наследует все 156 ssmbase-параметров + 2 from ssmbase16.
    let doc = load();
    let any_concrete_id = doc
        .logprotocols
        .logprotocols
        .iter()
        .flat_map(|p| p.ecus.iter())
        .find(|e| !e.is_template())
        .and_then(|e| {
            if e.include.as_deref() == Some("ssmbase16") {
                Some(e.id.clone())
            } else {
                None
            }
        })
        .expect("expected at least one ssmbase16-based concrete ECU");

    let resolved = doc
        .resolve_ecu(&any_concrete_id)
        .expect("resolve_ecu should succeed");
    assert_eq!(resolved.id, any_concrete_id);
    // ssmbase (156) + ssmbase16 (2 уникальных) = 158
    assert!(
        resolved.parameters.len() >= 158,
        "expected >=158 params, got {}",
        resolved.parameters.len()
    );

    // Engine Speed точно должен быть в наследовании.
    let rpm = resolved
        .find_parameter("Engine Speed")
        .expect("Engine Speed inherited");
    assert_eq!(rpm.offset, "#000E");
}

#[test]
fn compile_log_parameter_engine_speed_evaluates() {
    // Engine Speed формула: [value]/4 → byte 4000 → 1000 RPM.
    let doc = load();
    let base = doc.find_ecu("base").unwrap();
    let rpm  = base.find_parameter("Engine Speed").unwrap();
    let c    = rpm.compile().unwrap();
    assert_eq!(c.address.raw(),    0x000E);
    assert_eq!(c.storage_type,     romraider_defs::StorageType::UInt16);
    assert!((c.evaluate(4000.0) - 1000.0).abs() < 1e-9);
    assert!((c.evaluate(0.0)    -    0.0).abs() < 1e-9);
}

#[test]
fn total_template_parameters_match_snapshot() {
    // Smoke-test: суммарно во всех шаблонах должно быть много параметров.
    let doc   = load();
    let proto = &doc.logprotocols.logprotocols[0];
    let template_params: usize = proto
        .ecus
        .iter()
        .filter(|e| e.is_template())
        .map(|e| e.parameters.len())
        .sum();
    assert!(template_params > 100, "expected 100+ template params, got {template_params}");
    let all_alts: usize = proto
        .ecus
        .iter()
        .filter(|e| e.is_template())
        .flat_map(|e| e.parameters.iter())
        .map(|p| p.alts.len())
        .sum();
    assert!(all_alts > 50, "expected 50+ alts across templates, got {all_alts}");
}
