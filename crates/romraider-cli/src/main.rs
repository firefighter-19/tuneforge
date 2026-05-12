use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use romraider_core::{bytes, Address};
use romraider_defs::{
    resolve, LogParameter, LoggerDocument, LoggerEcu, ResolvedRom, ResolvedTable, RomDefinition,
    RomsDocument,
};
use romraider_io::serial::{SerialConfig, SerialTransport};
use romraider_io::tactrix::{TactrixConfig, TactrixTransport};
use romraider_io::Transport;
use romraider_logger::{LoggerSession, SessionConfig};
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

    /// Открыть канал, отправить SSM ECU-Init и распечатать ответ.
    ///
    /// По умолчанию использует SerialTransport (требует `--port`). С флагом
    /// `--tactrix` подключается к Openport 2.0 через USB-bulk (libusb).
    SsmInit {
        #[arg(short, long, default_value = "")]
        port: String,
        #[arg(short, long, default_value_t = 4800)]
        baud: u32,
        #[arg(long, default_value_t = 1500)]
        timeout_ms: u64,

        /// Использовать Tactrix Openport 2.0 (USB-bulk) вместо serial-порта.
        /// `--port` в этом режиме игнорируется.
        #[arg(long)]
        tactrix: bool,
    },

    /// Загрузить ROM-файл и вывести базовую инфу.
    InspectRom {
        path: PathBuf,
    },

    /// Дамп прошивки с ECU через SSM `ReadBlock` (0xA0) в `.bin`-файл.
    /// Адресный диапазон и размер зависят от ECU — для Subaru SH7055 обычно
    /// `--start 0 --length 524288` (512 KiB).
    DumpRom {
        #[arg(short, long, default_value = "")]
        port: String,
        #[arg(short, long, default_value_t = 4800)]
        baud: u32,

        /// Начальный адрес (`0x...` или просто hex без префикса).
        #[arg(long, default_value = "0x000000")]
        start: String,

        /// Сколько байт читать (десятичное или `0x...`).
        #[arg(long)]
        length: String,

        /// Куда сохранить дамп.
        #[arg(short, long)]
        output: PathBuf,

        /// Размер одного SSM-чанка (1..=254). По умолчанию 128 — безопасно
        /// для большинства реальных ECU; больше = меньше round-trip overhead.
        #[arg(long, default_value_t = 128)]
        chunk_size: usize,

        #[arg(long, default_value_t = 2000)]
        timeout_ms: u64,

        /// Использовать Tactrix Openport 2.0 (USB-bulk) вместо serial-порта.
        /// `--port` в этом режиме игнорируется.
        #[arg(long)]
        tactrix: bool,
    },

    /// (feature `kernel-upload`) Дамп прошивки **через RAM-резидентный kernel**
    /// (npkern). Это единственный надёжный способ снять полный ROM с
    /// Subaru SH7058 ECU — прямой SSM2 ReadBlock на этих ECU блокируется
    /// анти-fuzz защитой.
    #[cfg(feature = "kernel-upload")]
    DumpRomKernel {
        /// Куда сохранить дамп.
        #[arg(short, long)]
        output: PathBuf,

        /// Целевой MCU (`sh7058` для 2007 Forester XT, `sh7055` для GD WRX/STI).
        #[arg(long, default_value = "sh7058")]
        mcu: String,

        /// Стартовый адрес дампа (выровнен по 32 байтам). По умолчанию `0x0`.
        #[arg(long, default_value = "0x000000")]
        start: String,

        /// Сколько байт дампить. `0` = весь ROM (по MCU).
        #[arg(long, default_value_t = 0)]
        length: usize,

        #[arg(long, default_value_t = 5000)]
        timeout_ms: u64,
    },

    /// Прочитать N байт по последовательным адресам через **SSM2 ReadAddresses
    /// (0xA8)** — массив отдельных адресов вместо block-read.
    PeekRom {
        /// Начальный адрес.
        #[arg(long, default_value = "0x000000")]
        start: String,

        /// Сколько байт прочитать (1..=255 за один SSM2 ReadAddresses-запрос).
        #[arg(long, default_value_t = 16)]
        count: usize,

        #[arg(long, default_value_t = 2000)]
        timeout_ms: u64,

        /// Пропустить `ssm::ecu_init` перед чтением (чистый 0xA8 на свежий ECU).
        #[arg(long)]
        skip_init: bool,

        /// Пауза (мс) между `ecu_init` и `ReadAddresses` для P3-recovery ECU.
        #[arg(long, default_value_t = 100)]
        gap_ms: u64,
    },

    /// Headless-логгер: опрашивает ECU по SSM, пишет CSV-датлог. По
    /// умолчанию длительность бесконечная — прерывайте Ctrl+C.
    Logger {
        #[arg(short, long)]
        port: String,
        #[arg(short, long, default_value_t = 4800)]
        baud: u32,

        /// `log_defs.xml` с определениями параметров.
        #[arg(long)]
        def: PathBuf,

        /// ECU `id` для резолва (template `base` или конкретный hex-ID).
        #[arg(long, default_value = "base")]
        ecu: String,

        /// Список параметров по `id`, через запятую: `--params "Engine Speed,Throttle Opening Angle"`.
        #[arg(long, value_delimiter = ',', num_args = 1..)]
        params: Vec<String>,

        /// Интервал между опросами, мс.
        #[arg(long, default_value_t = 100)]
        interval_ms: u64,

        /// Сколько секунд опрашивать; `0` = бесконечно.
        #[arg(long, default_value_t = 0)]
        duration_secs: u64,

        /// Куда писать CSV-датлог.
        #[arg(long)]
        out: PathBuf,

        #[arg(long, default_value_t = 1500)]
        timeout_ms: u64,
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
        Cmd::SsmInit { port, baud, timeout_ms, tactrix } => {
            let timeout = Duration::from_millis(timeout_ms);
            if tactrix {
                ssm_init_tactrix(timeout)
            } else if port.is_empty() {
                anyhow::bail!("--port required when --tactrix is not set");
            } else {
                ssm_init(&port, baud, timeout)
            }
        }
        Cmd::InspectRom { path } => inspect_rom(&path),
        Cmd::InspectDef { path, resolve, rom, sample_byte } => {
            inspect_def(&path, resolve, rom.as_deref(), sample_byte)
        }
        Cmd::InspectLog { path, ecu } => inspect_log(&path, ecu.as_deref()),
        Cmd::ReadTable { rom, def, rom_id, table } => {
            read_table_cmd(&rom, &def, &rom_id, &table)
        }
        Cmd::DumpRom {
            port, baud, start, length, output, chunk_size, timeout_ms, tactrix,
        } => dump_rom_cmd(
            &port, baud, &start, &length, &output, chunk_size,
            Duration::from_millis(timeout_ms), tactrix,
        ),
        #[cfg(feature = "kernel-upload")]
        Cmd::DumpRomKernel { output, mcu, start, length, timeout_ms } => dump_rom_kernel_cmd(
            &output, &mcu, &start, length, Duration::from_millis(timeout_ms),
        ),
        Cmd::PeekRom { start, count, timeout_ms, skip_init, gap_ms } => peek_rom_cmd(
            &start, count, Duration::from_millis(timeout_ms), skip_init, gap_ms,
        ),
        Cmd::Logger {
            port, baud, def, ecu, params, interval_ms, duration_secs, out, timeout_ms,
        } => logger_cmd(
            &port, baud, &def, &ecu, &params, interval_ms, duration_secs, &out,
            Duration::from_millis(timeout_ms),
        ),
    }
}

