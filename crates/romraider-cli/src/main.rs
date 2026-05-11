use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use romraider_core::bytes;
use romraider_defs::{
    resolve, LogParameter, LoggerDocument, LoggerEcu, ResolvedRom, ResolvedTable, RomDefinition,
    RomsDocument,
};
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

    /// Загрузить ROM-файл вместе с XML-определением и распечатать значения
    /// одной таблицы (raw + scaled, с осями для 3D).
    ReadTable {
        /// Путь к бинарному файлу прошивки.
        rom: PathBuf,

        /// Путь к XML с определениями (`<roms>`).
        #[arg(long)]
        def: PathBuf,

        /// `xmlid` ROM-а внутри определения (например, `A2WC522S`).
        #[arg(long)]
        rom_id: String,

        /// Имя таблицы для чтения.
        #[arg(long)]
        table: String,
    },

    /// Прочитать `log_defs.xml` (`<ecus>`) и показать сводку.
    ///
    /// С флагом `--ecu <id>` фокусируется на одном ECU: list of параметров
    /// с offset/metric/expr.
    InspectLog {
        path: PathBuf,

        /// Сфокусироваться на одном ECU (template-id типа `ssmbase` или hex-ECU-ID).
        #[arg(long)]
        ecu: Option<String>,
    },

    /// Прочитать XML-определение ECU (`<roms>`) и показать сводку.
    ///
    /// С флагом `--resolve` дополнительно разрешает наследование (rom-base,
    /// table-base, scaling-base) и печатает разрешённые таблицы.
    InspectDef {
        path: PathBuf,

        /// Применить резолв наследования и печатать материализованные таблицы.
        #[arg(long)]
        resolve: bool,

        /// Если задано — фильтровать вывод резолва только по этому ROM (`xmlid`).
        #[arg(long)]
        rom: Option<String>,

        /// Если задано — компилировать первую scaling каждой таблицы и
        /// показать преобразование этого «байтового» значения в real-world.
        #[arg(long)]
        sample_byte: Option<f64>,
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
        Cmd::InspectDef { path, resolve, rom, sample_byte } => {
            inspect_def(&path, resolve, rom.as_deref(), sample_byte)
        }
        Cmd::InspectLog { path, ecu } => inspect_log(&path, ecu.as_deref()),
        Cmd::ReadTable { rom, def, rom_id, table } => {
            read_table_cmd(&rom, &def, &rom_id, &table)
        }
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

fn inspect_def(
    path:        &PathBuf,
    do_resolve:  bool,
    rom_filter:  Option<&str>,
    sample_byte: Option<f64>,
) -> Result<()> {
    let doc = romraider_defs::parse_file(path)
        .with_context(|| format!("parsing {}", path.display()))?;
    print_def_summary(path, &doc);
    if do_resolve {
        let resolved = resolve(&doc).context("resolving inheritance")?;
        println!();
        println!("=== RESOLVED ===");
        let it = resolved.iter().filter(|r| rom_filter.is_none_or(|f| r.xml_id == f));
        for rom in it {
            print_resolved_rom(rom, sample_byte);
        }
    }
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

fn print_resolved_rom(rom: &ResolvedRom, sample_byte: Option<f64>) {
    let ecu_id = rom.romid.ecu_id.as_deref().unwrap_or("-");
    println!();
    println!("ROM {} (ecuid={ecu_id})  tables={}", rom.xml_id, rom.tables.len());
    for t in &rom.tables {
        print_resolved_table(t, 1, sample_byte);
    }
}

fn print_resolved_table(t: &ResolvedTable, indent: usize, sample_byte: Option<f64>) {
    let pad     = "  ".repeat(indent);
    let kind    = t.kind.map_or("?", debug_kind);
    let addr    = t.storage_address.map_or_else(|| "-".into(), |a| format!("{a}"));
    let storage = t.storage_type.map_or("-", debug_storage);
    let dims = match (t.size_x, t.size_y) {
        (Some(x), Some(y)) => format!("{x}x{y}"),
        (Some(x), None)    => x.to_string(),
        (None, Some(y))    => y.to_string(),
        _ => "-".into(),
    };
    let units = t.scalings.first().and_then(|s| s.units.as_deref()).unwrap_or("");
    println!(
        "{pad}{:<6} {:<40} @ {:<10} {:<7} dims={:<8} units={}",
        kind, t.name, addr, storage, dims, units
    );

    if let (Some(byte), Some(scaling)) = (sample_byte, t.scalings.first()) {
        match scaling.compile() {
            Ok(c) => {
                let real = c.to_real(byte);
                println!("{pad}    [byte={byte} → {real:.4} {units}]");
            }
            Err(e) => {
                println!("{pad}    [scaling did not compile: {e}]");
            }
        }
    }

    for axis in &t.axes {
        print_resolved_table(axis, indent + 1, sample_byte);
    }
}

fn debug_kind(k: romraider_defs::TableKind) -> &'static str {
    use romraider_defs::TableKind::*;
    match k {
        OneD        => "1D",
        TwoD        => "2D",
        ThreeD      => "3D",
        XAxis       => "X-Axis",
        YAxis       => "Y-Axis",
        StaticXAxis => "SX-Axis",
        StaticYAxis => "SY-Axis",
    }
}

fn debug_storage(s: romraider_defs::StorageType) -> &'static str {
    use romraider_defs::StorageType::*;
    match s {
        UInt8  => "uint8",
        Int8   => "int8",
        UInt16 => "uint16",
        Int16  => "int16",
        UInt32 => "uint32",
        Int32  => "int32",
        Float  => "float",
        Hex    => "hex",
        Char   => "char",
    }
}

