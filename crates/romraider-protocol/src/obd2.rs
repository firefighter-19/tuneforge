//! OBD-II Mode 01 (current data) over ISO15765 / CAN @ 500 kbps.
//!
//! Стандартный путь для логирования на 2007+ Subaru: SSM2 `ReadAddresses`
//! через K-Line на этих ECU **режется анти-fuzz защитой** (отвечает только
//! на ECU_INIT, на 0xA8 silent), и реальный datalogging EcuFlash идёт
//! через CAN OBD-II. Mode 01 PID-ы — стандарт SAE J1979, поддержан **любым**
//! OBD-II-совместимым ECU независимо от производителя.
//!
//! Wire-формат:
//! - **Запрос:**  `01 <PID>`              (через CAN ID `0x7E0`)
//! - **Ответ:**   `41 <PID> <data...>`    (через CAN ID `0x7E8`)
//!
//! Несколько PID-ов можно отправить в одном запросе (`01 0C 05 10 ...`,
//! max 6 PID-ов из-за лимита single-frame CAN). Для простоты этот модуль
//! шлёт по одному — оверхед при 500 kbps пренебрежимо мал, проще debugging.

use std::time::Duration;

use romraider_io::transport::Transport;

use crate::error::{ProtocolError, ProtocolResult};

/// CAN OBD-II 11-bit ECU **request** ID.
pub const OBD_REQUEST_ID: u32 = 0x7E0;
/// CAN OBD-II 11-bit ECU **response** ID.
pub const OBD_RESPONSE_ID: u32 = 0x7E8;

/// Mode 01 response SID (= request SID `0x01` | `0x40`).
pub const MODE_01_RESPONSE: u8 = 0x41;

/// Стандартное обозначение **«PID не поддерживается ECU»** (NRC из OBD-II spec).
/// При запросе неподдерживаемого PID-а ECU отвечает `7F 01 12` (Sub-function
/// not supported) или просто silent.
pub const NRC_PID_NOT_SUPPORTED: u8 = 0x12;

/// Один Mode 01 параметр: PID, размер data-payload-а, scaling, units.
#[derive(Debug, Clone, Copy)]
pub struct ObdiiPid {
    /// Человеческое имя (`"RPM"`, `"Coolant Temp"`).
    pub name: &'static str,
    /// PID-байт.
    pub pid: u8,
    /// Сколько data-байтов ECU вернёт после `41 <pid>`.
    pub bytes: usize,
    /// Scaling: raw bytes → real value. Гарантировано получает `bytes` байт.
    pub scale: fn(&[u8]) -> f64,
    /// Единицы измерения, для CSV/UI.
    pub units: &'static str,
}

