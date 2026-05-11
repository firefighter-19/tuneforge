//! Парсер сырого XML в [`RomsDocument`].
//!
//! Обёртка над `quick-xml::de`. Содержит три точки входа — для строки,
//! `Read`-источника и пути к файлу — и заворачивает ошибки в [`DefError`].

use std::io::Read;
use std::path::Path;

use crate::ecu::RomsDocument;
use crate::error::{DefError, DefResult};

pub fn parse_str(xml: &str) -> DefResult<RomsDocument> {
    quick_xml::de::from_str(xml).map_err(DefError::from)
}

pub fn parse_reader<R: Read>(reader: R) -> DefResult<RomsDocument> {
    let buf_reader = std::io::BufReader::new(reader);
    quick_xml::de::from_reader(buf_reader).map_err(DefError::from)
}

pub fn parse_file(path: impl AsRef<Path>) -> DefResult<RomsDocument> {
    let xml = std::fs::read_to_string(path)?;
    parse_str(&xml)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Минимальный валидный документ — один пустой ROM с одним полем romid.
    #[test]
    fn parses_minimal_document() {
        let xml = r#"
            <roms>
              <rom>
                <romid>
                  <xmlid>EMPTY</xmlid>
                </romid>
              </rom>
            </roms>
        "#;
        let doc = parse_str(xml).unwrap();
        assert_eq!(doc.roms.len(), 1);
        let rom = &doc.roms[0];
        assert_eq!(rom.romid.as_ref().unwrap().xml_id.as_deref(), Some("EMPTY"));
        assert!(rom.tables.is_empty());
    }

    #[test]
    fn parses_scalingbase_attributes() {
        let xml = r##"
            <roms>
              <scalingbase name="rpm" units="RPM" expression="x" to_byte="x"
                           format="#" fineincrement="50" coarseincrement="100" />
            </roms>
        "##;
        let doc  = parse_str(xml).unwrap();
        assert_eq!(doc.scaling_bases.len(), 1);
        let s = &doc.scaling_bases[0];
        assert_eq!(s.name,             "rpm");
        assert_eq!(s.units.as_deref(), Some("RPM"));
        assert_eq!(s.expression,       "x");
        assert_eq!(s.to_byte,          "x");
        assert_eq!(s.format.as_deref(),           Some("#"));
        assert_eq!(s.fine_increment.as_deref(),   Some("50"));
        assert_eq!(s.coarse_increment.as_deref(), Some("100"));
    }

    #[test]
    fn parses_nested_table_axes() {
        let xml = r#"
            <roms>
              <rom>
                <romid><xmlid>TEST</xmlid></romid>
                <table type="3D" name="Outer" storageaddress="0x1000">
                  <table type="X Axis" name="rpm" storageaddress="0x2000" />
                  <table type="Y Axis" name="load" storageaddress="0x3000" />
                </table>
              </rom>
            </roms>
        "#;
        let doc   = parse_str(xml).unwrap();
        let table = &doc.roms[0].tables[0];
        assert_eq!(table.name.as_deref(),            Some("Outer"));
        assert_eq!(table.kind.as_deref(),            Some("3D"));
        assert_eq!(table.storage_address.as_deref(), Some("0x1000"));
        assert_eq!(table.nested.len(), 2);
        assert_eq!(table.nested[0].kind.as_deref(), Some("X Axis"));
        assert_eq!(table.nested[1].kind.as_deref(), Some("Y Axis"));
    }

    #[test]
    fn parses_inline_and_by_ref_scalings() {
        let xml = r#"
            <roms>
              <rom>
                <romid><xmlid>TEST</xmlid></romid>
                <table type="2D" name="t">
                  <scaling base="rpm" />
                  <scaling units="%" expression="x/.84" to_byte="x*.84" format="0.0" />
                </table>
              </rom>
            </roms>
        "#;
        let doc      = parse_str(xml).unwrap();
        let scalings = &doc.roms[0].tables[0].scalings;
        assert_eq!(scalings.len(), 2);
        assert_eq!(scalings[0].base.as_deref(), Some("rpm"));
        assert!(scalings[0].expression.is_none());
        assert_eq!(scalings[1].expression.as_deref(), Some("x/.84"));
        assert_eq!(scalings[1].units.as_deref(),      Some("%"));
    }

    #[test]
    fn parses_static_axis_data_labels() {
        let xml = r#"
            <roms>
              <rom>
                <romid><xmlid>T</xmlid></romid>
                <table>
                  <table type="Static Y Axis" name="ranges" sizey="3">
                    <data>Range A</data>
                    <data>Range B</data>
                    <data>Range C</data>
                  </table>
                </table>
              </rom>
            </roms>
        "#;
        let doc  = parse_str(xml).unwrap();
        let axis = &doc.roms[0].tables[0].nested[0];
        assert_eq!(axis.data, vec!["Range A", "Range B", "Range C"]);
    }

    #[test]
    fn rejects_malformed_xml() {
        let err = parse_str("<roms><rom").unwrap_err();
        assert!(matches!(err, DefError::Xml(_)));
    }

    #[test]
    fn parses_rom_with_base_reference() {
        let xml = r#"
            <roms>
              <rom>
                <romid><xmlid>BASE</xmlid></romid>
              </rom>
              <rom base="BASE">
                <romid>
                  <xmlid>CHILD</xmlid>
                  <ecuid>2F12795606</ecuid>
                </romid>
              </rom>
            </roms>
        "#;
        let doc = parse_str(xml).unwrap();
        assert_eq!(doc.roms.len(), 2);
        assert_eq!(doc.roms[1].base.as_deref(), Some("BASE"));
        assert_eq!(
            doc.roms[1].romid.as_ref().unwrap().ecu_id.as_deref(),
            Some("2F12795606")
        );
    }

    #[test]
    fn parses_switch_table_with_states() {
        let xml = r##"
            <roms>
              <rom>
                <romid><xmlid>R</xmlid></romid>
                <table type="Switch" name="Iridium Spark" storageaddress="0x1234" size="1">
                  <state name="On"  data="01"/>
                  <state name="Off" data="00"/>
                  <state name="Maybe" data="42"/>
                </table>
              </rom>
            </roms>
        "##;
        let doc = parse_str(xml).unwrap();
        let t   = &doc.roms[0].tables[0];
        assert_eq!(t.kind.as_deref(), Some("Switch"));
        assert_eq!(t.states.len(),    3);
        assert_eq!(t.states[0].name, "On");
        assert_eq!(t.states[0].data, "01");
        assert_eq!(t.states[0].data_bytes().unwrap(), vec![0x01]);
        assert_eq!(t.states[2].data_bytes().unwrap(), vec![0x42]);
    }

    #[test]
    fn parses_bitwise_switch_with_bits() {
        let xml = r##"
            <roms>
              <rom>
                <romid><xmlid>R</xmlid></romid>
                <table type="BitwiseSwitch" name="Engine Flags" storageaddress="0xABCD">
                  <bit name="AC Enabled" position="0"/>
                  <bit name="Cruise"      position="3"/>
                  <bit name="Sport Mode"  position="7"/>
                </table>
              </rom>
            </roms>
        "##;
        let doc = parse_str(xml).unwrap();
        let t   = &doc.roms[0].tables[0];
        assert_eq!(t.kind.as_deref(), Some("BitwiseSwitch"));
        assert_eq!(t.bits.len(), 3);
        assert_eq!(t.bits[0].bit_position().unwrap(), 0);
        assert_eq!(t.bits[1].bit_position().unwrap(), 3);
        assert_eq!(t.bits[2].bit_position().unwrap(), 7);
    }

    #[test]
    fn switch_state_data_bytes_handles_edge_cases() {
        use crate::ecu::SwitchState;
        let mk = |s: &str| SwitchState { name: "n".into(), data: s.into() };
        assert_eq!(mk("01").data_bytes().unwrap(),   vec![0x01]);
        assert_eq!(mk("0x0A").data_bytes().unwrap(), vec![0x0A]);
        assert_eq!(mk("A").data_bytes().unwrap(),    vec![0x0A]); // нечётная длина — pad
        assert_eq!(mk("0102").data_bytes().unwrap(), vec![0x01, 0x02]);
        assert_eq!(mk("  0F  ").data_bytes().unwrap(), vec![0x0F]); // trim
        assert!(mk("XY").data_bytes().is_err());
    }

    #[test]
    fn lookup_helpers_find_by_xmlid_and_scalingbase_name() {
        let xml = r#"
            <roms>
              <scalingbase name="rpm" expression="x" to_byte="x" />
              <rom>
                <romid><xmlid>A</xmlid></romid>
              </rom>
              <rom>
                <romid><xmlid>B</xmlid></romid>
              </rom>
            </roms>
        "#;
        let doc = parse_str(xml).unwrap();
        assert!(doc.find_rom_by_xml_id("A").is_some());
        assert!(doc.find_rom_by_xml_id("Z").is_none());
        assert!(doc.find_scaling_base("rpm").is_some());
        assert!(doc.find_scaling_base("nope").is_none());
    }
}
