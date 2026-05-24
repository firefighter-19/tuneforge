//! Subaru-specific proprietary extensions поверх ISO15765/CAN.
//!
//! Современные Subaru-ECU (2008+, многие 2007 USDM включая Forester XT)
//! предоставляют **SSM-команды через CAN-bus** в дополнение к стандартному
//! OBD-II Mode 01 — это и есть то, что Java-RomRaider называет «SSM3» и
//! использует для Subaru-specific tuner-grade параметров (knock correction,
//! A/F learning, boost target и т.п., адреса берутся из `<ecuparams>`
//! upstream `logger.xml`).
//!
//! Wire-формат идентичен SSM2 K-Line, но **без `80 10 F0 <len>` outer-wrap**-а
//! — CAN сам framing-ит:
//!
//! ```text
//! ── ReadAddresses (cmd 0xA8) ─────────────────────────────────────────
//!   TX (CAN 0x7E0):   A8 <pad=00> <addr_1 3B BE> <addr_2 3B BE> ...
//!   RX (CAN 0x7E8):   E8 <data_1> <data_2> ...  (по байту на адрес)
//! ```
//!
//! Адреса 24-bit big-endian (как в SSM2). ECU подставляет верхний `0xFF`
//! при доступе к RAM (`0xFFFF`-region на SH7058). Т.е. чтобы прочитать
//! `0xFFFF7664`, шлём `FF 76 64`.

use std::time::Duration;

use romraider_io::transport::Transport;

use crate::error::{ProtocolError, ProtocolResult};

/// CAN OBD-II 11-bit request/response IDs (те же что у UDS/OBD-II).
pub const CAN_REQUEST_ID:  u32 = 0x7E0;
pub const CAN_RESPONSE_ID: u32 = 0x7E8;

/// Subaru proprietary SSM commands (CAN-вариант). Внимание: некоторые
/// отличаются от K-Line SSM2! Особенно `ECU_INIT`: K-Line = `0xBF`, CAN = `0xAA`.
pub const CMD_ECU_INIT_CAN:        u8 = 0xAA;
pub const RESP_ECU_INIT_CAN:       u8 = 0xEA;
pub const CMD_READ_ADDRESSES:      u8 = 0xA8;
pub const RESP_READ_ADDRESSES:     u8 = 0xE8;
pub const CMD_READ_BLOCK:          u8 = 0xA0;
pub const RESP_READ_BLOCK:         u8 = 0xE0;

/// SSM-CAN ECU init: `AA` → `EA <ECU info bytes>`. **Должен** быть выполнен
/// **первым** в SSM-сессии — без него ECU режет любые другие `0xAx`-команды
/// через NRC `0x12 (Sub-Function Not Supported / Invalid Format)`.
///
/// Возвращает `ECU info` bytes (без `EA` echo) — это ROM/SSM ID + capability
/// bitmap, как у K-Line ECU init (`BF`/`FF`).
pub fn ecu_init_can<T: Transport + ?Sized>(
    tr:      &mut T,
    timeout: Duration,
) -> ProtocolResult<Vec<u8>> {
    let mut tx = Vec::with_capacity(4 + 1);
    tx.extend_from_slice(&CAN_REQUEST_ID.to_be_bytes());
    tx.push(CMD_ECU_INIT_CAN);
    tr.write_all(&tx, timeout)?;

    let mut buf = [0u8; 256];
    let n = tr.read_frame(&mut buf, timeout)?;
    if n < 4 + 1 {
        return Err(ProtocolError::ResponseTooShort {
            got:      n,
            expected: 4 + 1,
        });
    }
    let resp_id = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if resp_id != CAN_RESPONSE_ID {
        return Err(ProtocolError::UnexpectedResponse(buf[0]));
    }
    let uds = &buf[4..n];
    if uds.len() >= 3 && uds[0] == 0x7F {
        return Err(ProtocolError::UnexpectedResponse(uds[2]));
    }
    if uds[0] != RESP_ECU_INIT_CAN {
        return Err(ProtocolError::UnexpectedResponse(uds[0]));
    }
    Ok(uds[1..].to_vec())
}

