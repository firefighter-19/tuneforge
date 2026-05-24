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
use romraider_logger::{LoggerSession, Sample, SampleValue, SessionConfig};
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

    /// **Pre-flight check для Tactrix Openport 2.0** — enumerate USB шину,
    /// показать все найденные устройства с VID `0x0403` PID `0xCC4D`. Не
    /// требует root (passive list-only). Удобно для проверки «подключён
    /// ли кабель» перед `ssm-init` / `dump-rom-can` / `logger-can`. Если
    /// показывает «not found» — Tactrix не воткнут или USB-кабель
    /// глючит (попробуй другой port). Если показывает девайс **но**
    /// `manufacturer/product = None` — sudo нужен для дальнейших
    /// операций (macOS «allow accessory» prompt мог не показаться).
    TactrixInfo,

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
    InspectRom { path: PathBuf },

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

        /// Workaround для анти-fuzz Subaru ECU (2007+, например USDM Forester
        /// XT): пересоздавать K-Line канал перед каждым ReadBlock. Очень
        /// медленно (~6 ч на 1 МБ). По умолчанию **выключено** — обычные
        /// SSM2 ECU (до 2007) принимают серию ReadBlock в одной сессии.
        #[arg(long)]
        reset_per_chunk: bool,
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

    /// (Quick-win Slice 23) Прочитать **VIN автомобиля** через OBD-II Mode 09
    /// PID 02 по **CAN bus** (ISO15765 @ 500 kbps). Это standard public OBD-II,
    /// **никакой security access** не требуется — proof-of-concept что CAN-
    /// transport через Tactrix Openport работает на Mac.
    PeekVin {
        #[arg(long, default_value_t = 2000)]
        timeout_ms: u64,
    },

    /// Тот же quick-win — прочитать **Calibration Verification Number** (CVN)
    /// через OBD-II Mode 09 PID 06. Для нашей машины должен вернуть `3C C8 84 A1`
    /// (точно совпадает с `MEML` блоком в `.srf` от EcuFlash).
    PeekCvn {
        #[arg(long, default_value_t = 2000)]
        timeout_ms: u64,
    },

    /// Прочитать **Freeze Frame** через OBD-II Mode 0x02 — snapshot
    /// всех параметров **в момент когда ECU зафиксировал triggering DTC**.
    /// Очень полезно для диагностики: видим что было «непосредственно
    /// перед» fault-event (RPM, нагрузка, AFR, temp и т.п.).
    ///
    /// Если в ECU нет stored DTC → Freeze Frame пустой.
    #[cfg(feature = "kernel-upload")]
    FreezeFrameCan {
        #[arg(long, default_value_t = 2000)]
        timeout_ms: u64,
    },

    /// Прочитать **DTC коды** (Diagnostic Trouble Codes) через OBD-II
    /// Mode 0x03 (confirmed/stored) + Mode 0x07 (pending) + Mode 0x0A
    /// (permanent) поверх CAN.
    ///
    /// Возвращает 5-символьные DTC коды в стандартном формате
    /// (`P0301` = misfire cyl 1, `P0420` = catalyst efficiency, и т.п.).
    /// Расшифровка кодов — Google или официальный список SAE J2012.
    #[cfg(feature = "kernel-upload")]
    DtcCan {
        #[arg(long, default_value_t = 1500)]
        timeout_ms: u64,
    },

    /// Прочитать произвольный RAM-адрес ECU через **UDS Mode 0x23
    /// ReadMemoryByAddress** поверх CAN/ISO15765. Используется для проверки,
    /// что ECU отдаёт Subaru-specific tuner-grade параметры (knock correction,
    /// AFR, boost target/actual) в default session без security-access-а.
    ///
    /// Адреса берутся из `<ecuparams>` блока upstream `logger.xml` для
    /// нашего ROM (`4E42504007`), все в RAM-диапазоне `0xFFxxxx` (sensor data).
    /// Пример (E114 Knock Sum, 1 байт): `peek-ram-can --addr 0xFF7664 --len 1`.
    /// Пример (E39 Feedback Knock Correction, 4-byte float):
    /// `peek-ram-can --addr 0xFF768C --len 4`.
    PeekRamCan {
        /// 32-bit адрес (hex с `0x`-префиксом или без).
        #[arg(long)]
        addr: String,

        /// Сколько байт читать (1..=255). По умолчанию 4 (типичный 4-byte float
        /// для Subaru ecuparams).
        #[arg(long, default_value_t = 4)]
        len: u8,

        #[arg(long, default_value_t = 1500)]
        timeout_ms: u64,

        /// Перед Mode 0xA8 послать UDS `10 03` (ExtendedDiagnosticSession).
        /// Иногда разблокирует доступ к ecuparams-адресам (0xFFxxxx),
        /// которые в default session режутся NRC 0x12.
        #[arg(long)]
        extended_session: bool,
    },

    /// (Slice 23 Phase C+D+E, feature `kernel-upload`) **Полный Mac-native ROM
    /// dump через CAN-bus**. Делает всё end-to-end:
    /// 1. Open Tactrix in ISO15765 mode + FLOW_CONTROL filter
    /// 2. OBD-II wake-up (Mode 01/09)
    /// 3. ExtendedDiagnosticSession + SecurityAccess через `subaru_genkey_can`
    /// 4. RequestDownload + 64× TransferData (SID 0xB6) — upload OpenECU kernel
    /// 5. StartRoutine — jump в RAM
    /// 6. Kernel handshake + read-memory loop → собрать 1 МБ ROM
    /// 7. Сохранить как `.bin`, сравнить с fixture байт-в-байт
    #[cfg(feature = "kernel-upload")]
    DumpRomCan {
        /// Куда сохранить dump.
        #[arg(short, long)]
        output: PathBuf,

        /// Начальный адрес.
        #[arg(long, default_value = "0x000000")]
        start: String,

        /// Сколько байт читать. `0` = полный 1 МБ.
        #[arg(long, default_value_t = 0)]
        length: usize,

        /// Размер chunk-а для kernel read-memory (max 2048).
        #[arg(long, default_value_t = 2048)]
        chunk_size: u16,

        #[arg(long, default_value_t = 5000)]
        timeout_ms: u64,
    },

    /// (Slice 23 Phase B, feature `kernel-upload`) Subaru UDS handshake:
    /// StartDiagnosticSession (programming mode) → SecurityAccess RequestSeed.
    /// Выводит seed от ECU и (если `--send-key` указан) попробует отправить
    /// computed key через наш `subaru_genkey`. Это **read-only diagnostic**
    /// до SendKey; SendKey осторожен — после 2 невалидных попыток ECU
    /// блокируется на 10 секунд.
    #[cfg(feature = "kernel-upload")]
    PeekSeed {
        /// Дополнительно вычислить key через `subaru_genkey` (K-line algorithm)
        /// и отправить через SID 0x27 0x02. Если ECU вернёт `67 02` — algorithm
        /// совпадает с CAN-side. Если NRC `7F 27 35` — algorithm не тот.
        #[arg(long)]
        send_key: bool,

        #[arg(long, default_value_t = 2000)]
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
    ///
    /// По умолчанию использует SerialTransport (требует `--port`). С флагом
    /// `--tactrix` подключается к Openport 2.0 через USB-bulk (libusb).
    Logger {
        #[arg(short, long, default_value = "")]
        port: String,
        #[arg(short, long, default_value_t = 4800)]
        baud: u32,

        /// `log_defs.xml` с определениями параметров.
        #[arg(long)]
        def: PathBuf,

        /// ECU `id` для резолва (template `base` или конкретный hex-ID).
        #[arg(long, default_value = "base")]
        ecu: String,

        /// Список параметров по `id`, через запятую. В upstream
        /// `log_defs.xml` ID — человеческое имя:
        /// `"Engine Speed,Coolant Temperature,Mass Air Flow,Manifold Absolute Pressure"`.
        /// Полный список — `inspect-log <log_defs.xml> --ecu base`.
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

        /// Использовать Tactrix Openport 2.0 (USB-bulk) вместо serial-порта.
        /// `--port` в этом режиме игнорируется.
        #[arg(long)]
        tactrix: bool,
    },

    /// CAN-логгер через OBD-II Mode 01 (Current Data) поверх ISO15765.
    ///
    /// Подключается к Tactrix Openport 2.0 в CAN-mode (требует `sudo` на macOS),
    /// опрашивает стандартные OBD-II PID-ы (RPM, Coolant Temp, MAF и т.п.).
    /// Это правильный путь для 2007+ Subaru ECU — SSM2 `ReadAddresses` через
    /// K-Line на этих машинах режется анти-fuzz защитой (ECU отвечает только
    /// на init), а OBD-II/CAN всегда открыт.
    LoggerCan {
        /// Список параметров через запятую (имена из встроенного PID-таблицы).
        /// Используй `--list` чтобы увидеть все доступные имена. Пример:
        /// `--params "RPM,Coolant Temp,MAF,IAT,TPS"`.
        #[arg(long, value_delimiter = ',', num_args = 0..)]
        params: Vec<String>,

        /// Авто-подписаться на **все** PID-ы, которые ECU объявил поддержанными
        /// через chain bitmap-ов 0x00/0x20/0x40/... (и которые есть в нашей
        /// встроенной таблице). Игнорирует `--params`.
        #[arg(long)]
        all_supported: bool,

        /// Куда писать CSV-датлог.
        #[arg(long, required_unless_present_any = ["list"])]
        out: Option<PathBuf>,

        /// Интервал между опросами (мс).
        #[arg(long, default_value_t = 100)]
        interval_ms: u64,

        /// Длительность; 0 = бесконечно (Ctrl+C для остановки).
        #[arg(long, default_value_t = 0)]
        duration_secs: u64,

        /// CAN timeout на один PID (мс).
        #[arg(long, default_value_t = 500)]
        timeout_ms: u64,

        /// Вывести таблицу PID-ов и выйти. С `--probe` дополнительно подключится
        /// к ECU и пометит каждый PID как ✓ / ✗ по результату chain-bitmap-ов.
        #[arg(long)]
        list: bool,

        /// С `--list`: подключиться к ECU и опросить supported-PIDs chain,
        /// чтобы пометить таблицу галочками. Без `--list` игнорируется.
        #[arg(long)]
        probe: bool,
    },

    /// **Subaru SSM3 логгер через CAN** — опрашивает Subaru-specific
    /// параметры из стандартного `<parameters>`-блока RomRaider `logger.xml`
    /// (capability-bitmap'd, отличаются от OBD-II Mode 01 PID-ов!) через
    /// proprietary cmd `0xA8` ReadAddresses-over-CAN.
    ///
    /// На 2007+ Subaru это **единственный** способ снять tuner-grade данные:
    /// Knock Correction Advance (P23), A/F Sensor (P58), Fine Learning Knock
    /// (P91), WGDC (P36) и т.п. — то, чего нет в стандартном OBD-II.
    ///
    /// `<ecuparams>`-адреса (0xFFxxxx, float) в default session блокируются
    /// анти-fuzz-ом (NRC 0x12) — для них нужен kernel-upload, но он halt-ит
    /// engine, поэтому для live-логгинга непригоден.
    LoggerSsmCan {
        /// Список параметров по ID (`P8,P23,P3`) или имени
        /// (`"RPM,Knock Correction,A/F Correction #1"`), case-insensitive.
        #[arg(long, value_delimiter = ',', num_args = 0..)]
        params: Vec<String>,

        /// Авто-подписаться на **все** параметры из встроенной таблицы (~15
        /// тщательно подобранных под диагностику knock/AFR/boost).
        #[arg(long)]
        all: bool,

        /// Куда писать CSV.
        #[arg(long, required_unless_present_any = ["list"])]
        out: Option<PathBuf>,

        /// Интервал между опросами (мс). При батчинге всех ~15 параметров
        /// в одном запросе реально достижимы 20+ Hz через `--interval-ms 50`.
        #[arg(long, default_value_t = 100)]
        interval_ms: u64,

        /// Длительность; `0` = бесконечно.
        #[arg(long, default_value_t = 0)]
        duration_secs: u64,

        /// Timeout на один запрос (мс).
        #[arg(long, default_value_t = 500)]
        timeout_ms: u64,

        /// Распечатать таблицу доступных SSM-параметров и выйти.
        #[arg(long)]
        list: bool,
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
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Ports => list_ports(),
        Cmd::TactrixInfo => tactrix_info_cmd(),
        Cmd::SsmInit {
            port,
            baud,
            timeout_ms,
            tactrix,
        } => {
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
        Cmd::InspectDef {
            path,
            resolve,
            rom,
            sample_byte,
        } => inspect_def(&path, resolve, rom.as_deref(), sample_byte),
        Cmd::InspectLog { path, ecu } => inspect_log(&path, ecu.as_deref()),
        Cmd::ReadTable {
            rom,
            def,
            rom_id,
            table,
        } => read_table_cmd(&rom, &def, &rom_id, &table),
        Cmd::DumpRom {
            port,
            baud,
            start,
            length,
            output,
            chunk_size,
            timeout_ms,
            tactrix,
            reset_per_chunk,
        } => dump_rom_cmd(
            &port,
            baud,
            &start,
            &length,
            &output,
            chunk_size,
            Duration::from_millis(timeout_ms),
            tactrix,
            reset_per_chunk,
        ),
        #[cfg(feature = "kernel-upload")]
        Cmd::DumpRomKernel {
            output,
            mcu,
            start,
            length,
            timeout_ms,
        } => dump_rom_kernel_cmd(
            &output,
            &mcu,
            &start,
            length,
            Duration::from_millis(timeout_ms),
        ),
        Cmd::PeekRom {
            start,
            count,
            timeout_ms,
            skip_init,
            gap_ms,
        } => peek_rom_cmd(
            &start,
            count,
            Duration::from_millis(timeout_ms),
            skip_init,
            gap_ms,
        ),
        Cmd::PeekVin { timeout_ms } => {
            peek_obd_cmd(0x09, 0x02, "VIN", Duration::from_millis(timeout_ms))
        }
        Cmd::PeekCvn { timeout_ms } => {
            peek_obd_cmd(0x09, 0x06, "CVN", Duration::from_millis(timeout_ms))
        }
        Cmd::PeekRamCan {
            addr,
            len,
            timeout_ms,
            extended_session,
        } => peek_ram_can_cmd(
            &addr,
            len,
            Duration::from_millis(timeout_ms),
            extended_session,
        ),
        #[cfg(feature = "kernel-upload")]
        Cmd::DtcCan { timeout_ms } => dtc_can_cmd(Duration::from_millis(timeout_ms)),
        #[cfg(feature = "kernel-upload")]
        Cmd::FreezeFrameCan { timeout_ms } => {
            freeze_frame_can_cmd(Duration::from_millis(timeout_ms))
        }
        #[cfg(feature = "kernel-upload")]
        Cmd::PeekSeed {
            send_key,
            timeout_ms,
        } => peek_seed_cmd(send_key, Duration::from_millis(timeout_ms)),
        #[cfg(feature = "kernel-upload")]
        Cmd::DumpRomCan {
            output,
            start,
            length,
            chunk_size,
            timeout_ms,
        } => dump_rom_can_cmd(
            &output,
            &start,
            length,
            chunk_size,
            Duration::from_millis(timeout_ms),
        ),
        Cmd::Logger {
            port,
            baud,
            def,
            ecu,
            params,
            interval_ms,
            duration_secs,
            out,
            timeout_ms,
            tactrix,
        } => logger_cmd(
            &port,
            baud,
            &def,
            &ecu,
            &params,
            interval_ms,
            duration_secs,
            &out,
            Duration::from_millis(timeout_ms),
            tactrix,
        ),
        Cmd::LoggerCan {
            params,
            all_supported,
            out,
            interval_ms,
            duration_secs,
            timeout_ms,
            list,
            probe,
        } => logger_can_cmd(
            &params,
            all_supported,
            out.as_ref(),
            interval_ms,
            duration_secs,
            Duration::from_millis(timeout_ms),
            list,
            probe,
        ),
        Cmd::LoggerSsmCan {
            params,
            all,
            out,
            interval_ms,
            duration_secs,
            timeout_ms,
            list,
        } => logger_ssm_can_cmd(
            &params,
            all,
            out.as_ref(),
            interval_ms,
            duration_secs,
            Duration::from_millis(timeout_ms),
            list,
        ),
    }
}

fn dump_rom_cmd(
    port: &str,
    baud: u32,
    start: &str,
    length: &str,
    output: &PathBuf,
    chunk_size: usize,
    timeout: Duration,
    tactrix: bool,
    reset_per_chunk: bool,
) -> Result<()> {
    let start_addr = parse_int_or_hex_u32(start).with_context(|| format!("--start `{start}`"))?;
    let length_val =
        parse_int_or_hex_usize(length).with_context(|| format!("--length `{length}`"))?;
    if length_val == 0 {
        anyhow::bail!("--length must be > 0");
    }

    if tactrix {
        if reset_per_chunk {
            // Workaround под анти-fuzz Subaru ECU 2007+: каждый ReadBlock
            // в свежей K-Line-сессии. Очень медленно (~6 часов на 1 МБ),
            // но единственный путь когда ECU режет повторные ReadBlock.
            dump_rom_tactrix_session_per_chunk(start_addr, length_val, chunk_size, timeout, output)
        } else {
            // Обычный SSM2 (до анти-fuzz, 2003–2006 Subaru): один ecu_init,
            // дальше серия ReadBlock в одной сессии. ~22 мин на 512 КБ через
            // K-Line @ 4800 baud.
            dump_rom_tactrix_continuous(start_addr, length_val, chunk_size, timeout, output)
        }
    } else {
        if port.is_empty() {
            anyhow::bail!("--port required when --tactrix is not set");
        }
        let mut cfg = SerialConfig::ssm(port);
        cfg.baud_rate = baud;
        let mut tr =
            SerialTransport::open(&cfg).with_context(|| format!("opening serial {port}@{baud}"))?;
        tr.purge()?;
        do_dump_rom(&mut tr, start_addr, length_val, chunk_size, timeout, output)
    }
}

/// Tactrix-дамп через **одну** SSM2-сессию: ecu_init один раз, дальше серия
/// ReadBlock-запросов. Это «нормальный» режим SSM2 — работает на 2003–2006
/// Subaru до анти-fuzz эры. Намного быстрее `session_per_chunk`-варианта.
fn dump_rom_tactrix_continuous(
    start_addr: u32,
    length: usize,
    chunk_size: usize,
    timeout: Duration,
    output: &PathBuf,
) -> Result<()> {
    let mut tr = open_tactrix()?;

    let init = ssm::ecu_init(&mut tr, timeout)
        .context("SSM ecu-init failed (ignition ON? engine running?)")?;
    eprintln!(
        "  ECU online: ROM {} ({} cap bytes)",
        bytes::hex_dump(&init.rom_id),
        init.capabilities.len()
    );

    let started = std::time::Instant::now();
    eprintln!(
        "Dumping {length} bytes from 0x{start_addr:06X} via Tactrix \
         (chunks of {chunk_size}, single session, timeout {}ms)…",
        timeout.as_millis()
    );
    let mut last_percent = -1i32;
    let bytes = ssm::dump_rom(
        &mut tr,
        Address::new(start_addr),
        length,
        chunk_size,
        timeout,
        |done, total| {
            let percent = (done as i64 * 100 / total as i64) as i32;
            if percent != last_percent {
                let elapsed = started.elapsed().as_secs_f64();
                let rate = done as f64 / elapsed.max(1e-6);
                let eta = (total - done) as f64 / rate.max(1.0);
                eprintln!("  {done}/{total} ({percent}%)  {rate:.0} B/s  ETA {eta:.0}s");
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

/// Дамп через одну USB-сессию Tactrix, но с пересозданием K-Line канала
/// (`atc`/`ato`/`atf`) перед каждым ReadBlock. Subaru SSM2 на 2007 Forester XT
/// принимает только один ReadBlock на «свежий» канал — поэтому между чанками
/// делаем `reset_channel()` + повторный `ecu_init`. USB остаётся открытым,
/// что намного дешевле, чем полный open/close цикл.
fn dump_rom_tactrix_session_per_chunk(
    start_addr: u32,
    length: usize,
    chunk_size: usize,
    timeout: Duration,
    output: &PathBuf,
) -> Result<()> {
    const MAX_RETRIES: u32 = 5;
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
                        attempt,
                        MAX_RETRIES,
                        addr.raw()
                    );
                    std::thread::sleep(COOLDOWN_RETRY);
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!(
                            "chunk at 0x{:06X} failed after {MAX_RETRIES} retries",
                            addr.raw()
                        )
                    })
                }
            }
        };
        out.extend_from_slice(&data);

        let percent = (out.len() as i64 * 100 / length as i64) as i32;
        if percent != last_percent {
            let elapsed = started.elapsed().as_secs_f64();
            let rate = out.len() as f64 / elapsed.max(1e-6);
            let eta_s = (length - out.len()) as f64 / rate.max(1.0);
            eprintln!(
                "  {}/{} ({percent}%)  {rate:.1} B/s  ETA {:.0}s",
                out.len(),
                length,
                eta_s
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
        started.elapsed().as_secs_f64(),
        out.len(),
        output.display()
    );
    Ok(())
}