fn dump_rom_cmd(
    port:       &str,
    baud:       u32,
    start:      &str,
    length:     &str,
    output:     &PathBuf,
    chunk_size: usize,
    timeout:    Duration,
    tactrix:    bool,
) -> Result<()> {
    let start_addr = parse_int_or_hex_u32(start).with_context(|| format!("--start `{start}`"))?;
    let length_val = parse_int_or_hex_usize(length).with_context(|| format!("--length `{length}`"))?;
    if length_val == 0 {
        anyhow::bail!("--length must be > 0");
    }

    if tactrix {
        // Subaru SSM2 на 2007 Forester XT (и аналогах) держит только ОДИН
        // ReadBlock-ответ на сессию: второй и далее ECU игнорирует. Поэтому
        // для Tactrix-режима делаем «session-per-chunk» — для каждого
        // chunk заново открываем Tactrix, делаем ecu_init, читаем один блок,
        // закрываем. Медленно, но единственный надёжный путь без kernel-upload.
        dump_rom_tactrix_session_per_chunk(start_addr, length_val, chunk_size, timeout, output)
    } else {
        if port.is_empty() {
            anyhow::bail!("--port required when --tactrix is not set");
        }
        let mut cfg = SerialConfig::ssm(port);
        cfg.baud_rate = baud;
        let mut tr = SerialTransport::open(&cfg)
            .with_context(|| format!("opening serial {port}@{baud}"))?;
        tr.purge()?;
        do_dump_rom(&mut tr, start_addr, length_val, chunk_size, timeout, output)
    }
}

