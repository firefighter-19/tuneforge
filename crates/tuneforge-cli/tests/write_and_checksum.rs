//! End-to-end tests for the headless ROM-editing commands `write-table` and
//! `fix-checksum`: run the real binary, then inspect the produced `.bin` bytes.
//!
//! Self-contained (no extra deps): a tiny synthetic def with a `boost` table
//! (scaling `x*0.5`, inverse `x*2`) protected by a `Checksum Fix` region that
//! covers it, so we can assert both the written values AND the recomputed
//! Subaru checksum.

use std::path::PathBuf;
use std::process::Command;

const DEF: &str = r#"
<roms>
  <rom>
    <romid><xmlid>TEST</xmlid></romid>
    <table type="Switch" name="Checksum Fix" sizey="12" storageaddress="0x00">
      <state name="on" data="00 00 00 00 00 00 00 00 5A A5 A5 5A"/>
    </table>
    <table type="2D" name="boost" storagetype="uint16" endian="big" sizex="4" storageaddress="0x10">
      <scaling units="psi" expression="x*0.5" to_byte="x*2" format="0.0"/>
    </table>
  </rom>
</roms>
"#;

fn tmp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// ROM layout (0x18 bytes): [0x00] checksum entry (start,end,diff),
/// [0x10] boost table (4×uint16). The checksum region is [0x10, 0x17] inclusive
/// (the whole 8-byte boost table). Boost bytes `00 14 00 28 00 3C 00 50`
/// (= real 10/20/30/40 psi) → 2 BE words summing to 0x00500078, so the correct
/// diff = CHECK_TOTAL(0x5AA5A55A) - 0x00500078 = 0x5A55A4E2.
const EXPECTED_BOOST: [u8; 8] = [0x00, 0x14, 0x00, 0x28, 0x00, 0x3C, 0x00, 0x50];
const EXPECTED_DIFF: [u8; 4] = [0x5A, 0x55, 0xA4, 0xE2];

fn base_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x18];
    rom[0x00..0x04].copy_from_slice(&0x10u32.to_be_bytes()); // checksum region start
    rom[0x04..0x08].copy_from_slice(&0x17u32.to_be_bytes()); // ...end (inclusive)
    rom
}

#[test]
fn write_table_applies_scaling_and_fixes_checksum() {
    let def = tmp("wt_def.xml");
    let rom = tmp("wt_rom.bin");
    let vals = tmp("wt_vals.txt");
    let out = tmp("wt_out.bin");
    std::fs::write(&def, DEF).unwrap();
    std::fs::write(&rom, base_rom()).unwrap(); // boost + diff both zero
    std::fs::write(&vals, "10 20 30 40").unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_tuneforge"))
        .args([
            "write-table",
            rom.to_str().unwrap(),
            "--def",
            def.to_str().unwrap(),
            "--rom-id",
            "TEST",
            "--table",
            "boost",
            "--values-file",
            vals.to_str().unwrap(),
            "--output",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("run tuneforge");
    assert!(status.success(), "write-table should exit 0");

    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[0x10..0x18], &EXPECTED_BOOST, "boost values written");
    assert_eq!(
        &bytes[0x08..0x0C],
        &EXPECTED_DIFF,
        "checksum recomputed after edit"
    );
}

#[test]
fn fix_checksum_repairs_stale_diff() {
    let def = tmp("fc_def.xml");
    let rom = tmp("fc_rom.bin");
    let out = tmp("fc_out.bin");
    std::fs::write(&def, DEF).unwrap();
    // Boost already set to the target values, but diff is stale (zero).
    let mut bytes = base_rom();
    bytes[0x10..0x18].copy_from_slice(&EXPECTED_BOOST);
    std::fs::write(&rom, bytes).unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_tuneforge"))
        .args([
            "fix-checksum",
            rom.to_str().unwrap(),
            "--def",
            def.to_str().unwrap(),
            "--rom-id",
            "TEST",
            "--output",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("run tuneforge");
    assert!(status.success(), "fix-checksum should exit 0");

    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(
        &bytes[0x08..0x0C],
        &EXPECTED_DIFF,
        "stale checksum repaired"
    );
}