fn read_one_chunk_via(
    tr: &mut TactrixTransport,
    addr: Address,
    count: usize,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let _init = ssm::ecu_init(tr, timeout).context("ecu_init")?;
    let data = ssm::read_block(tr, addr, count, timeout).context("read_block")?;
    Ok(data)
}

fn do_dump_rom(
    transport: &mut dyn romraider_io::transport::Transport,
    start_addr: u32,
    length: usize,
    chunk_size: usize,
    timeout: Duration,
    output: &PathBuf,
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
                let rate = done as f64 / elapsed.max(1e-6);
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
        None => trimmed.parse::<u32>().context("invalid decimal"),
    }
}

fn parse_int_or_hex_usize(s: &str) -> Result<usize> {
    let trimmed = s.trim();
    let stripped = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"));
    match stripped {
        Some(hex) => usize::from_str_radix(hex, 16).context("invalid hex"),
        None => trimmed.parse::<usize>().context("invalid decimal"),
    }
}

fn logger_cmd(
    port: &str,
    baud: u32,
    def_path: &PathBuf,
    ecu_id: &str,
    param_ids: &[String],
    interval_ms: u64,
    duration_secs: u64,
    out_path: &PathBuf,
    timeout: Duration,
    tactrix: bool,
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
        eprintln!(
            "  + {} @ {:?} ({:?}, expr=`{}`, {})",
            p.id,
            compiled.address,
            compiled.storage_type,
            p.expr.as_deref().unwrap_or("(none)"),
            p.metric.as_deref().unwrap_or(""),
        );
        session.subscribe(compiled);
    }

    // 3. CSV-датлог.
    let mut datalog = romraider_logger::datalog::DatalogWriter::create(out_path)
        .with_context(|| format!("opening {}", out_path.display()))?;

    // 4. Открыть транспорт.
    let mut transport: Box<dyn romraider_io::transport::Transport> = if tactrix {
        Box::new(open_tactrix()?)
    } else if port.is_empty() {
        anyhow::bail!("--port required when --tactrix is not set");
    } else {
        let mut cfg = SerialConfig::ssm(port);
        cfg.baud_rate = baud;
        let mut tr =
            SerialTransport::open(&cfg).with_context(|| format!("opening serial {port}@{baud}"))?;
        tr.purge()?;
        Box::new(tr)
    };

    // 4b. Разбудить шину через SSM ECU-init (0xBF). Без этого ECU молчит на
    // ReadAddresses — K-line бус остаётся в idle, первый `0xA8` уходит в пустоту.
    eprintln!("Sending SSM ECU-init to wake bus…");
    let init = ssm::ecu_init(transport.as_mut(), timeout)
        .context("SSM ECU-init failed (check ignition / cable / baud)")?;
    eprintln!(
        "  ECU ROM-ID: {}  SSM-ID: {}  caps: {} bytes",
        bytes::hex_dump(&init.rom_id),
        bytes::hex_dump(&init.ssm_id),
        init.capabilities.len(),
    );

    // 5. Loop.
    let interval = Duration::from_millis(interval_ms);
    let deadline = if duration_secs > 0 {
        Some(std::time::Instant::now() + Duration::from_secs(duration_secs))
    } else {
        None
    };
    let mut count = 0u64;
    eprintln!(
        "Starting log to {} (interval {}ms)…",
        out_path.display(),
        interval_ms
    );
    loop {
        let started = std::time::Instant::now();
        match session.poll_once(transport.as_mut()) {
            Ok(sample) => {
                datalog.write_sample(&sample)?;
                count += 1;
                if count % 10 == 0 {
                    let preview = sample
                        .values
                        .iter()
                        .map(|v| format!("{}={:.2}", v.parameter_id, v.value))
                        .collect::<Vec<_>>()
                        .join(" ");
                    eprintln!("  [{count:>5}]  {preview}");
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

fn logger_ssm_can_cmd(
    param_keys: &[String],
    all: bool,
    out_path: Option<&PathBuf>,
    interval_ms: u64,
    duration_secs: u64,
    timeout: Duration,
    list: bool,
) -> Result<()> {
    use romraider_protocol::subaru::{
        self, find_derived_param, find_ssm_param, read_ssm_params_can, SsmDerivedParam, SsmParam,
        SUBARU_DERIVED_PARAMS, SUBARU_SSM_PARAMS,
    };

    if list {
        println!("Available Subaru SSM3 RAW parameters (via CAN Mode 0xA8):");
        println!(
            "  {:<5}  {:<32}  {:<10}  {:<5}  Units",
            "ID", "Name", "Addr", "Bytes"
        );
        for p in SUBARU_SSM_PARAMS {
            println!(
                "  {:<5}  {:<32}  0x{:06X}    {:<5}  {}",
                p.id, p.name, p.address, p.bytes, p.units,
            );
        }
        println!("\nDERIVED parameters (computed from raw values, no extra ECU read):");
        println!("  {:<5}  {:<32}  {:<10}  Units", "", "Name", "Depends-on");
        for d in SUBARU_DERIVED_PARAMS {
            println!(
                "  {:<5}  {:<32}  {:<40}  {}",
                "—",
                d.name,
                d.depends_on.join(" + "),
                d.units,
            );
        }
        return Ok(());
    }

    let out_path = out_path.expect("clap required_unless_present validates this");

    // 1. Резолв подписок: разделяем на raw и derived.
    let mut chosen_raw: Vec<&'static SsmParam> = Vec::new();
    let mut chosen_derived: Vec<&'static SsmDerivedParam> = Vec::new();
    if all {
        chosen_raw.extend(SUBARU_SSM_PARAMS.iter());
        chosen_derived.extend(SUBARU_DERIVED_PARAMS.iter());
    } else {
        if param_keys.is_empty() {
            anyhow::bail!("at least one --params is required (or --all / --list)");
        }
        for key in param_keys {
            if let Some(p) = find_ssm_param(key) {
                chosen_raw.push(p);
            } else if let Some(d) = find_derived_param(key) {
                chosen_derived.push(d);
            } else {
                anyhow::bail!("unknown SSM param '{key}' — use `--list`");
            }
        }
    }

    // 2. Авто-добавить raw-параметры от которых зависят derived.
    for d in &chosen_derived {
        for dep_name in d.depends_on {
            if !chosen_raw.iter().any(|p| p.name == *dep_name) {
                if let Some(p) = find_ssm_param(dep_name) {
                    chosen_raw.push(p);
                    eprintln!("  (auto-added '{dep_name}' as dependency of '{}')", d.name);
                }
            }
        }
    }

    eprintln!(
        "Subscribing to {} raw + {} derived param(s):",
        chosen_raw.len(),
        chosen_derived.len()
    );
    for p in &chosen_raw {
        eprintln!(
            "  + {:<4} {:<32} addr=0x{:06X} ({}B, {})",
            p.id, p.name, p.address, p.bytes, p.units
        );
    }
    for d in &chosen_derived {
        eprintln!(
            "  Δ      {:<32} = f({}) ({})",
            d.name,
            d.depends_on.join(", "),
            d.units
        );
    }

    // 2. Открыть CAN + SSM-CAN ECU init.
    let mut tr = open_tactrix_can()?;
    eprintln!("SSM-CAN ECU init (cmd 0xAA)…");
    let info = subaru::ecu_init_can(&mut tr, timeout).context("SSM-CAN ECU init failed")?;
    if info.len() >= 8 {
        eprintln!(
            "  ECU online: SSM-ID {:02X} {:02X} {:02X}, ROM-ID {} ({} cap-bytes)",
            info[0],
            info[1],
            info[2],
            bytes::hex_dump(&info[3..8]),
            info.len() - 8,
        );
    }

    // 3. CSV.
    let mut datalog = romraider_logger::datalog::DatalogWriter::create(out_path)
        .with_context(|| format!("opening {}", out_path.display()))?;

    // 4. Loop с батчингом — все параметры в одном Mode 0xA8 запросе.
    let interval = Duration::from_millis(interval_ms);
    let deadline = if duration_secs > 0 {
        Some(std::time::Instant::now() + Duration::from_secs(duration_secs))
    } else {
        None
    };
    let mut count = 0u64;
    eprintln!(
        "Starting SSM-CAN log to {} (interval {} ms, {} raw + {} derived)…",
        out_path.display(),
        interval_ms,
        chosen_raw.len(),
        chosen_derived.len(),
    );
    loop {
        let started = std::time::Instant::now();
        match read_ssm_params_can(&mut tr, &chosen_raw, timeout) {
            Ok(per_param) => {
                // 1. Сначала raw values + name→value lookup для derived-вычислений.
                let mut values = Vec::with_capacity(chosen_raw.len() + chosen_derived.len());
                let mut by_name: std::collections::HashMap<&str, f64> =
                    std::collections::HashMap::new();
                for (p, raw) in chosen_raw.iter().zip(&per_param) {
                    let v = (p.scale)(raw);
                    by_name.insert(p.name, v);
                    values.push(SampleValue {
                        parameter_id: p.name.into(),
                        raw: raw.clone(),
                        value: v,
                    });
                }
                // 2. Compute derived params from raw values.
                for d in &chosen_derived {
                    let inputs: Vec<f64> = d
                        .depends_on
                        .iter()
                        .map(|n| by_name.get(n).copied().unwrap_or(0.0))
                        .collect();
                    let value = (d.compute)(&inputs);
                    values.push(SampleValue {
                        parameter_id: d.name.into(),
                        raw: Vec::new(), // derived не имеет raw байтов
                        value,
                    });
                }
                let sample = Sample {
                    timestamp: std::time::SystemTime::now(),
                    values,
                };
                datalog.write_sample(&sample)?;
                count += 1;
                if count == 1 || count % 10 == 0 {
                    let preview = sample
                        .values
                        .iter()
                        .map(|v| format!("{}={:.2}", v.parameter_id, v.value))
                        .collect::<Vec<_>>()
                        .join("  ");
                    eprintln!("  [{count:>5}]  {preview}");
                }
            }
            Err(e) => eprintln!("  poll error: {e}"),
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

#[cfg(feature = "kernel-upload")]
fn freeze_frame_can_cmd(timeout: Duration) -> Result<()> {
    use romraider_kernel::dtc_db::dtc_lookup;
    use romraider_kernel::orchestrator::read_freeze_frame;
    let mut tr = open_tactrix_can()?;
    eprintln!("Reading Freeze Frame (OBD-II Mode 0x02)…");
    let ff = read_freeze_frame(&mut tr, timeout).context("read_freeze_frame failed")?;

    match &ff.triggering_dtc {
        Some(dtc) => {
            let desc = dtc_lookup(dtc).unwrap_or("(unknown code)");
            eprintln!("\n=== Triggering DTC ===");
            eprintln!("  {dtc}  —  {desc}");
        }
        None => {
            eprintln!("\n=== No Freeze Frame stored ===");
            eprintln!("  ECU не имеет stored DTC → нет snapshot.");
            return Ok(());
        }
    }

    eprintln!("\n=== Parameter snapshot at fault moment ===");
    if ff.values.is_empty() {
        eprintln!("  (ECU не вернул ни одного параметра — Mode 0x02 ограничен на этой firmware)");
    } else {
        eprintln!("  {:<30} {:>12}  Units", "Parameter", "Value");
        eprintln!("  {}", "-".repeat(56));
        for v in &ff.values {
            eprintln!("  {:<30} {:>12.2}  {}", v.name, v.value, v.units);
        }
    }
    Ok(())
}

#[cfg(feature = "kernel-upload")]
fn dtc_can_cmd(timeout: Duration) -> Result<()> {
    use romraider_kernel::dtc_db::dtc_lookup;
    use romraider_kernel::orchestrator::read_dtcs;
    let mut tr = open_tactrix_can()?;
    let report = read_dtcs(&mut tr, timeout).context("read_dtcs failed")?;
    let sections = [
        ("Stored DTCs (Mode 0x03)", &report.stored),
        ("Pending DTCs (Mode 0x07)", &report.pending),
        ("Permanent DTCs (Mode 0x0A)", &report.permanent),
    ];
    for (label, codes) in sections {
        eprintln!("\n=== {label} ===");
        if codes.is_empty() {
            eprintln!("  ECU reported 0 DTC(s):");
            eprintln!("    (нет кодов)");
        } else {
            eprintln!("  ECU reported {} DTC(s):", codes.len());
            for c in codes {
                match dtc_lookup(c) {
                    Some(desc) => eprintln!("    {c}  —  {desc}"),
                    None => eprintln!("    {c}  —  (unknown code)"),
                }
            }
        }
    }
    Ok(())
}

fn peek_ram_can_cmd(
    addr_str: &str,
    len: u8,
    timeout: Duration,
    extended_session: bool,
) -> Result<()> {
    use romraider_protocol::subaru;

    let raw_addr =
        parse_int_or_hex_u32(addr_str).with_context(|| format!("--addr `{addr_str}`"))?;
    // SSM-over-CAN использует 24-bit адреса (`0xFF7664`). ECU сам подставляет
    // верхний `0xFF` для RAM-доступа. Принимаем и 32-bit форму (`0xFFFF7664`) —
    // просто маскируем младшие 24 бита.
    let addr = raw_addr & 0x00FF_FFFF;
    if raw_addr != addr {
        eprintln!("  (маскировал --addr 0x{raw_addr:08X} → 24-bit SSM 0x{addr:06X})");
    }
    if len == 0 || len > 64 {
        anyhow::bail!("--len must be 1..=64");
    }

    let mut tr = open_tactrix_can()?;

    if extended_session {
        // UDS DiagnosticSessionControl (0x10) под-функция 0x03 = ExtendedDiagSession.
        eprintln!("Entering UDS ExtendedDiagnosticSession (10 03)…");
        let mut tx = Vec::with_capacity(4 + 2);
        tx.extend_from_slice(&0x7E0u32.to_be_bytes());
        tx.push(0x10);
        tx.push(0x03);
        tr.write_all(&tx, timeout).context("10 03 TX")?;
        let mut rb = [0u8; 64];
        match tr.read_frame(&mut rb, timeout) {
            Ok(n) if n >= 5 => {
                let uds = &rb[4..n];
                if uds[0] == 0x7F {
                    eprintln!("  ⚠️  ExtendedDiag rejected: NRC 0x{:02X}", uds[2]);
                } else if uds[0] == 0x50 {
                    eprintln!("  ✅ ExtendedDiag granted (0x50 echo)");
                } else {
                    eprintln!("  unexpected: {:02X?}", uds);
                }
            }
            Ok(n) => eprintln!("  short RX ({n} bytes)"),
            Err(e) => eprintln!("  RX error: {e}"),
        }
    }

    // SSM-CAN protocol требует ECU init (`AA` → `EA <info>`) перед любыми
    // 0xAx-командами. Без него ECU режет всё через NRC 0x12 — это **не**
    // тот же init что K-Line `BF` (тот sends только для K-Line bus).
    eprintln!("SSM-CAN ECU init (cmd 0xAA) …");
    match subaru::ecu_init_can(&mut tr, timeout) {
        Ok(info) => {
            eprintln!(
                "  EA response ({} bytes): {}",
                info.len(),
                bytes::hex_dump(&info[..info.len().min(16)]),
            );
        }
        Err(e) => {
            eprintln!("  ⚠️  SSM-CAN init failed: {e} — продолжаем, вдруг ECU и без него отвечает");
        }
    }

    eprintln!(
        "Reading {} byte(s) from 24-bit SSM addr 0x{:06X} via Mode 0xA8 ReadAddresses-over-CAN…",
        len, addr,
    );
    match subaru::read_block_can(&mut tr, addr, len as usize, timeout) {
        Ok(data) => {
            println!("Raw ({} bytes):  {}", data.len(), bytes::hex_dump(&data));
            if data.len() == 4 {
                let f = f32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                let u = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                println!("As float BE:    {f}");
                println!("As uint32 BE:   {u}");
            } else if data.len() == 2 {
                let u = u16::from_be_bytes([data[0], data[1]]);
                println!("As uint16 BE:   {u}");
            } else if data.len() == 1 {
                println!("As uint8:       {}", data[0]);
            }
            println!("\n✅ Mode 0xA8 SSM-over-CAN работает — можно строить ecuparams-логгер.");
        }
        Err(e) => {
            eprintln!("\n❌ Mode 0xA8 SSM-over-CAN failed: {e}");
            eprintln!(
                "Если timeout — ECU не отвечает на Mode 0xA8 в default session.\n\
                 Возможные следующие шаги:\n\
                 - попробовать ExtendedDiagSession (`10 03`) перед Mode 0xA8\n\
                 - попробовать SSM Mode 0xA0 ReadBlock (cmd `A0 <pad> <addr 3B> <size 1B>`)\n\
                 - в крайнем случае — kernel-upload с расширением kernel-side read commands.",
            );
            anyhow::bail!("read_addresses_can failed");
        }
    }
    Ok(())
}

fn logger_can_cmd(
    param_names: &[String],
    all_supported: bool,
    out_path: Option<&PathBuf>,
    interval_ms: u64,
    duration_secs: u64,
    timeout: Duration,
    list: bool,
    probe: bool,
) -> Result<()> {
    use romraider_protocol::obd2::{
        discover_supported_pids, find_pid, read_pid, read_pids_batched, ObdiiPid, MAX_BATCH_PIDS,
        STANDARD_PIDS,
    };

    // --list режим: распечатать таблицу. Если есть --probe — заодно спросить ECU.
    if list {
        let supported: Option<Vec<u8>> = if probe {
            let mut tr = open_tactrix_can()?;
            eprintln!("Probing ECU for supported PIDs via bitmap chain…");
            let pids = discover_supported_pids(&mut tr, timeout)
                .context("discover_supported_pids failed")?;
            eprintln!("  ECU reports {} supported PIDs.", pids.len());
            Some(pids)
        } else {
            None
        };
        println!("Available OBD-II Mode 01 PIDs:");
        if supported.is_some() {
            println!("  ✓/✗  {:<22}  {:<5}  {:<5}  Units", "Name", "PID", "Bytes");
        } else {
            println!("       {:<22}  {:<5}  {:<5}  Units", "Name", "PID", "Bytes");
        }
        for p in STANDARD_PIDS {
            let mark = match &supported {
                Some(s) if s.contains(&p.pid) => " ✓ ",
                Some(_) => " ✗ ",
                None => "   ",
            };
            println!(
                "  {mark}  {:<22}  0x{:02X}   {:<5}  {}",
                p.name, p.pid, p.bytes, p.units,
            );
        }
        if let Some(s) = supported {
            // Покажем PIDs, которые ECU объявил, но **которых нет** в нашей таблице.
            let unknown: Vec<u8> = s
                .iter()
                .filter(|pid| !STANDARD_PIDS.iter().any(|p| &p.pid == *pid))
                .copied()
                .collect();
            if !unknown.is_empty() {
                println!(
                    "\nECU также сообщил поддержку {} PID-ов **без scaling-формулы** в нашей таблице:",
                    unknown.len(),
                );
                let hex: Vec<String> = unknown.iter().map(|p| format!("0x{p:02X}")).collect();
                println!("  {}", hex.join(", "));
                println!("(добавь их в STANDARD_PIDS чтобы логировать.)");
            }
        }
        return Ok(());
    }

    let out_path = out_path.expect("clap required_unless_present validates this");

    // 1. Открыть Tactrix в CAN-mode.
    let mut tr = open_tactrix_can()?;

    // 2. Резолв подписок: либо явный список, либо all-supported через discover.
    let pids: Vec<&'static ObdiiPid> = if all_supported {
        eprintln!("Probing ECU for supported PIDs via bitmap chain…");
        let supported =
            discover_supported_pids(&mut tr, timeout).context("discover_supported_pids failed")?;
        let mut chosen: Vec<&'static ObdiiPid> = STANDARD_PIDS
            .iter()
            .filter(|p| supported.contains(&p.pid))
            .collect();
        // PID 0x00/0x20/0x40 — bitmap-pidы, неинтересны для логгинга.
        chosen.retain(|p| ![0x00, 0x20, 0x40, 0x60, 0x80, 0xA0].contains(&p.pid));
        if chosen.is_empty() {
            anyhow::bail!(
                "ECU reports {} PIDs but none are in our STANDARD_PIDS table",
                supported.len(),
            );
        }
        eprintln!(
            "  Subscribing to {} PIDs (intersection of ECU-supported and our table):",
            chosen.len(),
        );
        for p in &chosen {
            eprintln!("    + {} (PID 0x{:02X}, {})", p.name, p.pid, p.units);
        }
        chosen
    } else {
        if param_names.is_empty() {
            anyhow::bail!(
                "at least one --params name is required (or use --all-supported / --list)"
            );
        }
        let mut chosen = Vec::with_capacity(param_names.len());
        for name in param_names {
            let p = find_pid(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown PID '{name}' — use `logger-can --list` to see available names"
                )
            })?;
            chosen.push(p);
            eprintln!("  + {} (PID 0x{:02X}, {})", p.name, p.pid, p.units);
        }
        chosen
    };

    // 4. CSV.
    let mut datalog = romraider_logger::datalog::DatalogWriter::create(out_path)
        .with_context(|| format!("opening {}", out_path.display()))?;

    // 5. Loop.
    let interval = Duration::from_millis(interval_ms);
    let deadline = if duration_secs > 0 {
        Some(std::time::Instant::now() + Duration::from_secs(duration_secs))
    } else {
        None
    };
    let mut count = 0u64;
    eprintln!(
        "Starting CAN log to {} (interval {} ms, {} PIDs)…",
        out_path.display(),
        interval_ms,
        pids.len(),
    );
    loop {
        let started = std::time::Instant::now();
        // **Slice 24b — batched polling**: чанк-им подписки по `MAX_BATCH_PIDS`
        // (= 6) и отправляем одним Mode 01 запросом. ECU отвечает multi-frame
        // ISO-TP (Tactrix сам склеит). Это даёт ~5-6× speedup для большой
        // подписки (например 40 PIDs → 7 cycles вместо 40). Если batched
        // запрос фейлится — fall back на single-PID для этого чанка.
        let mut values = Vec::with_capacity(pids.len());
        let mut all_ok = true;
        'outer: for chunk in pids.chunks(MAX_BATCH_PIDS) {
            match read_pids_batched(&mut tr, chunk, timeout) {
                Ok(batched) => {
                    for p in chunk {
                        let raw = batched
                            .iter()
                            .find(|b| b.pid == p.pid)
                            .map(|b| b.data.clone());
                        match raw {
                            Some(d) if d.len() >= p.bytes => {
                                let raw = d[..p.bytes].to_vec();
                                let value = (p.scale)(&raw);
                                values.push(SampleValue {
                                    parameter_id: p.name.into(),
                                    raw,
                                    value,
                                });
                            }
                            _ => {
                                eprintln!("  poll error: {} missing in batched response", p.name);
                                all_ok = false;
                                break 'outer;
                            }
                        }
                    }
                }
                Err(batched_err) => {
                    // Fallback: некоторые ECU отвергают batch с >N PID (NRC 0x13).
                    // Делаем sequential single-PID для этого chunk-а.
                    tracing::debug!(
                        "batched read failed ({batched_err}), falling back to single-PID for chunk of {}",
                        chunk.len()
                    );
                    for p in chunk {
                        match read_pid(&mut tr, p.pid, timeout) {
                            Ok(data) if data.len() >= p.bytes => {
                                let raw = data[..p.bytes].to_vec();
                                let value = (p.scale)(&raw);
                                values.push(SampleValue {
                                    parameter_id: p.name.into(),
                                    raw,
                                    value,
                                });
                            }
                            Ok(short) => {
                                eprintln!(
                                    "  poll error: {} returned {} bytes (expected {})",
                                    p.name,
                                    short.len(),
                                    p.bytes,
                                );
                                all_ok = false;
                                break 'outer;
                            }
                            Err(e) => {
                                eprintln!("  poll error: {}: {e}", p.name);
                                all_ok = false;
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
        if all_ok {
            let sample = Sample {
                timestamp: std::time::SystemTime::now(),
                values,
            };
            datalog.write_sample(&sample)?;
            count += 1;
            if count % 10 == 0 || count == 1 {
                let preview = sample
                    .values
                    .iter()
                    .map(|v| format!("{}={:.2}", v.parameter_id, v.value))
                    .collect::<Vec<_>>()
                    .join("  ");
                eprintln!("  [{count:>5}]  {preview}");
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

fn tactrix_info_cmd() -> Result<()> {
    use romraider_io::tactrix::{find_tactrix, TACTRIX_OP2_PID, TACTRIX_VID};
    eprintln!(
        "Scanning USB bus for Tactrix Openport (VID=0x{:04X} PID=0x{:04X})…",
        TACTRIX_VID, TACTRIX_OP2_PID,
    );
    let devices = find_tactrix().context("USB enumeration failed")?;
    if devices.is_empty() {
        eprintln!("❌ Not found. Check:");
        eprintln!("  • USB-cable plugged on both ends (Mac + Tactrix)");
        eprintln!("  • LED on Tactrix is on (если совсем тёмный — power issue)");
        eprintln!("  • Try different USB port (preferably not through hub)");
        anyhow::bail!("no Tactrix devices found");
    }
    println!("✅ Found {} Tactrix device(s):", devices.len());
    for (i, d) in devices.iter().enumerate() {
        println!(
            "\n[{}] bus={:03} addr={:03}  VID=0x{:04X}  PID=0x{:04X}",
            i, d.bus_number, d.address, d.vid, d.pid
        );
        println!("    USB version:     {}", d.usb_version);
        println!("    Device version:  {}", d.device_version);
        println!("    Configurations:  {}", d.num_interfaces);
        println!(
            "    Manufacturer:    {}",
            d.manufacturer.as_deref().unwrap_or("(?)")
        );
        println!(
            "    Product:         {}",
            d.product.as_deref().unwrap_or("(?)")
        );
        println!(
            "    Serial:          {}",
            d.serial.as_deref().unwrap_or("(?)")
        );
        if d.manufacturer.is_none() && d.product.is_none() && d.serial.is_none() {
            println!();
            println!(
                "    ⚠️  Не смогли прочитать string-descriptors — устройство видно но open() не прошёл."
            );
            println!("       На macOS возможно нужен `sudo` для bulk-операций (claim-interface),");
            println!("       или ещё не появился dialog «Allow accessory». Re-plug если нужно.");
        }
    }
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
    println!(
        "ROM ID:        {} ({})",
        bytes::hex_dump(&init.rom_id),
        printable_ascii(&init.rom_id)
    );
    println!("Capabilities:  {} bytes", init.capabilities.len());
    if !init.capabilities.is_empty() {
        println!("               {}", bytes::hex_dump(&init.capabilities));
    }
}

fn printable_ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                '.'
            }
        })
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
    path: &PathBuf,
    do_resolve: bool,
    rom_filter: Option<&str>,
    sample_byte: Option<f64>,
) -> Result<()> {
    let doc =
        romraider_defs::parse_file(path).with_context(|| format!("parsing {}", path.display()))?;
    print_def_summary(path, &doc);
    if do_resolve {
        let resolved = resolve(&doc).context("resolving inheritance")?;
        println!();
        println!("=== RESOLVED ===");
        let it = resolved
            .iter()
            .filter(|r| rom_filter.is_none_or(|f| r.xml_id == f));
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
    println!(
        "ROM {} (ecuid={ecu_id})  tables={}",
        rom.xml_id,
        rom.tables.len()
    );
    for t in &rom.tables {
        print_resolved_table(t, 1, sample_byte);
    }
}

fn print_resolved_table(t: &ResolvedTable, indent: usize, sample_byte: Option<f64>) {
    let pad = "  ".repeat(indent);
    let kind = t.kind.map_or("?", debug_kind);
    let addr = t
        .storage_address
        .map_or_else(|| "-".into(), |a| format!("{a}"));
    let storage = t.storage_type.map_or("-", debug_storage);
    let dims = match (t.size_x, t.size_y) {
        (Some(x), Some(y)) => format!("{x}x{y}"),
        (Some(x), None) => x.to_string(),
        (None, Some(y)) => y.to_string(),
        _ => "-".into(),
    };
    let units = t
        .scalings
        .first()
        .and_then(|s| s.units.as_deref())
        .unwrap_or("");
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
    use romraider_defs::TableKind::{
        BitwiseSwitch, OneD, StaticXAxis, StaticYAxis, Switch, ThreeD, TwoD, XAxis, YAxis,
    };
    match k {
        OneD => "1D",
        TwoD => "2D",
        ThreeD => "3D",
        XAxis => "X-Axis",
        YAxis => "Y-Axis",
        StaticXAxis => "SX-Axis",
        StaticYAxis => "SY-Axis",
        Switch => "Switch",
        BitwiseSwitch => "BSwitch",
    }
}

fn debug_storage(s: romraider_defs::StorageType) -> &'static str {
    use romraider_defs::StorageType::{
        Char, Float, Hex, Int16, Int32, Int8, UInt16, UInt32, UInt8,
    };
    match s {
        UInt8 => "uint8",
        Int8 => "int8",
        UInt16 => "uint16",
        Int16 => "int16",
        UInt32 => "uint32",
        Int32 => "int32",
        Float => "float",
        Hex => "hex",
        Char => "char",
    }
}

fn read_table_cmd(
    rom_path: &PathBuf,
    def_path: &PathBuf,
    rom_id: &str,
    table_name: &str,
) -> Result<()> {
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

    let rom =
        RomImage::open(rom_path).with_context(|| format!("opening {}", rom_path.display()))?;

    print_read_table(&rom, table)
}

fn print_read_table(rom: &RomImage, table: &ResolvedTable) -> Result<()> {
    let storage = table.storage_type.map_or("?", debug_storage);
    let endian = match table.endian {
        Some(romraider_core::Endian::Big) => "big",
        Some(romraider_core::Endian::Little) => "little",
        None => "?",
    };
    let dims = match (table.size_x, table.size_y) {
        (Some(x), Some(y)) => format!("{x}x{y}"),
        (Some(x), None) => x.to_string(),
        (None, Some(y)) => y.to_string(),
        _ => "?".into(),
    };
    let addr = table
        .storage_address
        .map_or_else(|| "?".into(), |a| format!("{a}"));
    println!(
        "Table {} ({:?}) — {} {} {} @ {}",
        table.name, table.kind, storage, endian, dims, addr,
    );

    let raw = rom.read_table(table).context("reading main table cells")?;
    let scaled_units = table
        .scalings
        .first()
        .and_then(|s| s.units.as_deref())
        .unwrap_or("");
    let scaling = table
        .scalings
        .first()
        .map(romraider_defs::ResolvedScaling::compile)
        .transpose()?;

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
    raw: &[f64],
    scaling: Option<&romraider_defs::CompiledScaling>,
    size_x: Option<u32>,
    units: &str,
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
    rom: &RomImage,
    axis: &ResolvedTable,
    parent_size_x: Option<u32>,
    parent_size_y: Option<u32>,
) {
    let count = match axis.kind {
        Some(romraider_defs::TableKind::XAxis) => parent_size_x,
        Some(romraider_defs::TableKind::YAxis) => parent_size_y,
        _ => None,
    };
    let Some(count) = count.map(|n| n as usize) else {
        println!(
            "\nAxis {} ({:?}): size not derivable from parent — skip",
            axis.name, axis.kind
        );
        return;
    };

    match rom.read_cells(axis, count) {
        Ok(raw) => {
            let units = axis
                .scalings
                .first()
                .and_then(|s| s.units.as_deref())
                .unwrap_or("");
            let scaling = axis.scalings.first().and_then(|s| s.compile().ok());
            println!(
                "\n{:?} {} ({} cells, units={}):",
                axis.kind, axis.name, count, units
            );
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
        println!(
            "  [{:<5}] {:<30} → {:<6} ({})",
            f.kind, f.name, f.metric, f.expr
        );
    }
    println!();
    println!("Log protocols:     {}", doc.logprotocols.logprotocols.len());
    for p in &doc.logprotocols.logprotocols {
        let templates: Vec<_> = p.ecus.iter().filter(|e| e.is_template()).collect();
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
    let metric = p.metric.as_deref().unwrap_or("");
    let expr = p.expr.as_deref().unwrap_or("?");
    let bb = match (p.byte.as_deref(), p.bit.as_deref()) {
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
    output: &PathBuf,
    mcu: &str,
    start: &str,
    length: usize,
    timeout: Duration,
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
        mcu: mcu_family,
        start_addr,
        length: length_val,
        fast_baud: None, // TODO: после Tactrix baud-switch API
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
            let rate = done as f64 / elapsed.max(1e-6);
            let eta_s = (total - done) as f64 / rate.max(1.0);
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
        started.elapsed().as_secs_f64(),
        bytes.len(),
        output.display()
    );
    Ok(())
}

fn peek_rom_cmd(
    start: &str,
    count: usize,
    timeout: Duration,
    skip_init: bool,
    gap_ms: u64,
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
    let bytes =
        ssm::read_addresses(&mut tr, &addresses, 0x00, timeout).context("ReadAddresses failed")?;

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
            .map(|&b| {
                if (0x20..0x7F).contains(&b) {
                    b as char
                } else {
                    '.'
                }
            })
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

/// Стандартные OBD-II 11-bit CAN-ID для «engine ECU» (ECU 1).
const OBD2_ECU_REQUEST_ID: u32 = 0x7E0;
const OBD2_ECU_RESPONSE_ID: u32 = 0x7E8;

#[cfg(feature = "kernel-upload")]
fn dump_rom_can_cmd(
    output: &PathBuf,
    start: &str,
    length: usize,
    chunk_size: u16,
    timeout: Duration,
) -> Result<()> {
    use romraider_kernel::orchestrator::{dump_rom_via_can, DumpProgress};

    let start_addr = parse_int_or_hex_u32(start).with_context(|| format!("--start `{start}`"))?;

    eprintln!("=== Phase A: Open Tactrix in CAN mode ===");
    let mut tr = open_tactrix_can()?;

    let mut last_pct = -1i32;
    let bytes_dumped = dump_rom_via_can(
        &mut tr,
        start_addr,
        length,
        chunk_size,
        timeout,
        |event| match event {
            DumpProgress::PhaseAObdiiWake { mode01_response } => {
                eprintln!("\n=== Phase A: OBD-II wake-up ===");
                eprintln!("  Mode 01 PID 00: {}", bytes::hex_dump(&mode01_response));
            }
            DumpProgress::PhaseAVin { vin_raw } => {
                eprintln!("  Mode 09 PID 02 (VIN): {}", bytes::hex_dump(&vin_raw));
            }
            DumpProgress::PhaseACvn { cvn_raw } => {
                eprintln!("  Mode 09 PID 06 (CVN): {}", bytes::hex_dump(&cvn_raw));
            }
            DumpProgress::PhaseBExtSession => {
                eprintln!("\n=== Phase B: ExtendedDiagnosticSession + SecurityAccess ===");
                eprintln!("  ✅ Extended session active");
            }
            DumpProgress::PhaseBSeed { seed } => {
                eprintln!("  Seed: {}", bytes::hex_dump(&seed));
            }
            DumpProgress::PhaseBKey { key } => {
                eprintln!("  Key (computed): {}", bytes::hex_dump(&key));
            }
            DumpProgress::PhaseBGranted => {
                eprintln!("  ✅ SecurityAccess granted (67 02)");
            }
            DumpProgress::PhaseBProgSession => {
                eprintln!("  ✅ ProgrammingSession active (50 02)");
            }
            DumpProgress::PhaseCUploadStart { total_bytes } => {
                eprintln!("\n=== Phase C: Kernel upload ({total_bytes} bytes encrypted) ===");
            }
            DumpProgress::PhaseCUploadDone { elapsed_secs } => {
                eprintln!("  ✅ Kernel uploaded + jumped in {elapsed_secs:.2}s");
            }
            DumpProgress::PhaseDBanner { banner } => {
                eprintln!("\n=== Phase D: Kernel handshake ===");
                eprintln!("  ✅ Kernel banner: {banner:?}");
            }
            DumpProgress::PhaseEReadStart {
                start_addr,
                total,
                chunk_size,
            } => {
                eprintln!("\n=== Phase E: ROM dump loop ===");
                eprintln!(
                    "  Reading {total} bytes from 0x{start_addr:06X} (chunks of {chunk_size})…"
                );
            }
            DumpProgress::PhaseEReadProgress {
                done,
                total,
                rate_bps,
            } => {
                let pct = (done as i64 * 100 / total.max(1) as i64) as i32;
                if pct != last_pct {
                    eprintln!("  {done}/{total} ({pct}%) — {rate_bps:.0} B/s");
                    last_pct = pct;
                }
            }
            DumpProgress::PhaseEDone {
                bytes_dumped,
                elapsed_secs,
            } => {
                eprintln!("\n✅✅✅ DONE — {bytes_dumped} байт за {elapsed_secs:.1}s");
            }
        },
    )?;

    std::fs::write(output, &bytes_dumped)
        .with_context(|| format!("writing {}", output.display()))?;
    eprintln!("Сохранено в {}", output.display());
    eprintln!("\nProTip: сверь с fixture:");
    eprintln!(
        "    cmp {} fixtures/forester-xt-2007-4E42504007.bin",
        output.display(),
    );
    Ok(())
}

/// Открыть Tactrix в CAN-mode + установить flow-control filter.
///
/// Используется и kernel-upload (CAN dump-rom), и обычным OBD-II логгером
/// (Mode 01 PID polling) — поэтому **не закрыт** feature-flag-ом.
fn open_tactrix_can() -> Result<TactrixTransport> {
    let cfg = TactrixConfig::iso15765_500k();
    eprintln!(
        "Opening Tactrix Openport (VID={:#06X} PID={:#06X}, ISO15765 @ {} baud)…",
        cfg.vid, cfg.pid, cfg.baud,
    );
    let mut tr = TactrixTransport::open(&cfg).context("Tactrix open failed")?;
    eprintln!("Transport: {}", tr.description());
    tr.set_can_flow_control_filter(OBD2_ECU_RESPONSE_ID, OBD2_ECU_REQUEST_ID)
        .context("Set CAN flow-control filter failed")?;
    Ok(tr)
}

#[cfg(feature = "kernel-upload")]
fn peek_seed_cmd(send_key: bool, timeout: Duration) -> Result<()> {
    use romraider_kernel::orchestrator::uds_request;
    use romraider_kernel::seed_key::{subaru_genkey, subaru_genkey_can};

    let mut tr = open_tactrix_can()?;

    // Wake-up sequence по образу EcuFlash. Subaru ECU отвергает прямой
    // `10 03` если перед ним не было OBD-II запросов (NRC 0x22 conditionsNotCorrect).
    eprintln!("\n→ Wake-up: OBD-II Mode 01 PID 00 (supported PIDs)…");
    let resp = uds_request(&mut tr, &[0x01, 0x00], timeout)?;
    eprintln!("  RX UDS: {}", bytes::hex_dump(&resp));
    if resp.first() != Some(&0x41) {
        eprintln!("  ⚠️  Unexpected response, продолжаем");
    }

    eprintln!("\n→ Wake-up: OBD-II Mode 09 PID 02 (VIN)…");
    let resp = uds_request(&mut tr, &[0x09, 0x02], timeout)?;
    eprintln!(
        "  RX UDS ({} bytes): {}",
        resp.len(),
        bytes::hex_dump(&resp)
    );

    eprintln!("\n→ Wake-up: OBD-II Mode 09 PID 06 (CVN)…");
    let resp = uds_request(&mut tr, &[0x09, 0x06], timeout)?;
    eprintln!("  RX UDS: {}", bytes::hex_dump(&resp));

    eprintln!("\n→ SID 0x10 0x03 (ExtendedDiagnosticSession)…");
    let resp = uds_request(&mut tr, &[0x10, 0x03], timeout)?;
    eprintln!("  RX UDS: {}", bytes::hex_dump(&resp));
    if resp.first() == Some(&0x7F) {
        anyhow::bail!(
            "Negative response on StartSession: {}",
            bytes::hex_dump(&resp)
        );
    }
    if resp.first() != Some(&0x50) {
        anyhow::bail!(
            "Unexpected response (expected 0x50): {}",
            bytes::hex_dump(&resp)
        );
    }
    eprintln!("  ✅ Extended diagnostic session active");

    eprintln!("\n→ SID 0x27 0x01 (SecurityAccess RequestSeed)…");
    let resp = uds_request(&mut tr, &[0x27, 0x01], timeout)?;
    eprintln!("  RX UDS: {}", bytes::hex_dump(&resp));
    if resp.len() < 6 || resp[0] != 0x67 || resp[1] != 0x01 {
        anyhow::bail!("Bad RequestSeed response: {}", bytes::hex_dump(&resp));
    }
    let seed: [u8; 4] = [resp[2], resp[3], resp[4], resp[5]];
    eprintln!(
        "\n  Seed: {:02X} {:02X} {:02X} {:02X}",
        seed[0], seed[1], seed[2], seed[3]
    );

    // Сравнение с captured pair из EcuFlash session.
    const CAPTURED_SEED: [u8; 4] = [0xDD, 0xEE, 0xAB, 0x05];
    const CAPTURED_KEY: [u8; 4] = [0x74, 0x15, 0x7C, 0x7D];
    if seed == CAPTURED_SEED {
        eprintln!("  📌 Seed совпадает с captured value (deterministic) →");
        eprintln!(
            "     captured key `{:02X?}` должен сработать для replay-attack",
            CAPTURED_KEY
        );
    } else {
        eprintln!(
            "  ⚠️  Seed отличается от captured `{:02X?}` → seed dynamic,",
            CAPTURED_SEED
        );
        eprintln!("     replay-attack невозможен, нужен real algorithm");
    }

    let computed_key_kline = subaru_genkey(seed);
    let computed_key = subaru_genkey_can(seed);
    eprintln!(
        "\n  K-Line genkey (probably wrong for CAN): {:02X?}",
        computed_key_kline,
    );
    eprintln!(
        "  CAN genkey (per-firmware table from ROM 0x05972C): {:02X?}",
        computed_key,
    );
    if seed == CAPTURED_SEED && computed_key == CAPTURED_KEY {
        eprintln!("  ✅ CAN genkey совпадает с captured key (тест-вектор подтвердился)");
    }

    if !send_key {
        eprintln!("\n(use --send-key чтобы попробовать отправить computed key)");
        return Ok(());
    }

    eprintln!("\n→ SID 0x27 0x02 (SecurityAccess SendKey, computed)…");
    eprintln!("  ⚠️  Если key неверный, ECU засчитает попытку. После 2 невалидных");
    eprintln!("      попыток — lockout 10 секунд.");
    let mut send = Vec::with_capacity(6);
    send.push(0x27);
    send.push(0x02);
    send.extend_from_slice(&computed_key);
    let resp = uds_request(&mut tr, &send, timeout)?;
    eprintln!("  RX UDS: {}", bytes::hex_dump(&resp));
    match resp.as_slice() {
        [0x67, 0x02, ..] => {
            eprintln!(
                "  ✅✅✅ KEY ПРИНЯТ! Algorithm совпадает с CAN-side. Mac-native dump unlocked!"
            );
        }
        [0x7F, 0x27, nrc, ..] => {
            eprintln!(
                "  ❌ ECU отверг key (NRC 0x{:02X}). Algorithm не тот, нужен RE.",
                nrc
            );
        }
        _ => {
            eprintln!("  ❓ Неожиданный response");
        }
    }
    Ok(())
}

/// Quick OBD-II Mode 09 query через ISO15765/CAN. Поддерживает Mode 09 PID 02
/// (VIN), PID 06 (CVN), и т.п. — общая логика wire-protocol-а.
fn peek_obd_cmd(mode: u8, pid: u8, label: &str, timeout: Duration) -> Result<()> {
    let cfg = TactrixConfig::iso15765_500k();
    eprintln!(
        "Opening Tactrix Openport (VID={:#06X} PID={:#06X}, ISO15765 @ {} baud)…",
        cfg.vid, cfg.pid, cfg.baud,
    );
    let mut tr = TactrixTransport::open(&cfg).context("Tactrix open failed")?;
    eprintln!("Transport: {}", tr.description());

    eprintln!(
        "Installing FLOW_CONTROL filter (rx=0x{:08X}, tx=0x{:08X})…",
        OBD2_ECU_RESPONSE_ID, OBD2_ECU_REQUEST_ID,
    );
    tr.set_can_flow_control_filter(OBD2_ECU_RESPONSE_ID, OBD2_ECU_REQUEST_ID)
        .context("Set CAN flow-control filter failed")?;

    // Wire format для att6: <4-byte CAN ID BE><UDS payload>
    let mut tx = Vec::with_capacity(6);
    tx.extend_from_slice(&OBD2_ECU_REQUEST_ID.to_be_bytes());
    tx.push(mode);
    tx.push(pid);
    eprintln!(
        "Sending OBD-II Mode {:02X} PID {:02X} ({label}) on CAN ID 0x{:03X}…",
        mode, pid, OBD2_ECU_REQUEST_ID,
    );
    tr.write_all(&tx, timeout).context("CAN write_all failed")?;

    // Прочитать ответ. Tactrix вернёт single-frame с весь UDS-payload в RxEnd.
    let mut buf = [0u8; 64];
    let n = tr
        .read_frame(&mut buf, timeout)
        .context("CAN read_frame failed")?;
    let raw = &buf[..n];
    eprintln!("\nRaw response ({n} bytes):");
    eprintln!("  {}", bytes::hex_dump(raw));

    // Парсинг: <4-byte CAN_ID BE><49 mode pid><frame_count><data...>
    if raw.len() < 4 + 3 {
        anyhow::bail!("response too short: {n} bytes (expected ≥7)");
    }
    let resp_can_id = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
    if resp_can_id != OBD2_ECU_RESPONSE_ID {
        eprintln!(
            "  ⚠️  CAN ID 0x{:08X} != expected 0x{:08X}",
            resp_can_id, OBD2_ECU_RESPONSE_ID,
        );
    }
    let uds = &raw[4..];
    if uds.len() < 3 || uds[0] != mode | 0x40 {
        anyhow::bail!("unexpected UDS response: {}", bytes::hex_dump(uds));
    }
    if uds[1] != pid {
        anyhow::bail!("PID mismatch: requested 0x{pid:02X}, got 0x{:02X}", uds[1]);
    }
    let payload = &uds[3..]; // skip mode, pid, frame_count
    eprintln!("\n✅ ECU responded:");
    eprintln!("  {label}: {}", bytes::hex_dump(payload));
    let as_ascii: String = payload
        .iter()
        .map(|&b| {
            if (0x20..0x7F).contains(&b) {
                b as char
            } else {
                '.'
            }
        })
        .collect();
    eprintln!("  ASCII: {as_ascii}");
    Ok(())
}

fn print_rom_summary(idx: usize, rom: &RomDefinition) {
    let id = rom.romid.as_ref();
    let xml_id = id.and_then(|r| r.xml_id.as_deref()).unwrap_or("?");
    let ecu_id = id.and_then(|r| r.ecu_id.as_deref()).unwrap_or("-");
    let make = id.and_then(|r| r.make.as_deref()).unwrap_or("-");
    let model = id.and_then(|r| r.model.as_deref()).unwrap_or("-");
    let submodel = id.and_then(|r| r.submodel.as_deref()).unwrap_or("-");
    let base = rom.base.as_deref().unwrap_or("(none)");
    let n_tables = rom.tables.len();
    let n_nested = rom.tables.iter().map(|t| t.nested.len()).sum::<usize>();

    println!("  [{idx}] xmlid={xml_id:<14} ecuid={ecu_id:<12} {make} {model} {submodel}");
    println!("      base={base:<14} tables={n_tables} (nested axes/inner tables: {n_nested})");
}
