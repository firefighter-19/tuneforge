use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use romraider_core::bytes;
use romraider_defs::{RomDefinition, RomsDocument};
use romraider_io::serial::{SerialConfig, SerialTransport};
use romraider_io::Transport;
use romraider_protocol::ssm::{self, EcuInitResponse};
use romraider_rom::RomImage;

#[derive(Parser)]
#[command(name = "romraider", version, about = "romraider-rs headless tools")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Перечислить доступные последовательные порты.
    Ports,

    /// Открыть порт, отправить SSM ECU-Init и распечатать ответ.
    SsmInit {
        #[arg(short, long)]
        port: String,
        #[arg(short, long, default_value_t = 4800)]
        baud: u32,
        #[arg(long, default_value_t = 1500)]
        timeout_ms: u64,
    },

    /// Загрузить ROM-файл и вывести базовую инфу.
    InspectRom {
        path: PathBuf,
    },

    /// Прочитать XML-определение ECU (`<roms>`) и показать сводку.
    InspectDef {
        path: PathBuf,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Ports => list_ports(),
        Cmd::SsmInit { port, baud, timeout_ms } => ssm_init(&port, baud, Duration::from_millis(timeout_ms)),
        Cmd::InspectRom { path } => inspect_rom(&path),
        Cmd::InspectDef { path } => inspect_def(&path),
    }
}

fn list_ports() -> Result<()> {
    let ports = serialport::available_ports().context("listing serial ports")?;
    for p in ports {
        println!("{}\t{:?}", p.port_name, p.port_type);
    }
    Ok(())
}

fn ssm_init(port: &str, baud: u32, timeout: Duration) -> Result<()> {
    let mut cfg = SerialConfig::ssm(port);
    cfg.baud_rate = baud;
    let mut tr = SerialTransport::open(&cfg)?;
    tr.purge()?;

    let request = ssm::build_request(ssm::Command::EcuInit, &[]);
    println!("→ {}", bytes::hex_dump(&request));

    let init = ssm::ecu_init(&mut tr, timeout)
        .context("SSM ecu-init failed (check cable, ignition, baud rate)")?;
    print_ecu_init(&init);
    Ok(())
}

fn print_ecu_init(init: &EcuInitResponse) {
    println!("SSM ID:        {}", bytes::hex_dump(&init.ssm_id));
    println!("ROM ID:        {} ({})", bytes::hex_dump(&init.rom_id), printable_ascii(&init.rom_id));
    println!("Capabilities:  {} bytes", init.capabilities.len());
    if !init.capabilities.is_empty() {
        println!("               {}", bytes::hex_dump(&init.capabilities));
    }
}

fn printable_ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
        .collect()
}

fn inspect_rom(path: &PathBuf) -> Result<()> {
    let rom = RomImage::open(path)?;
    println!("path:  {}", path.display());
    println!("size:  {} bytes ({} KiB)", rom.size(), rom.size() / 1024);
    let head = rom.raw().get(..16.min(rom.size())).unwrap_or(&[]);
    println!("head:  {}", bytes::hex_dump(head));
    Ok(())
}

fn inspect_def(path: &PathBuf) -> Result<()> {
    let doc = romraider_defs::parse_file(path)
        .with_context(|| format!("parsing {}", path.display()))?;
    print_def_summary(path, &doc);
    Ok(())
}

fn print_def_summary(path: &PathBuf, doc: &RomsDocument) {
    println!("File:           {}", path.display());
    println!("Scaling bases:  {}", doc.scaling_bases.len());
    for s in &doc.scaling_bases {
        let units = s.units.as_deref().unwrap_or("-");
        println!("  {:30} units={:<12} expr={}", s.name, units, s.expression);
    }
    println!();
    println!("ROMs:           {}", doc.roms.len());
    for (i, rom) in doc.roms.iter().enumerate() {
        print_rom_summary(i + 1, rom);
    }
}

fn print_rom_summary(idx: usize, rom: &RomDefinition) {
    let id        = rom.romid.as_ref();
    let xml_id    = id.and_then(|r| r.xml_id.as_deref()).unwrap_or("?");
    let ecu_id    = id.and_then(|r| r.ecu_id.as_deref()).unwrap_or("-");
    let make      = id.and_then(|r| r.make.as_deref()).unwrap_or("-");
    let model     = id.and_then(|r| r.model.as_deref()).unwrap_or("-");
    let submodel  = id.and_then(|r| r.submodel.as_deref()).unwrap_or("-");
    let base      = rom.base.as_deref().unwrap_or("(none)");
    let n_tables  = rom.tables.len();
    let n_nested  = rom.tables.iter().map(|t| t.nested.len()).sum::<usize>();

    println!(
        "  [{idx}] xmlid={xml_id:<14} ecuid={ecu_id:<12} {make} {model} {submodel}"
    );
    println!(
        "      base={base:<14} tables={n_tables} (nested axes/inner tables: {n_nested})"
    );
}