/// Дамп через одну USB-сессию Tactrix, но с пересозданием K-Line канала
/// (`atc`/`ato`/`atf`) перед каждым ReadBlock. Subaru SSM2 на 2007 Forester XT
/// принимает только один ReadBlock на «свежий» канал — поэтому между чанками
/// делаем `reset_channel()` + повторный `ecu_init`. USB остаётся открытым,
/// что намного дешевле, чем полный open/close цикл.
fn dump_rom_tactrix_session_per_chunk(
    start_addr: u32,
    length:     usize,
    chunk_size: usize,
    timeout:    Duration,
    output:     &PathBuf,
) -> Result<()> {
    const MAX_RETRIES:    u32      = 5;
    const COOLDOWN_AFTER: Duration = Duration::from_millis(100);
    const COOLDOWN_RETRY: Duration = Duration::from_millis(1500);

    let cfg = TactrixConfig::default();
    eprintln!(
        "Opening Tactrix Openport (VID={:#06X} PID={:#06X}, ISO-9141 + NO_CHECKSUM @ {} baud)…",
        cfg.vid, cfg.pid, cfg.baud
    );
    let mut tr = TactrixTransport::open(&cfg).context("Tactrix open failed")?;
    eprintln!("Transport: {}", tr.description());
    tr.purge()?;

    let mut out = Vec::with_capacity(length);
    let started = std::time::Instant::now();
    eprintln!(
        "Dumping {length} bytes from 0x{start_addr:06X} via Tactrix \
         (chunks of {chunk_size}, channel-reset between chunks, timeout {}ms)…",
        timeout.as_millis()
    );

    let mut last_percent = -1i32;
    let mut first_chunk = true;
    while out.len() < length {
        let remaining = length - out.len();
        let this_chunk = chunk_size.min(remaining);
        let addr = Address::new(start_addr + out.len() as u32);

        let mut attempt = 0u32;
        let data = loop {
            attempt += 1;
            // Перед каждым chunk (кроме первого) — пересоздаём K-Line channel.
            if !first_chunk {
                if let Err(e) = tr.reset_channel() {
                    eprintln!("  reset_channel failed (attempt {attempt}): {e:#}");
                    std::thread::sleep(COOLDOWN_RETRY);
                    continue;
                }
            }
            first_chunk = false;

            match read_one_chunk_via(&mut tr, addr, this_chunk, timeout) {
                Ok(d) => break d,
                Err(e) if attempt < MAX_RETRIES => {
                    eprintln!(
                        "  retry {}/{} for 0x{:06X}: {e:#}",
                        attempt, MAX_RETRIES, addr.raw()
                    );
                    std::thread::sleep(COOLDOWN_RETRY);
                }
                Err(e) => return Err(e).with_context(|| {
                    format!("chunk at 0x{:06X} failed after {MAX_RETRIES} retries", addr.raw())
                }),
            }
        };
        out.extend_from_slice(&data);

        let percent = (out.len() as i64 * 100 / length as i64) as i32;
        if percent != last_percent {
            let elapsed = started.elapsed().as_secs_f64();
            let rate    = out.len() as f64 / elapsed.max(1e-6);
            let eta_s   = (length - out.len()) as f64 / rate.max(1.0);
            eprintln!(
                "  {}/{} ({percent}%)  {rate:.1} B/s  ETA {:.0}s",
                out.len(), length, eta_s
            );
            last_percent = percent;
        }

        if out.len() < length {
            std::thread::sleep(COOLDOWN_AFTER);
        }
    }

    std::fs::write(output, &out).with_context(|| format!("writing {}", output.display()))?;
    eprintln!(
        "Done in {:.1}s. {} bytes written to {}",
        started.elapsed().as_secs_f64(), out.len(), output.display()
    );
    Ok(())
}