/// Стандартные SAE J1979 Mode 01 PID-ы. Покрывают большинство полезных
/// для тюнинга/диагностики параметров в диапазоне 0x01–0x51. Реальный
/// набор поддержки ECU вычисляется через [`discover_supported_pids`].
///
/// Spec: <https://en.wikipedia.org/wiki/OBD-II_PIDs> (тот же текст что
/// SAE J1979 standard, но без paywall-а).
pub const STANDARD_PIDS: &[ObdiiPid] = &[
    // ── Status / monitor bitmaps (raw uint, для interpretation см. SAE J1979) ──
    ObdiiPid {
        name: "Monitor Status", // PID 0x01: 4-byte bitmap, MIL+DTC count+test readiness.
        pid: 0x01,
        bytes: 4,
        scale: |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64,
        units: "raw",
    },
    ObdiiPid {
        name: "Fuel System Status", // PID 0x03: byte A=bank1 state, byte B=bank2 state (enum).
        pid: 0x03,
        bytes: 2,
        scale: |b| b[0] as f64 * 256.0 + b[1] as f64,
        units: "raw",
    },
    // ── Fuel system & basic engine ───────────────────────────────────────
    ObdiiPid {
        name: "Engine Load",
        pid: 0x04,
        bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0,
        units: "%",
    },
    ObdiiPid {
        name: "Coolant Temp",
        pid: 0x05,
        bytes: 1,
        scale: |b| b[0] as f64 - 40.0,
        units: "C",
    },
    ObdiiPid {
        name: "STFT B1", // Short-Term Fuel Trim, Bank 1
        pid: 0x06,
        bytes: 1,
        scale: |b| (b[0] as f64 - 128.0) * 100.0 / 128.0,
        units: "%",
    },
    ObdiiPid {
        name: "LTFT B1", // Long-Term Fuel Trim, Bank 1
        pid: 0x07,
        bytes: 1,
        scale: |b| (b[0] as f64 - 128.0) * 100.0 / 128.0,
        units: "%",
    },
    ObdiiPid {
        name: "STFT B2",
        pid: 0x08,
        bytes: 1,
        scale: |b| (b[0] as f64 - 128.0) * 100.0 / 128.0,
        units: "%",
    },
    ObdiiPid {
        name: "LTFT B2",
        pid: 0x09,
        bytes: 1,
        scale: |b| (b[0] as f64 - 128.0) * 100.0 / 128.0,
        units: "%",
    },
    ObdiiPid {
        name: "Fuel Pressure",
        pid: 0x0A,
        bytes: 1,
        scale: |b| b[0] as f64 * 3.0,
        units: "kPa",
    },
    ObdiiPid {
        name: "MAP",
        pid: 0x0B,
        bytes: 1,
        scale: |b| b[0] as f64,
        units: "kPa",
    },
    ObdiiPid {
        name: "RPM",
        pid: 0x0C,
        bytes: 2,
        scale: |b| (b[0] as f64 * 256.0 + b[1] as f64) / 4.0,
        units: "RPM",
    },
    ObdiiPid {
        name: "Vehicle Speed",
        pid: 0x0D,
        bytes: 1,
        scale: |b| b[0] as f64,
        units: "km/h",
    },
    ObdiiPid {
        name: "Timing Advance",
        pid: 0x0E,
        bytes: 1,
        scale: |b| (b[0] as f64 / 2.0) - 64.0,
        units: "deg",
    },
    ObdiiPid {
        name: "IAT",
        pid: 0x0F,
        bytes: 1,
        scale: |b| b[0] as f64 - 40.0,
        units: "C",
    },
    ObdiiPid {
        name: "MAF",
        pid: 0x10,
        bytes: 2,
        scale: |b| (b[0] as f64 * 256.0 + b[1] as f64) / 100.0,
        units: "g/s",
    },
    ObdiiPid {
        name: "TPS",
        pid: 0x11,
        bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0,
        units: "%",
    },
    ObdiiPid {
        name: "Secondary Air Status", // PID 0x12: enum (1=upstream, 2=downstream, 4=off, 8=pump on)
        pid: 0x12,
        bytes: 1,
        scale: |b| b[0] as f64,
        units: "raw",
    },
    ObdiiPid {
        name: "O2 Sensors Present 2B", // PID 0x13: bitmap of installed O2 in 2 banks
        pid: 0x13,
        bytes: 1,
        scale: |b| b[0] as f64,
        units: "raw",
    },
    // ── O2 sensors (Bank 1) ──────────────────────────────────────────────
    // PIDs 0x14–0x1B: каждый возвращает [voltage_byte, stft_byte].
    // voltage = A/200 (V), stft = (B-128)*100/128 (%).
    // Если STFT == 0xFF → датчик не используется для closed-loop.
    ObdiiPid {
        name: "O2 B1S1 Voltage",
        pid: 0x14,
        bytes: 2,
        scale: |b| b[0] as f64 / 200.0,
        units: "V",
    },
    ObdiiPid {
        name: "O2 B1S2 Voltage",
        pid: 0x15,
        bytes: 2,
        scale: |b| b[0] as f64 / 200.0,
        units: "V",
    },
    ObdiiPid {
        name: "OBD Standards", // PID 0x1C: enum (1=OBD-II Calif, 3=OBD/OBD-II, 6=EOBD, ...)
        pid: 0x1C,
        bytes: 1,
        scale: |b| b[0] as f64,
        units: "raw",
    },
    // ── Run time + chain bridge ──────────────────────────────────────────
    ObdiiPid {
        name: "Run Time",
        pid: 0x1F,
        bytes: 2,
        scale: |b| b[0] as f64 * 256.0 + b[1] as f64,
        units: "s",
    },
    // ── Emissions + secondary range (0x21–0x40) ──────────────────────────
    ObdiiPid {
        name: "Distance with MIL",
        pid: 0x21,
        bytes: 2,
        scale: |b| b[0] as f64 * 256.0 + b[1] as f64,
        units: "km",
    },
    ObdiiPid {
        name: "Fuel Rail Pressure (vac)",
        pid: 0x22,
        bytes: 2,
        scale: |b| (b[0] as f64 * 256.0 + b[1] as f64) * 0.079,
        units: "kPa",
    },
    ObdiiPid {
        name: "Fuel Rail Pressure",
        pid: 0x23,
        bytes: 2,
        scale: |b| (b[0] as f64 * 256.0 + b[1] as f64) * 10.0,
        units: "kPa",
    },
    ObdiiPid {
        // PID 0x24: wide-range O2 sensor 1, 4 bytes. A,B = lambda*2/65536;
        // C,D = voltage*8/65536. Шкалируем только lambda (главное значение
        // для тюнинга closed-loop).
        name: "O2 B1S1 Wideband Lambda",
        pid: 0x24,
        bytes: 4,
        scale: |b| (b[0] as f64 * 256.0 + b[1] as f64) * 2.0 / 65536.0,
        units: "lambda",
    },
    ObdiiPid {
        // PID 0x34: тот же wide-range S1, но с current вместо voltage.
        // C,D = current*(1/256)-128 mA. Опять-таки лямбда главное.
        name: "O2 B1S1 Wideband Lambda (I)",
        pid: 0x34,
        bytes: 4,
        scale: |b| (b[0] as f64 * 256.0 + b[1] as f64) * 2.0 / 65536.0,
        units: "lambda",
    },
    ObdiiPid {
        name: "Commanded EGR",
        pid: 0x2C,
        bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0,
        units: "%",
    },
    ObdiiPid {
        name: "EGR Error",
        pid: 0x2D,
        bytes: 1,
        scale: |b| (b[0] as f64 - 128.0) * 100.0 / 128.0,
        units: "%",
    },
    ObdiiPid {
        name: "Commanded Evap Purge",
        pid: 0x2E,
        bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0,
        units: "%",
    },
    ObdiiPid {
        name: "Fuel Tank Level",
        pid: 0x2F,
        bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0,
        units: "%",
    },
    ObdiiPid {
        name: "Warm-ups Since Cleared",
        pid: 0x30,
        bytes: 1,
        scale: |b| b[0] as f64,
        units: "count",
    },
    ObdiiPid {
        name: "Distance Since Cleared",
        pid: 0x31,
        bytes: 2,
        scale: |b| b[0] as f64 * 256.0 + b[1] as f64,
        units: "km",
    },
    ObdiiPid {
        name: "Barometric Pressure",
        pid: 0x33,
        bytes: 1,
        scale: |b| b[0] as f64,
        units: "kPa",
    },
    // ── Third range (0x41–0x60) ──────────────────────────────────────────
    ObdiiPid {
        name: "Monitor Status DC", // PID 0x41: this drive cycle, same encoding как 0x01
        pid: 0x41,
        bytes: 4,
        scale: |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64,
        units: "raw",
    },
    ObdiiPid {
        name: "Battery Voltage",
        pid: 0x42,
        bytes: 2,
        scale: |b| (b[0] as f64 * 256.0 + b[1] as f64) / 1000.0,
        units: "V",
    },
    ObdiiPid {
        name: "Absolute Load",
        pid: 0x43,
        bytes: 2,
        scale: |b| (b[0] as f64 * 256.0 + b[1] as f64) * 100.0 / 255.0,
        units: "%",
    },
    ObdiiPid {
        name: "Commanded AFR",
        pid: 0x44,
        bytes: 2,
        scale: |b| (b[0] as f64 * 256.0 + b[1] as f64) * 2.0 / 65536.0,
        units: "lambda",
    },
    ObdiiPid {
        name: "Relative TPS",
        pid: 0x45,
        bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0,
        units: "%",
    },
    ObdiiPid {
        name: "Ambient Temp",
        pid: 0x46,
        bytes: 1,
        scale: |b| b[0] as f64 - 40.0,
        units: "C",
    },
    ObdiiPid {
        name: "Absolute TPS B",
        pid: 0x47,
        bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0,
        units: "%",
    },
    ObdiiPid {
        name: "Accel Pedal D",
        pid: 0x49,
        bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0,
        units: "%",
    },
    ObdiiPid {
        name: "Accel Pedal E",
        pid: 0x4A,
        bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0,
        units: "%",
    },
    ObdiiPid {
        name: "Commanded Throttle",
        pid: 0x4C,
        bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0,
        units: "%",
    },
    ObdiiPid {
        name: "Time with MIL",
        pid: 0x4D,
        bytes: 2,
        scale: |b| b[0] as f64 * 256.0 + b[1] as f64,
        units: "min",
    },
    ObdiiPid {
        name: "Time Since Cleared",
        pid: 0x4E,
        bytes: 2,
        scale: |b| b[0] as f64 * 256.0 + b[1] as f64,
        units: "min",
    },
    ObdiiPid {
        name: "Fuel Type", // PID 0x51: enum (1=Gas, 4=Diesel, 5=LPG, ...)
        pid: 0x51,
        bytes: 1,
        scale: |b| b[0] as f64,
        units: "raw",
    },
    ObdiiPid {
        name: "Relative Accel Pedal",
        pid: 0x5A,
        bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0,
        units: "%",
    },
];