/// Прочитать несколько RAM-байтов ECU по 24-bit SSM-адресам через CAN.
///
/// Каждый адрес возвращает ровно один байт (это SSM2-семантика — для
/// 4-byte float-параметра caller вызывает функцию с 4-мя последовательными
/// адресами и собирает результат как BE-float).
///
/// Pad byte = `0x00` (SSM2 default; единственный вариант что мы видели
/// в captured Wireshark-traffic для нашего ECU).
pub fn read_addresses_can<T: Transport + ?Sized>(
    tr:        &mut T,
    addresses: &[u32],
    timeout:   Duration,
) -> ProtocolResult<Vec<u8>> {
    if addresses.is_empty() {
        return Err(ProtocolError::ResponseTooShort { got: 0, expected: 1 });
    }
    let mut tx = Vec::with_capacity(4 + 2 + 3 * addresses.len());
    tx.extend_from_slice(&CAN_REQUEST_ID.to_be_bytes());
    tx.push(CMD_READ_ADDRESSES);
    tx.push(0x00); // pad
    for addr in addresses {
        tx.push(((*addr >> 16) & 0xFF) as u8);
        tx.push(((*addr >> 8)  & 0xFF) as u8);
        tx.push((*addr         & 0xFF) as u8);
    }
    tracing::debug!(tx = ?tx, "SSM-CAN ReadAddresses TX");
    tr.write_all(&tx, timeout)?;

    let mut buf = [0u8; 256];
    let n = tr.read_frame(&mut buf, timeout)?;
    tracing::debug!(rx = ?&buf[..n], "SSM-CAN ReadAddresses RX");
    if n < 4 + 1 + addresses.len() {
        eprintln!("  RX raw ({} bytes): {:02X?}", n, &buf[..n]);
        return Err(ProtocolError::ResponseTooShort {
            got:      n,
            expected: 4 + 1 + addresses.len(),
        });
    }
    let resp_id = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if resp_id != CAN_RESPONSE_ID {
        eprintln!("  RX raw ({} bytes): {:02X?}", n, &buf[..n]);
        return Err(ProtocolError::UnexpectedResponse(buf[0]));
    }
    let uds = &buf[4..n];
    // Negative response: `7F A8 <NRC>`.
    if uds.len() >= 3 && uds[0] == 0x7F {
        eprintln!(
            "  NRC: SID 0x{:02X} → 0x{:02X} (raw UDS: {:02X?})",
            uds[1], uds[2], uds,
        );
        return Err(ProtocolError::UnexpectedResponse(uds[2]));
    }
    if uds[0] != RESP_READ_ADDRESSES {
        eprintln!("  RX uds ({} bytes): {:02X?}", uds.len(), uds);
        return Err(ProtocolError::UnexpectedResponse(uds[0]));
    }
    if uds.len() < 1 + addresses.len() {
        return Err(ProtocolError::ResponseTooShort {
            got:      uds.len(),
            expected: 1 + addresses.len(),
        });
    }
    Ok(uds[1..1 + addresses.len()].to_vec())
}

/// Helper: прочитать **N последовательных байтов** начиная с 24-bit адреса.
/// Удобно для float-параметров (N=4) или uint16 (N=2).
pub fn read_block_can<T: Transport + ?Sized>(
    tr:      &mut T,
    base:    u32,
    len:     usize,
    timeout: Duration,
) -> ProtocolResult<Vec<u8>> {
    let addresses: Vec<u32> = (0..len as u32).map(|i| base + i).collect();
    read_addresses_can(tr, &addresses, timeout)
}

/// Один SSM-параметр из стандартного `<parameters>`-блока RomRaider
/// `logger.xml`. Эти параметры **защищены capability-bitmap-ом из ECU init**
/// (`AA`/`EA`), но **доступны** в default session (в отличие от `<ecuparams>`,
/// которые на 2007+ Subaru блокируются анти-fuzz-ом → NRC `0x12`).
#[derive(Debug, Clone, Copy)]
pub struct SsmParam {
    /// RomRaider ID (`"P8"`, `"P23"`).
    pub id:      &'static str,
    /// Человеческое имя.
    pub name:    &'static str,
    /// 24-bit базовый адрес. Многобайтные параметры (uint16) занимают
    /// `address`..`address+bytes-1`.
    pub address: u32,
    /// Сколько байт (обычно 1, для uint16 = 2).
    pub bytes:   usize,
    /// Scaling: raw → real value.
    pub scale:   fn(&[u8]) -> f64,
    /// Единицы измерения.
    pub units:   &'static str,
}

