#!/usr/bin/env python3
"""
Извлекает Subaru DTC-коды (P0xxx/P1xxx/P2xxx) из upstream
RomRaider `ecu_defs.xml` и генерирует Rust-источник для
`crates/romraider-kernel/src/dtc_db.rs`.

Использование:
    python3 tools/extract_dtc_db.py \
        /Applications/RomRaider/definitions/ecu_defs.xml \
        > crates/romraider-kernel/src/dtc_db.rs

Что извлекается:
- Все `<table name="(PXXXX) DESCRIPTION ...">` блоки — это canonical
  Subaru DTC list, поддерживаемый сообществом с 2009 года.
- Когда один и тот же DTC встречается у разных ROM-ов с разными
  описаниями, берём наиболее длинное (обычно самое информативное).

Augmentation: добавляем критичные generic SAE J2012 коды (P07xx
transmission, U0xxx network) которые не входят в Subaru-specific
list но могут приходить от TCM / других ECU на CAN-шине.
"""

import json
import re
import sys


def extract_subaru(xml_path):
    text = open(xml_path).read()
    pattern = re.compile(r'name="\(([PCBU]\d{4})\)\s+([^"]+)"')
    dtcs = {}
    for m in pattern.finditer(text):
        code, name = m.group(1), m.group(2).strip()
        if code not in dtcs or len(name) > len(dtcs[code]):
            dtcs[code] = name
    return dtcs


GENERIC_EXTRAS = {
    # Transmission Control Module — P07xx series
    "P0700": "TRANSMISSION CONTROL SYSTEM (MIL REQUEST)",
    "P0701": "TRANSMISSION CONTROL SYSTEM RANGE/PERFORMANCE",
    "P0702": "TRANSMISSION CONTROL SYSTEM ELECTRICAL",
    "P0703": "TORQUE CONVERTER/BRAKE SWITCH B CIRCUIT",
    "P0704": "CLUTCH SWITCH INPUT CIRCUIT MALFUNCTION",
    "P0705": "TRANSMISSION RANGE SENSOR CIRCUIT MALFUNCTION",
    "P0706": "TRANSMISSION RANGE SENSOR CIRCUIT RANGE/PERFORMANCE",
    "P0710": "TRANSMISSION FLUID TEMPERATURE SENSOR CIRCUIT",
    "P0715": "INPUT/TURBINE SPEED SENSOR A CIRCUIT MALFUNCTION",
    "P0720": "OUTPUT SHAFT SPEED SENSOR CIRCUIT",
    "P0730": "INCORRECT GEAR RATIO",
    "P0740": "TORQUE CONVERTER CLUTCH CIRCUIT MALFUNCTION",
    "P0750": "SHIFT SOLENOID A MALFUNCTION",
    "P0755": "SHIFT SOLENOID B MALFUNCTION",
    "P0760": "SHIFT SOLENOID C MALFUNCTION",
    # Network / U codes — для CAN-bus issues
    "U0073": "CONTROL MODULE COMMUNICATION BUS OFF",
    "U0101": "LOST COMMUNICATION WITH TCM",
    "U0121": "LOST COMMUNICATION WITH ANTI-LOCK BRAKE SYSTEM",
    "U0140": "LOST COMMUNICATION WITH BODY CONTROL MODULE",
    "U0155": "LOST COMMUNICATION WITH INSTRUMENT PANEL CLUSTER",
}


def to_title_case(s):
    """ALL CAPS → Title Case, preserving short acronyms (B1, MAF, ECU)."""
    out = []
    for w in s.split():
        if w.isdigit() or (len(w) <= 2 and w.isupper()):
            out.append(w)
        else:
            out.append(w.capitalize())
    return " ".join(out)


def emit_rust(dtcs, subaru_count):
    print("//! SAE J2012 + Subaru-specific Diagnostic Trouble Code database.")
    print("//!")
    print("//! Извлечено из upstream RomRaider `ecu_defs.xml` (Subaru table-names")
    print(f"//! в формате `(P0420) ...`) — {subaru_count} Subaru-specific")
    print("//! codes — плюс key generic transmission/network (U) codes из SAE J2012")
    print("//! для machines где TCM/BCM шлют коды.")
    print("//!")
    print("//! Регенерация: `python3 tools/extract_dtc_db.py <ecu_defs.xml>`.")
    print()
    print("/// Sorted (code, description) pairs. Lookup через [`dtc_lookup`]")
    print("/// делается через `binary_search_by_key` за O(log N).")
    print("pub const DTC_DATABASE: &[(&str, &str)] = &[")
    for code in sorted(dtcs):
        desc = to_title_case(dtcs[code]).replace('"', '\\"')
        print(f'    ("{code}", "{desc}"),')
    print("];")
    print()
    print("/// Найти описание DTC-кода (`P0301`, `C1234`, etc). Возвращает `None`")
    print("/// если код не известен — caller отображает просто номер.")
    print("#[must_use]")
    print("pub fn dtc_lookup(code: &str) -> Option<&'static str> {")
    print("    DTC_DATABASE")
    print("        .binary_search_by_key(&code, |(c, _)| c)")
    print("        .ok()")
    print("        .map(|i| DTC_DATABASE[i].1)")
    print("}")
    print()
    print("#[cfg(test)]")
    print("mod tests {")
    print("    use super::*;")
    print()
    print("    #[test]")
    print("    fn database_is_sorted() {")
    print("        for w in DTC_DATABASE.windows(2) {")
    print('            assert!(w[0].0 < w[1].0, "DTC_DATABASE must be sorted: {} >= {}", w[0].0, w[1].0);')
    print("        }")
    print("    }")
    print()
    print("    #[test]")
    print("    fn lookup_known_codes() {")
    print('        assert!(dtc_lookup("P0030").is_some());')
    print('        assert!(dtc_lookup("P0700").is_some());')
    print('        assert!(dtc_lookup("P0420").is_some());')
    print("    }")
    print()
    print("    #[test]")
    print("    fn lookup_unknown_returns_none() {")
    print('        assert_eq!(dtc_lookup("P9999"), None);')
    print('        assert_eq!(dtc_lookup("XXXXX"), None);')
    print("    }")
    print()
    print("    #[test]")
    print("    fn database_size_is_reasonable() {")
    print('        assert!(DTC_DATABASE.len() >= 100, "DTC_DATABASE too small: {}", DTC_DATABASE.len());')
    print("    }")
    print("}")


def main():
    if len(sys.argv) != 2:
        print("usage: extract_dtc_db.py <ecu_defs.xml>", file=sys.stderr)
        sys.exit(1)
    subaru = extract_subaru(sys.argv[1])
    subaru_count = len(subaru)
    combined = dict(subaru)
    for k, v in GENERIC_EXTRAS.items():
        if k not in combined:
            combined[k] = v
    emit_rust(combined, subaru_count)


if __name__ == "__main__":
    main()