/// Опросить ECU по chain Mode 01 PID 0x00/0x20/0x40/... и вернуть список
/// **всех PID-ов, которые ECU объявляет поддерживаемыми**.
///
/// Каждый «supported-PIDs» PID отвечает 4-байтовым bitmap-ом:
/// - bit 31 (MSB) = `<probe_pid> + 1` поддержан/нет
/// - bit  0 (LSB) = `<probe_pid> + 0x20` — этот же PID одновременно
///   служит «next-chain-query». Если LSB=1 → надо запросить `<probe_pid+0x20>`
///   чтобы получить bitmap для следующих 32-х PID-ов; LSB=0 → конец chain-а.
///
/// Возвращает отсортированный список PID-байтов. Включает сам chain-PID
/// (`0x20`, `0x40`, …) если он поддержан — это **корректно**, потому что эти
/// PID-ы возвращают реальные данные (supported-bitmap).
pub fn discover_supported_pids<T: Transport + ?Sized>(
    tr: &mut T,
    timeout: Duration,
) -> ProtocolResult<Vec<u8>> {
    let mut supported = Vec::new();
    let mut probe_pid: u8 = 0x00;
    loop {
        let bitmap_bytes = read_pid(tr, probe_pid, timeout)?;
        if bitmap_bytes.len() < 4 {
            break;
        }
        let bitmap = u32::from_be_bytes([
            bitmap_bytes[0],
            bitmap_bytes[1],
            bitmap_bytes[2],
            bitmap_bytes[3],
        ]);
        for i in 0..32u8 {
            if bitmap & (1u32 << (31 - i)) != 0 {
                supported.push(probe_pid + i + 1);
            }
        }
        // LSB sigaled "next 32-PID range supported" — продолжаем chain.
        if bitmap & 1 == 0 {
            break;
        }
        match probe_pid.checked_add(0x20) {
            Some(next) => probe_pid = next,
            None => break,
        }
        // safety: max 8 ranges = 256 PIDs.
        if probe_pid >= 0xE0 {
            break;
        }
    }
    Ok(supported)
}