fn read_table_cmd(rom_path: &PathBuf, def_path: &PathBuf, rom_id: &str, table_name: &str) -> Result<()> {
    let doc = romraider_defs::parse_file(def_path)
        .with_context(|| format!("parsing {}", def_path.display()))?;
    let resolved = resolve(&doc).context("resolving inheritance")?;
    let rom_def = resolved
        .iter()
        .find(|r| r.xml_id == rom_id)
        .ok_or_else(|| anyhow::anyhow!("ROM `{rom_id}` not found in {}", def_path.display()))?;
    let table = rom_def
        .tables
        .iter()
        .find(|t| t.name == table_name)
        .ok_or_else(|| anyhow::anyhow!("table `{table_name}` not found in ROM `{rom_id}`"))?;

    let rom = RomImage::open(rom_path)
        .with_context(|| format!("opening {}", rom_path.display()))?;

    print_read_table(&rom, table)
}

fn print_read_table(rom: &RomImage, table: &ResolvedTable) -> Result<()> {
    let storage = table.storage_type.map(|s| debug_storage(s)).unwrap_or("?");
    let endian  = match table.endian {
        Some(romraider_core::Endian::Big)    => "big",
        Some(romraider_core::Endian::Little) => "little",
        None                                 => "?",
    };
    let dims = match (table.size_x, table.size_y) {
        (Some(x), Some(y)) => format!("{x}x{y}"),
        (Some(x), None)    => x.to_string(),
        (None, Some(y))    => y.to_string(),
        _ => "?".into(),
    };
    let addr = table.storage_address.map_or_else(|| "?".into(), |a| format!("{a}"));
    println!(
        "Table {} ({:?}) — {} {} {} @ {}",
        table.name,
        table.kind,
        storage,
        endian,
        dims,
        addr,
    );

    let raw = rom.read_table(table).context("reading main table cells")?;
    let scaled_units = table.scalings.first().and_then(|s| s.units.as_deref()).unwrap_or("");
    let scaling      = table.scalings.first().map(|s| s.compile()).transpose()?;

    println!();
    println!("Cells ({}):", raw.len());
    print_grid(&raw, scaling.as_ref(), table.size_x, scaled_units);

    // Оси: count берём из соответствующего размера родителя.
    for axis in &table.axes {
        print_axis(rom, axis, table.size_x, table.size_y);
    }
    Ok(())
}

fn print_grid(
    raw:      &[f64],
    scaling:  Option<&romraider_defs::CompiledScaling>,
    size_x:   Option<u32>,
    units:    &str,
) {
    let cols = size_x.map_or(raw.len(), |n| n as usize).max(1);
    for (i, &v) in raw.iter().enumerate() {
        let real = scaling.map_or(v, |c| c.to_real(v));
        if i % cols == 0 && i > 0 {
            println!();
        }
        print!("{:>10.3}  ", real);
    }
    println!();
    if !units.is_empty() {
        println!("units: {units}");
    }
}