/// Тщательно подобранный набор стандартных SSM-параметров (subset из 156).
/// Целенаправленно для диагностики knock retard / lean condition / boost
/// на Subaru 2007+ (Forester XT / Impreza WRX и т.п.).
pub const SUBARU_SSM_PARAMS: &[SsmParam] = &[
    // ── Engine state ─────────────────────────────────────────────────
    SsmParam {
        id: "P2", name: "Coolant Temp", address: 0x000008, bytes: 1,
        scale: |b| b[0] as f64 - 40.0, units: "C",
    },
    SsmParam {
        id: "P8", name: "RPM", address: 0x00000E, bytes: 2,
        scale: |b| (b[0] as u16 as f64 * 256.0 + b[1] as f64) / 4.0,
        units: "RPM",
    },
    SsmParam {
        id: "P11", name: "IAT", address: 0x000012, bytes: 1,
        scale: |b| b[0] as f64 - 40.0, units: "C",
    },
    SsmParam {
        id: "P12", name: "MAF", address: 0x000013, bytes: 2,
        scale: |b| (b[0] as u16 as f64 * 256.0 + b[1] as f64) / 100.0,
        units: "g/s",
    },
    SsmParam {
        id: "P9", name: "Vehicle Speed", address: 0x000010, bytes: 1,
        scale: |b| b[0] as f64, units: "km/h",
    },
    SsmParam {
        id: "P13", name: "TPS", address: 0x000015, bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0, units: "%",
    },
    SsmParam {
        id: "P17", name: "Battery Voltage", address: 0x00001C, bytes: 1,
        scale: |b| b[0] as f64 * 8.0 / 100.0, units: "V",
    },
    SsmParam {
        id: "P1", name: "Engine Load Relative", address: 0x000007, bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0, units: "%",
    },
    // ── Boost ────────────────────────────────────────────────────────
    SsmParam {
        id: "P7", name: "MAP", address: 0x00000D, bytes: 1,
        // Subaru: `x*37/255` PSI абсолютного (вычитай ~14.5 для буста).
        scale: |b| b[0] as f64 * 37.0 / 255.0, units: "PSI abs",
    },
    SsmParam {
        id: "P36", name: "Primary WGDC", address: 0x000030, bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0, units: "%",
    },
    // ── Fuel mix (lean/rich) — для перегрева критично ────────────────
    SsmParam {
        id: "P3", name: "A/F Correction #1 (STFT)", address: 0x000009, bytes: 1,
        scale: |b| (b[0] as f64 - 128.0) * 100.0 / 128.0, units: "%",
    },
    SsmParam {
        id: "P4", name: "A/F Learning #1 (LTFT)", address: 0x00000A, bytes: 1,
        scale: |b| (b[0] as f64 - 128.0) * 100.0 / 128.0, units: "%",
    },
    SsmParam {
        id: "P58", name: "A/F Sensor #1", address: 0x000046, bytes: 1,
        // Subaru wide-range frontend: `x * 14.7 / 128` = AFR.
        // (Stoich = 14.7 при raw=128.)
        scale: |b| b[0] as f64 * 14.7 / 128.0, units: "AFR",
    },
    // ── Knock — главное для провала и перегрева ──────────────────────
    SsmParam {
        id: "P10", name: "Ignition Total Timing", address: 0x000011, bytes: 1,
        scale: |b| (b[0] as f64 - 128.0) / 2.0, units: "deg BTDC",
    },
    SsmParam {
        id: "P23", name: "Knock Correction", address: 0x000022, bytes: 1,
        // (x-128)/2 — отрицательные значения = retard, главный indicator
        // того, что ECU тянет тайминг.
        scale: |b| (b[0] as f64 - 128.0) / 2.0, units: "deg",
    },
    SsmParam {
        id: "P91", name: "Fine Learning Knock Correction", address: 0x000199, bytes: 1,
        scale: |b| (b[0] as f64 - 128.0) / 2.0, units: "deg",
    },
    // ── AVCS (intake cam timing — для проверки ремня ГРМ / AVCS-актуатора) ──
    // Idle прогретый: оба около 0° (commanded). Если actual постоянно
    // off-set-нут на N° от commanded — либо ремень смещён, либо AVCS
    // плохо реагирует (масло/oil-control valve), либо CMP-датчик врёт.
    SsmParam {
        id: "P48", name: "Intake AVCS Right", address: 0x00003C, bytes: 1,
        scale: |b| b[0] as f64 - 50.0, units: "deg",
    },
    SsmParam {
        id: "P49", name: "Intake AVCS Left", address: 0x00003D, bytes: 1,
        scale: |b| b[0] as f64 - 50.0, units: "deg",
    },
];