/// Сколько PID-ов в нашей встроенной таблице **и** в списке ECU-supported.
/// Удобно для рапорта в `--list --probe`.
#[must_use]
pub fn known_supported(supported: &[u8]) -> Vec<&'static ObdiiPid> {
    STANDARD_PIDS
        .iter()
        .filter(|p| supported.contains(&p.pid))
        .collect()
}

/// Найти PID по человеческому имени (case-insensitive).
#[must_use]
pub fn find_pid(name: &str) -> Option<&'static ObdiiPid> {
    STANDARD_PIDS
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
}

/// Построить wire-frame для Mode 01 single-PID запроса: `01 <PID>`.
#[must_use]
pub fn build_mode_01(pid: u8) -> [u8; 2] {
    [0x01, pid]
}

/// Парс Mode 01 response payload (без CAN_ID prefix) → data-байты.
///
/// Принимает то, что приходит **после** strip-а CAN_ID transport-ом:
/// `41 <PID> <data...>`. Возвращает `&data`.
pub fn parse_mode_01(pid: u8, response: &[u8]) -> ProtocolResult<&[u8]> {
    if response.len() < 2 {
        return Err(ProtocolError::ResponseTooShort {
            got: response.len(),
            expected: 2,
        });
    }
    if response[0] != MODE_01_RESPONSE {
        return Err(ProtocolError::UnexpectedResponse(response[0]));
    }
    if response[1] != pid {
        return Err(ProtocolError::UnexpectedResponse(response[1]));
    }
    Ok(&response[2..])
}