fn read_one_chunk_via(
    tr:      &mut TactrixTransport,
    addr:    Address,
    count:   usize,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let _init = ssm::ecu_init(tr, timeout).context("ecu_init")?;
    let data  = ssm::read_block(tr, addr, count, timeout).context("read_block")?;
    Ok(data)
}

fn do_dump_rom(
    transport:  &mut dyn romraider_io::transport::Transport,
    start_addr: u32,
    length:     usize,
    chunk_size: usize,
    timeout:    Duration,
    output:     &PathBuf,
) -> Result<()> {
    // Открываем SSM-сессию до начала дампа — `ReadBlock` без активной сессии
    // ECU может проигнорировать (особенно после тайм-аутов). Заодно
    // подтверждаем, что ECU реально отвечает.
    eprintln!("Opening SSM session (ecu_init)…");
    let init = ssm::ecu_init(transport, timeout)
        .context("SSM ecu_init failed (ignition ON? K-Line wired? ECU asleep?)")?;
    eprintln!(
        "  ECU online: ROM {} ({} cap bytes)",
        bytes::hex_dump(&init.rom_id),
        init.capabilities.len()
    );

    let started = std::time::Instant::now();
    eprintln!(
        "Dumping {length} bytes from 0x{start_addr:06X} (chunks of {chunk_size}, timeout {}ms)…",
        timeout.as_millis()
    );
    let mut last_percent = -1i32;
    let bytes = ssm::dump_rom(
        transport,
        Address::new(start_addr),
        length,
        chunk_size,
        timeout,
        |done, total| {
            let percent = (done as i64 * 100 / total as i64) as i32;
            if percent != last_percent && (percent % 5 == 0 || done == total) {
                let elapsed = started.elapsed().as_secs_f64();
                let rate    = done as f64 / elapsed.max(1e-6);
                eprintln!("  {done}/{total} ({percent}%)  {rate:.1} B/s");
                last_percent = percent;
            }
        },
    )
    .context("dump_rom failed")?;
    std::fs::write(output, &bytes).with_context(|| format!("writing {}", output.display()))?;
    eprintln!(
        "Done in {:.1}s. {} bytes written to {}",
        started.elapsed().as_secs_f64(),
        bytes.len(),
        output.display()
    );
    Ok(())
}

fn parse_int_or_hex_u32(s: &str) -> Result<u32> {
    let trimmed = s.trim();
    let stripped = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"));
    match stripped {
        Some(hex) => u32::from_str_radix(hex, 16).context("invalid hex"),
        None      => trimmed.parse::<u32>().context("invalid decimal"),
    }
}

fn parse_int_or_hex_usize(s: &str) -> Result<usize> {
    let trimmed = s.trim();
    let stripped = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"));
    match stripped {
        Some(hex) => usize::from_str_radix(hex, 16).context("invalid hex"),
        None      => trimmed.parse::<usize>().context("invalid decimal"),
    }
}