fn print_axis(
    rom:           &RomImage,
    axis:          &ResolvedTable,
    parent_size_x: Option<u32>,
    parent_size_y: Option<u32>,
) {
    let count = match axis.kind {
        Some(romraider_defs::TableKind::XAxis) => parent_size_x,
        Some(romraider_defs::TableKind::YAxis) => parent_size_y,
        _ => None,
    };
    let Some(count) = count.map(|n| n as usize) else {
        println!("\nAxis {} ({:?}): size not derivable from parent — skip", axis.name, axis.kind);
        return;
    };

    match rom.read_cells(axis, count) {
        Ok(raw) => {
            let units = axis.scalings.first().and_then(|s| s.units.as_deref()).unwrap_or("");
            let scaling = axis
                .scalings
                .first()
                .and_then(|s| s.compile().ok());
            println!("\n{:?} {} ({} cells, units={}):", axis.kind, axis.name, count, units);
            let values: Vec<f64> = raw
                .iter()
                .map(|x| scaling.as_ref().map_or(*x, |c| c.to_real(*x)))
                .collect();
            for v in &values {
                print!("{:>10.3}  ", v);
            }
            println!();
        }
        Err(e) => {
            println!("\nAxis {} read failed: {e}", axis.name);
        }
    }
}

fn inspect_log(path: &PathBuf, ecu_filter: Option<&str>) -> Result<()> {
    let doc = romraider_defs::parse_log_file(path)
        .with_context(|| format!("parsing {}", path.display()))?;
    if let Some(id) = ecu_filter {
        let ecu = doc
            .find_ecu(id)
            .ok_or_else(|| anyhow::anyhow!("ECU `{id}` not found in {}", path.display()))?;
        print_log_ecu_detail(&doc, ecu);
    } else {
        print_log_overview(path, &doc);
    }
    Ok(())
}

fn print_log_overview(path: &PathBuf, doc: &LoggerDocument) {
    println!("File:              {}", path.display());
    println!("Convert factors:   {}", doc.ecu_tools.convert_factors.len());
    for f in &doc.ecu_tools.convert_factors {
        println!("  [{:<5}] {:<30} → {:<6} ({})", f.kind, f.name, f.metric, f.expr);
    }
    println!();
    println!("Log protocols:     {}", doc.logprotocols.logprotocols.len());
    for p in &doc.logprotocols.logprotocols {
        let templates: Vec<_>  = p.ecus.iter().filter(|e| e.is_template()).collect();
        let concrete = p.ecus.len() - templates.len();
        let template_params: usize = templates.iter().map(|e| e.parameters.len()).sum();
        println!(
            "  type={:<6}  default={:<10}  templates={:<3} concrete={:<4} template-params={}",
            p.kind,
            p.default.as_deref().unwrap_or("-"),
            templates.len(),
            concrete,
            template_params,
        );
        for t in templates {
            let inc = t.include.as_deref().unwrap_or("-");
            println!(
                "    template {:<12} include={:<10} params={}",
                t.kind.as_deref().unwrap_or("?"),
                inc,
                t.parameters.len(),
            );
        }
    }
}

fn print_log_ecu_detail(doc: &LoggerDocument, ecu: &LoggerEcu) {
    println!("ECU:        {}", ecu.id);
    println!("Name:       {}", ecu.name);
    println!("Type:       {}", ecu.kind.as_deref().unwrap_or("-"));
    println!("Include:    {}", ecu.include.as_deref().unwrap_or("-"));
    println!("Parameters: {}", ecu.parameters.len());
    if ecu.parameters.is_empty() {
        println!("  (none — naследует через include={:?})", ecu.include);
        return;
    }
    for p in &ecu.parameters {
        print_log_parameter(doc, p);
    }
}

fn print_log_parameter(doc: &LoggerDocument, p: &LogParameter) {
    let storage = p.storage_type.as_deref().unwrap_or("-");
    let metric  = p.metric.as_deref().unwrap_or("");
    let expr    = p.expr.as_deref().unwrap_or("?");
    let bb      = match (p.byte.as_deref(), p.bit.as_deref()) {
        (Some(b), Some(bt)) => format!("byte={b} bit={bt}"),
        _ => "—".into(),
    };
    let factor = p
        .kind
        .as_deref()
        .and_then(|k| doc.find_convert_factor(k))
        .map(|f| format!("  alt: {} → {}", f.expr, f.metric))
        .unwrap_or_default();

    println!(
        "  • {:<35} offset={:<10} {:<7} {:<14} expr={:<26} metric={}{}",
        p.id, p.offset, storage, bb, expr, metric, factor
    );
    for a in &p.alts {
        let st = a.storage_type.as_deref().unwrap_or("-");
        println!("      alt {:<8} {}", st, a.offset);
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