/// Найти SSM-параметр по ID (`"P8"`) или имени (case-insensitive).
#[must_use]
pub fn find_ssm_param(key: &str) -> Option<&'static SsmParam> {
    SUBARU_SSM_PARAMS.iter().find(|p| {
        p.id.eq_ignore_ascii_case(key) || p.name.eq_ignore_ascii_case(key)
    })
}

/// Производный (computed) параметр — не читается с ECU напрямую, а
/// вычисляется из других [`SsmParam`] значений через `compute()`.
///
/// Используется для diagnostic-ratios которые ECU сам не выставляет, но
/// полезны при разборе симптомов (например `AVCS Diff = R - L` для
/// детекции стуканного AVCS oil-control valve).
#[derive(Debug, Clone, Copy)]
pub struct SsmDerivedParam {
    /// Человеческое имя для CSV/preview.
    pub name: &'static str,
    /// Список имён [`SsmParam`] от которых зависит. Caller должен
    /// гарантировать что все эти params в подписке (raw values посчитаны).
    pub depends_on: &'static [&'static str],
    /// Вычисление: принимает scaled values тех же имён в том же порядке
    /// что `depends_on`. Возвращает derived value.
    pub compute: fn(&[f64]) -> f64,
    pub units: &'static str,
}

/// Производные параметры для tuning-диагностики. Все они **дешёвые**
/// (просто арифметика по уже-полученным byte values), не добавляют
/// никаких extra ECU-запросов.
pub const SUBARU_DERIVED_PARAMS: &[SsmDerivedParam] = &[
    // AVCS R - L: если ≥3° в transients → залипший OCV на правом банке
    // (типичная failure после 10+ лет). На stable steady-state должно
    // быть ≤1° (оба банка в одной позиции).
    SsmDerivedParam {
        name:       "AVCS Diff (R-L)",
        depends_on: &["Intake AVCS Right", "Intake AVCS Left"],
        compute:    |v| v[0] - v[1],
        units:      "deg",
    },
    // Boost gauge = MAP абсолютное минус atmospheric (~14.5 psi на
    // уровне моря). Полезно для quick «сколько буста сейчас» без
    // вычитания в голове.
    SsmDerivedParam {
        name:       "Boost (gauge)",
        depends_on: &["MAP"],
        compute:    |v| v[0] - 14.5,
        units:      "PSI",
    },
];

/// Найти derived-параметр по имени (case-insensitive).
#[must_use]
pub fn find_derived_param(name: &str) -> Option<&'static SsmDerivedParam> {
    SUBARU_DERIVED_PARAMS
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case(name))
}

/// Опросить N SSM-параметров за **один** Mode 0xA8 запрос (батчинг).
/// Возвращает по `Vec<u8>` на параметр (длиной `param.bytes` каждый),
/// в том же порядке что и `params`.
pub fn read_ssm_params_can<T: Transport + ?Sized>(
    tr:      &mut T,
    params:  &[&'static SsmParam],
    timeout: Duration,
) -> ProtocolResult<Vec<Vec<u8>>> {
    let mut all_addrs: Vec<u32> = Vec::new();
    let mut starts:    Vec<usize> = Vec::with_capacity(params.len());
    for p in params {
        starts.push(all_addrs.len());
        for i in 0..p.bytes {
            all_addrs.push(p.address + i as u32);
        }
    }
    let raw = read_addresses_can(tr, &all_addrs, timeout)?;
    let mut out = Vec::with_capacity(params.len());
    for (i, p) in params.iter().enumerate() {
        out.push(raw[starts[i]..starts[i] + p.bytes].to_vec());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_constants_are_correct() {
        assert_eq!(CMD_READ_ADDRESSES, 0xA8);
        assert_eq!(RESP_READ_ADDRESSES, CMD_READ_ADDRESSES | 0x40);
    }
}
