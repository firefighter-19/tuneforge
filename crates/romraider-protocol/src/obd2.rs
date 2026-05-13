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
pub const OBD_REQUEST_ID:  u32 = 0x7E0;
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
    pub name:  &'static str,
    /// PID-байт.
    pub pid:   u8,
    /// Сколько data-байтов ECU вернёт после `41 <pid>`.
    pub bytes: usize,
    /// Scaling: raw bytes → real value. Гарантировано получает `bytes` байт.
    pub scale: fn(&[u8]) -> f64,
    /// Единицы измерения, для CSV/UI.
    pub units: &'static str,
}

/// Стандартные SAE J1979 Mode 01 PID-ы, обычно полезные для тюнинга
/// Subaru. Pid-ы 0x01–0x4F гарантированно поддерживаются почти всеми
/// OBD-II ECU; некоторые (0x42 Control Module Voltage, 0x46 Ambient Temp)
/// поддерживаются не всегда — в этом случае `read_pid()` вернёт ошибку.
pub const STANDARD_PIDS: &[ObdiiPid] = &[
    ObdiiPid {
        name:  "Engine Load",
        pid:   0x04, bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0,
        units: "%",
    },
    ObdiiPid {
        name:  "Coolant Temp",
        pid:   0x05, bytes: 1,
        scale: |b| b[0] as f64 - 40.0,
        units: "C",
    },
    ObdiiPid {
        name:  "Fuel Pressure",
        pid:   0x0A, bytes: 1,
        scale: |b| b[0] as f64 * 3.0,
        units: "kPa",
    },
    ObdiiPid {
        name:  "MAP",
        pid:   0x0B, bytes: 1,
        scale: |b| b[0] as f64,
        units: "kPa",
    },
    ObdiiPid {
        name:  "RPM",
        pid:   0x0C, bytes: 2,
        scale: |b| (b[0] as f64 * 256.0 + b[1] as f64) / 4.0,
        units: "RPM",
    },
    ObdiiPid {
        name:  "Vehicle Speed",
        pid:   0x0D, bytes: 1,
        scale: |b| b[0] as f64,
        units: "km/h",
    },
    ObdiiPid {
        name:  "Timing Advance",
        pid:   0x0E, bytes: 1,
        scale: |b| (b[0] as f64 / 2.0) - 64.0,
        units: "deg",
    },
    ObdiiPid {
        name:  "IAT",
        pid:   0x0F, bytes: 1,
        scale: |b| b[0] as f64 - 40.0,
        units: "C",
    },
    ObdiiPid {
        name:  "MAF",
        pid:   0x10, bytes: 2,
        scale: |b| (b[0] as f64 * 256.0 + b[1] as f64) / 100.0,
        units: "g/s",
    },
    ObdiiPid {
        name:  "TPS",
        pid:   0x11, bytes: 1,
        scale: |b| b[0] as f64 * 100.0 / 255.0,
        units: "%",
    },
    ObdiiPid {
        name:  "Run Time",
        pid:   0x1F, bytes: 2,
        scale: |b| b[0] as f64 * 256.0 + b[1] as f64,
        units: "s",
    },
    ObdiiPid {
        name:  "Battery Voltage",
        pid:   0x42, bytes: 2,
        scale: |b| (b[0] as f64 * 256.0 + b[1] as f64) / 1000.0,
        units: "V",
    },
    ObdiiPid {
        name:  "Ambient Temp",
        pid:   0x46, bytes: 1,
        scale: |b| b[0] as f64 - 40.0,
        units: "C",
    },
];

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
    tr:      &mut T,
    pid:     u8,
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
            got:      n,
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
}