/// High-level: послать `01 <pid>` через CAN, дождаться `41 <pid> <data>`,
/// вернуть data-байты (без `41 <pid>` echo).
///
/// Подразумевает CAN-transport (Tactrix `ato6` ISO15765 @ 500kbps + flow-
/// control filter). Transport должен возвращать `<CAN_ID 4B BE><UDS bytes>`
/// в `read_frame` (как уже сделано в [`romraider_io::tactrix`]).
pub fn read_pid<T: Transport + ?Sized>(
    tr: &mut T,
    pid: u8,
    timeout: Duration,
) -> ProtocolResult<Vec<u8>> {
    // TX = <CAN_ID 4B BE> + `01 <pid>` (Tactrix txflags=64 FRAME_PAD дополнит до 8 байт).
    let mut tx = Vec::with_capacity(4 + 2);
    tx.extend_from_slice(&OBD_REQUEST_ID.to_be_bytes());
    tx.extend_from_slice(&build_mode_01(pid));
    tr.write_all(&tx, timeout)?;

    let mut buf = [0u8; 256];
    let n = tr.read_frame(&mut buf, timeout)?;
    if n < 4 {
        return Err(ProtocolError::ResponseTooShort {
            got: n,
            expected: 4,
        });
    }
    // Validate CAN response ID
    let resp_id = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if resp_id != OBD_RESPONSE_ID {
        return Err(ProtocolError::UnexpectedResponse(buf[0]));
    }
    let uds = &buf[4..n];
    // Negative response: `7F 01 <NRC>` — PID не поддерживается / неверный режим.
    if uds.len() >= 3 && uds[0] == 0x7F {
        return Err(ProtocolError::UnexpectedResponse(uds[2]));
    }
    let data = parse_mode_01(pid, uds)?;
    Ok(data.to_vec())
}

/// Max PIDs в одном batched Mode 01 запросе. Per SAE J1979, ECU обязан
/// принимать до 6 PIDs (`01 <pid1>..<pid6>` = 7 байт = max ISO-TP single-frame).
/// Subaru ECU 2007+ принимает ровно 6, проверено живьём на 2007 Forester XT.
pub const MAX_BATCH_PIDS: usize = 6;

/// Один результат batched-чтения: `pid → raw data bytes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchedPidValue {
    pub pid: u8,
    pub data: Vec<u8>,
}

/// **Slice 24b — batched read**. Шлёт `01 <PID1> <PID2> ... <PIDN>`
/// (`N` ≤ [`MAX_BATCH_PIDS`]), дождаться `41 <pid_a> <data_a> <pid_b> ...`,
/// вернуть распарсенный список (`pid`, `data`).
///
/// **Парсинг by-echo**: каждый встреченный байт `<pid>` ищем в списке
/// запрошенных, на основании [`ObdiiPid::bytes`] потребляем нужное
/// количество data-байтов. Robust к **ordering changes** ECU и к
/// **частичным ответам** (ECU dropped non-supported PIDs).
///
/// `pid_defs` — список `ObdiiPid` для всех запрошенных PIDов. Используется
/// для знания `bytes`-counts (нужно при парсинге). Все элементы должны
/// принадлежать [`STANDARD_PIDS`].
///
/// **Multi-frame ISO-TP**: для 6 PIDs (`41 <pid> <data> × 6` ≈ 14-19 байт)
/// ответ ECU будет multi-frame ISO-TP. Tactrix-transport уже это handles
/// transparently — мы получаем уже-склеенный UDS payload.
pub fn read_pids_batched<T: Transport + ?Sized>(
    tr: &mut T,
    pid_defs: &[&ObdiiPid],
    timeout: Duration,
) -> ProtocolResult<Vec<BatchedPidValue>> {
    if pid_defs.is_empty() {
        return Err(ProtocolError::ResponseTooShort {
            got: 0,
            expected: 1,
        });
    }
    if pid_defs.len() > MAX_BATCH_PIDS {
        return Err(ProtocolError::ResponseTooShort {
            got: pid_defs.len(),
            expected: MAX_BATCH_PIDS,
        });
    }

    // Build wire: <CAN_ID 4B BE> 01 <pid1> <pid2> ... <pidN>
    let mut tx = Vec::with_capacity(4 + 1 + pid_defs.len());
    tx.extend_from_slice(&OBD_REQUEST_ID.to_be_bytes());
    tx.push(0x01);
    for p in pid_defs {
        tx.push(p.pid);
    }
    tr.write_all(&tx, timeout)?;

    let mut buf = [0u8; 256];
    let n = tr.read_frame(&mut buf, timeout)?;
    if n < 4 + 2 {
        return Err(ProtocolError::ResponseTooShort {
            got: n,
            expected: 4 + 2,
        });
    }
    let resp_id = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if resp_id != OBD_RESPONSE_ID {
        return Err(ProtocolError::UnexpectedResponse(buf[0]));
    }
    let uds = &buf[4..n];
    parse_mode_01_batched(uds, pid_defs)
}