fn logger_cmd(
    port:          &str,
    baud:          u32,
    def_path:      &PathBuf,
    ecu_id:        &str,
    param_ids:     &[String],
    interval_ms:   u64,
    duration_secs: u64,
    out_path:      &PathBuf,
    timeout:       Duration,
) -> Result<()> {
    if param_ids.is_empty() {
        anyhow::bail!("at least one --params id is required");
    }

    // 1. Парс log_defs.xml + резолв ECU (через include-цепочку).
    let doc = romraider_defs::parse_log_file(def_path)
        .with_context(|| format!("parsing {}", def_path.display()))?;
    let resolved = doc
        .resolve_ecu(ecu_id)
        .with_context(|| format!("resolving log-ECU `{ecu_id}`"))?;
    tracing::info!(ecu = %ecu_id, total_params = resolved.parameters.len(), "log-ECU resolved");

    // 2. Скомпилировать выбранные параметры.
    let mut session = LoggerSession::new(SessionConfig {
        timeout,
        ..SessionConfig::default()
    });
    for id in param_ids {
        let p = resolved
            .find_parameter(id)
            .ok_or_else(|| anyhow::anyhow!("parameter `{id}` not found in ECU `{ecu_id}`"))?;
        let compiled = p
            .compile()
            .with_context(|| format!("compiling parameter `{id}`"))?;
        session.subscribe(compiled);
    }

    // 3. CSV-датлог.
    let mut datalog = romraider_logger::datalog::DatalogWriter::create(out_path)
        .with_context(|| format!("opening {}", out_path.display()))?;

    // 4. Открыть serial.
    let mut cfg = SerialConfig::ssm(port);
    cfg.baud_rate = baud;
    let mut transport = SerialTransport::open(&cfg)
        .with_context(|| format!("opening serial {port}@{baud}"))?;
    transport.purge()?;

    // 5. Loop.
    let interval = Duration::from_millis(interval_ms);
    let deadline = if duration_secs > 0 {
        Some(std::time::Instant::now() + Duration::from_secs(duration_secs))
    } else {
        None
    };
    let mut count = 0u64;
    eprintln!("Starting log to {} (interval {}ms)…", out_path.display(), interval_ms);
    loop {
        let started = std::time::Instant::now();
        match session.poll_once(&mut transport) {
            Ok(sample) => {
                datalog.write_sample(&sample)?;
                count += 1;
                if count % 10 == 0 {
                    eprintln!("  {count} samples written");
                }
            }
            Err(e) => {
                eprintln!("poll error: {e}");
            }
        }
        if let Some(d) = deadline {
            if std::time::Instant::now() >= d {
                break;
            }
        }
        if let Some(rem) = interval.checked_sub(started.elapsed()) {
            std::thread::sleep(rem);
        }
    }
    datalog.flush()?;
    eprintln!("Done. {count} samples written to {}", out_path.display());
    Ok(())
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

fn ssm_init_tactrix(timeout: Duration) -> Result<()> {
    let mut tr = open_tactrix()?;
    let request = ssm::build_request(ssm::Command::EcuInit, &[]);
    println!("→ {}", bytes::hex_dump(&request));

    let init = ssm::ecu_init(&mut tr, timeout)
        .context("SSM ecu-init via Tactrix failed (ignition ON? K-Line wired?)")?;
    print_ecu_init(&init);
    Ok(())
}

fn open_tactrix() -> Result<TactrixTransport> {
    let cfg = TactrixConfig::default();
    eprintln!(
        "Opening Tactrix Openport (VID={:#06X} PID={:#06X}, ISO-9141 + NO_CHECKSUM @ {} baud)…",
        cfg.vid, cfg.pid, cfg.baud
    );
    let mut tr = TactrixTransport::open(&cfg)
        .context("Tactrix open failed (check USB cable + Openport firmware)")?;
    eprintln!("Transport: {}", tr.description());
    tr.purge()?;
    Ok(tr)
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
        OneD          => "1D",
        TwoD          => "2D",
        ThreeD        => "3D",
        XAxis         => "X-Axis",
        YAxis         => "Y-Axis",
        StaticXAxis   => "SX-Axis",
        StaticYAxis   => "SY-Axis",
        Switch        => "Switch",
        BitwiseSwitch => "BSwitch",
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

#[cfg(feature = "kernel-upload")]
fn dump_rom_kernel_cmd(
    output:     &PathBuf,
    mcu:        &str,
    start:      &str,
    length:     usize,
    timeout:    Duration,
) -> Result<()> {
    use romraider_kernel::{dump_rom_via_kernel, KernelDumpConfig, McuFamily};

    let mcu_family = match mcu.to_ascii_lowercase().as_str() {
        "sh7058" | "7058" => McuFamily::Sh7058,
        "sh7055" | "7055" => McuFamily::Sh7055,
        other => anyhow::bail!("unknown --mcu `{other}` (expected sh7058 or sh7055)"),
    };
    let start_addr = parse_int_or_hex_u32(start).with_context(|| format!("--start `{start}`"))?;
    let length_val = if length == 0 {
        mcu_family.rom_size()
    } else {
        length
    };

    // Возвращаемся к ISO9141 + NO_CHECKSUM (как в ssm-init): эмпирически в этом
    // mode Tactrix пропускает raw K-Line traffic, что позволяет нам послать и
    // SSM-frame (wake-up), и KWP2000-frame (kernel-upload). ISO14230-mode Tactrix
    // требует свой fast-init handshake которого мы не делаем — и ECU молчит на
    // всё в таком случае.
    let mut tr = open_tactrix()?;
    let cfg = KernelDumpConfig {
        mcu:        mcu_family,
        start_addr,
        length:     length_val,
        fast_baud:  None, // TODO: после Tactrix baud-switch API
        timeout,
    };
    eprintln!(
        "Dumping {} bytes from 0x{:06X} via kernel-upload ({:?}); это занимает ~{} мин на 4800 бод…",
        length_val, start_addr, mcu_family,
        // Грубая оценка: ~480 байт/сек после kernel handshake (без baud-switch).
        (length_val as f64 / 480.0 / 60.0).max(1.0).ceil() as i32,
    );

    let started = std::time::Instant::now();
    let mut last_percent = -1i32;
    let bytes = dump_rom_via_kernel(&mut tr, &cfg, |done, total| {
        let percent = (done as i64 * 100 / total.max(1) as i64) as i32;
        if percent != last_percent {
            let elapsed = started.elapsed().as_secs_f64();
            let rate    = done as f64 / elapsed.max(1e-6);
            let eta_s   = (total - done) as f64 / rate.max(1.0);
            eprintln!(
                "  {}/{} ({percent}%)  {rate:.0} B/s  ETA {:.0}s",
                done, total, eta_s,
            );
            last_percent = percent;
        }
    })
    .context("kernel-upload dump failed")?;
    std::fs::write(output, &bytes).with_context(|| format!("writing {}", output.display()))?;
    eprintln!(
        "Done in {:.1}s. {} bytes written to {}",
        started.elapsed().as_secs_f64(), bytes.len(), output.display()
    );
    Ok(())
}

fn peek_rom_cmd(
    start:     &str,
    count:     usize,
    timeout:   Duration,
    skip_init: bool,
    gap_ms:    u64,
) -> Result<()> {
    if !(1..=255).contains(&count) {
        anyhow::bail!("--count must be 1..=255 (limit of single SSM2 ReadAddresses)");
    }
    let start_addr = parse_int_or_hex_u32(start).with_context(|| format!("--start `{start}`"))?;

    let mut tr = open_tactrix()?;

    if !skip_init {
        eprintln!("Pinging ECU via SSM ecu_init…");
        let init = ssm::ecu_init(&mut tr, timeout)
            .context("SSM ecu_init failed (ignition ON? K-Line wired?)")?;
        eprintln!("  ECU online: ROM {}", bytes::hex_dump(&init.rom_id));
        if gap_ms > 0 {
            eprintln!("Sleeping {gap_ms}ms before ReadAddresses (P3 guard)…");
            std::thread::sleep(Duration::from_millis(gap_ms));
        }
    } else {
        eprintln!("Skipping ecu_init (clean ReadAddresses on cold ECU)");
    }

    // Сгенерировать массив sequential адресов.
    let addresses: Vec<Address> = (0..count)
        .map(|i| Address::new(start_addr + i as u32))
        .collect();

    eprintln!(
        "Reading {} bytes from 0x{:06X} via SSM2 ReadAddresses (0xA8)…",
        count, start_addr,
    );
    let bytes = ssm::read_addresses(&mut tr, &addresses, 0x00, timeout)
        .context("ReadAddresses failed")?;

    eprintln!("\nResult:");
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let offset = start_addr + (i * 16) as u32;
        let hex: String = chunk
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' })
            .collect();
        println!("0x{offset:06X}  {hex:<48}  {ascii}");
    }

    // Если первые 8 байт = `00 00 0B 68 FF FF BF A0` — это SH7058 boot vector,
    // **рабочий ROM dump** через 0xA8. Если все 0xFF — anti-fuzz блокирует.
    if bytes.iter().all(|&b| b == 0xFF) {
        eprintln!("\n⚠️  Все байты 0xFF — похоже, anti-fuzz блокирует 0xA8 для ROM-адресов.");
        eprintln!("    Нужен другой подход (kernel-upload через замыкание Read Memory).");
    } else {
        eprintln!("\n✅ ECU вернул реальные данные. Можно дампить весь ROM через 0xA8.");
    }
    Ok(())
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