/// Парс multi-PID Mode 01 response payload (без CAN_ID prefix-а):
/// `41 <pid_a> <data_a> <pid_b> <data_b> ...`. Извлечён как pure-функция
/// из [`read_pids_batched`] чтобы unit-тестить без hardware.
pub fn parse_mode_01_batched(
    uds: &[u8],
    pid_defs: &[&ObdiiPid],
) -> ProtocolResult<Vec<BatchedPidValue>> {
    if uds.len() < 2 {
        return Err(ProtocolError::ResponseTooShort {
            got: uds.len(),
            expected: 2,
        });
    }
    // Negative response: `7F 01 <NRC>` — например NRC `0x13` (Incorrect Format)
    // если ECU не любит batched запрос с слишком многими PIDs.
    if uds[0] == 0x7F {
        let nrc = uds.get(2).copied().unwrap_or(0);
        return Err(ProtocolError::UnexpectedResponse(nrc));
    }
    if uds[0] != MODE_01_RESPONSE {
        return Err(ProtocolError::UnexpectedResponse(uds[0]));
    }

    let mut out = Vec::with_capacity(pid_defs.len());
    let mut i = 1; // skip `41` echo
    while i < uds.len() {
        let pid = uds[i];
        i += 1;
        // Найти определение PID-а в нашем requested-наборе.
        let Some(def) = pid_defs.iter().find(|p| p.pid == pid) else {
            // Unknown PID echo — может быть padding-байт (`0x00`) или ECU
            // вернул лишний PID. Если 0x00 — конец полезных данных, выход.
            // Иначе — лог-warn и continue (но потеряем sync; для robustness
            // лучше break).
            if pid == 0x00 {
                break;
            }
            tracing::warn!(
                pid = format!("{:02X}", pid),
                "unexpected PID echo in batched response (not in request)"
            );
            break;
        };
        // Извлечь bytes data-байт.
        if i + def.bytes > uds.len() {
            return Err(ProtocolError::ResponseTooShort {
                got: uds.len() - i,
                expected: def.bytes,
            });
        }
        let data = uds[i..i + def.bytes].to_vec();
        i += def.bytes;
        out.push(BatchedPidValue { pid, data });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_pids_are_unique_and_have_valid_scales() {
        let mut seen: std::collections::HashSet<u8> = std::collections::HashSet::new();
        for p in STANDARD_PIDS {
            assert!(seen.insert(p.pid), "duplicate PID 0x{:02X}", p.pid);
            // Smoke-check scaling не паникует на minimum bytes.
            let zeros = vec![0u8; p.bytes];
            let _ = (p.scale)(&zeros);
        }
    }

    #[test]
    fn rpm_pid_scales_correctly() {
        let rpm = find_pid("RPM").unwrap();
        // 0x0F * 256 + 0xA0 = 4000 raw → 1000 RPM.
        assert!(((rpm.scale)(&[0x0F, 0xA0]) - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn coolant_pid_scales_correctly() {
        let coolant = find_pid("Coolant Temp").unwrap();
        // raw 120 → 80°C.
        assert!(((coolant.scale)(&[120]) - 80.0).abs() < 1e-9);
    }

    #[test]
    fn battery_pid_scales_correctly() {
        let batt = find_pid("Battery Voltage").unwrap();
        // 14123 → 14.123 V.
        assert!(((batt.scale)(&[0x37, 0x2B]) - 14.123).abs() < 1e-9);
    }

    #[test]
    fn find_pid_is_case_insensitive() {
        assert!(find_pid("rpm").is_some());
        assert!(find_pid("RPM").is_some());
        assert!(find_pid("Coolant temp").is_some());
        assert!(find_pid("Unknown").is_none());
    }

    #[test]
    fn parse_mode_01_strips_echo() {
        let resp = [0x41, 0x0C, 0x0F, 0xA0];
        let data = parse_mode_01(0x0C, &resp).unwrap();
        assert_eq!(data, &[0x0F, 0xA0]);
    }

    #[test]
    fn parse_mode_01_rejects_wrong_pid() {
        let resp = [0x41, 0x05, 0x50];
        assert!(parse_mode_01(0x0C, &resp).is_err());
    }

    #[test]
    fn parse_batched_decodes_mixed_lengths() {
        // Запрашиваем 3 PIDa: RPM (2B) + Coolant (1B) + MAF (2B).
        let rpm = find_pid("RPM").unwrap();
        let cool = find_pid("Coolant Temp").unwrap();
        let maf = find_pid("MAF").unwrap();
        let pids = &[rpm, cool, maf];

        // Synthetic response: 41 0C <hi> <lo> 05 <val> 10 <hi> <lo>
        // RPM raw 4000 (= 1000 RPM); Coolant raw 120 (= 80°C); MAF raw 500 (= 5.0 g/s).
        let uds = [
            0x41, // Mode 01 response
            0x0C, 0x0F, 0xA0, // RPM = 4000
            0x05, 120, // Coolant
            0x10, 0x01, 0xF4, // MAF = 500
        ];
        let parsed = parse_mode_01_batched(&uds, pids).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].pid, 0x0C);
        assert_eq!(parsed[0].data, vec![0x0F, 0xA0]);
        assert_eq!(parsed[1].pid, 0x05);
        assert_eq!(parsed[1].data, vec![120]);
        assert_eq!(parsed[2].pid, 0x10);
        assert_eq!(parsed[2].data, vec![0x01, 0xF4]);
    }

    #[test]
    fn parse_batched_handles_reordered_response() {
        // ECU вернул PIDы не в том порядке что запрос (legal per spec) —
        // парсер должен правильно разобрать by-echo.
        let rpm = find_pid("RPM").unwrap();
        let tps = find_pid("TPS").unwrap();
        let pids = &[rpm, tps];

        // Запрос: RPM, TPS; ответ: TPS first, RPM after.
        let uds = [0x41, 0x11, 0x80, 0x0C, 0x0F, 0xA0];
        let parsed = parse_mode_01_batched(&uds, pids).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].pid, 0x11);
        assert_eq!(parsed[1].pid, 0x0C);
    }

    #[test]
    fn parse_batched_rejects_nrc() {
        let rpm = find_pid("RPM").unwrap();
        // NRC `7F 01 13` (Incorrect Message Length/Format) — ECU не любит batch.
        let uds = [0x7F, 0x01, 0x13];
        let err = parse_mode_01_batched(&uds, &[rpm]).unwrap_err();
        // Должно вернуться 0x13 NRC через UnexpectedResponse.
        match err {
            ProtocolError::UnexpectedResponse(nrc) => assert_eq!(nrc, 0x13),
            _ => panic!("expected UnexpectedResponse(0x13), got {err:?}"),
        }
    }

    #[test]
    fn parse_batched_stops_on_zero_pad() {
        // Tactrix иногда оставляет padding-байты после полезных данных.
        let rpm = find_pid("RPM").unwrap();
        let uds = [0x41, 0x0C, 0x0F, 0xA0, 0x00, 0x00, 0x00];
        let parsed = parse_mode_01_batched(&uds, &[rpm]).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].data, vec![0x0F, 0xA0]);
    }
}
