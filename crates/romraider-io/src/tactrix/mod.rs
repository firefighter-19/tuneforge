//! Tactrix Openport 2.0 transport (USB bulk).
//!
//! Openport 2.0 — это **не** USB CDC serial, несмотря на FTDI-VID. Это
//! libusb-bulk устройство (VID `0x0403`, PID `0xCC4D`), у которого один
//! интерфейс с парой bulk-эндпоинтов IN/OUT. Сам Tactrix-протокол —
//! ASCII-команды с `\r\n` терминаторами для управления и бинарные
//! frame-ы для данных, плывущих в обе стороны.
//!
//! Эталон реализации: [`dschultzca/j2534`](https://github.com/dschultzca/j2534)
//! — рабочий Linux/macOS-драйвер на C через libusb-1.0, который
//! используется RomRaider как backend на не-Windows платформах.
//!
//! ## Wire-протокол
//!
//! Хост → устройство (ASCII команды, `\r\n` терминатор):
//!
//! ```text
//! ati                          identify
//! ata                          attach
//! ato<P> 0 <baud> 0            open channel; P=0x33 ISO9141 / 0x34 ISO14230
//!                                            / 0x35 CAN / 0x36 ISO15765
//! att<ch> <len> 0\r\n<data>    transmit (followed by `len` raw bytes)
//! atc<ch>                      close channel
//! atz                          close device
//! ```
//!
//! Устройство → хост — два вида ответов:
//!
//! - ASCII: `aro\r\n` (ack), `ari <firmware-info>\r\n` (identify reply)
//! - Бинарный data-фрейм:
//!   ```text
//!   'a' 'r' <proto_byte:1B> <len:1B> <pkt_type:1B> <ts:4B BE> <payload>
//!   ```
//!   где `len` покрывает `pkt_type + ts + payload`, поэтому `payload.len() = len - 5`.
//!
//! Для SSM2 K-Line: `ato52 0 4800 0` (52 dec = 0x34 = ISO-14230 @ 4800 baud).

mod protocol;
mod transport;

pub use protocol::{
    parse_frame, PacketKind, ParseError, TactrixFrame, PROTO_CAN, PROTO_ISO14230, PROTO_ISO15765,
    PROTO_ISO9141,
};
pub use transport::{TactrixConfig, TactrixTransport, TACTRIX_OP2_PID, TACTRIX_VID};

use std::time::Duration;

/// Информация о найденном Tactrix-устройстве (passive enumeration —
/// **без `sudo`**, не пытается claim-нуть интерфейс).
#[derive(Debug, Clone)]
pub struct TactrixDeviceInfo {
    /// USB bus number (информативно, для отладки multi-bus случаев).
    pub bus_number: u8,
    /// USB device address на шине.
    pub address: u8,
    pub vid: u16,
    pub pid: u16,
    /// Из string descriptor-а — `"Tactrix"` если успел прочитать,
    /// `None` если открыть устройство для чтения strings не удалось
    /// (на macOS иногда требует «allow accessory» prompt).
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    pub usb_version: String,
    pub device_version: String,
    pub num_interfaces: u8,
}

/// Перечислить все Tactrix Openport устройства на USB-шине.
///
/// **Не требует root** — это просто `DeviceList::new()` + `device_descriptor()`,
/// что любой user-process может делать на macOS/Linux/Windows. Открытие
/// устройства (для чтения string-descriptors) пытается best-effort: если
/// прав нет — основные поля (VID/PID/bus/addr) всё равно вернутся.
///
/// **Использование**: pre-flight check перед `TactrixTransport::open` —
/// чтобы пользователь сразу знал «не подключён» vs «подключён но нет sudo».
///
/// # Errors
///
/// Только если сам libusb не может enumerate USB-шину (редкое, обычно
/// означает что USB-driver не загружен).
pub fn find_tactrix() -> rusb::Result<Vec<TactrixDeviceInfo>> {
    let mut out = Vec::new();
    for device in rusb::DeviceList::new()?.iter() {
        let desc = match device.device_descriptor() {
            Ok(d) => d,
            Err(_) => continue,
        };
        if desc.vendor_id() != TACTRIX_VID || desc.product_id() != TACTRIX_OP2_PID {
            continue;
        }
        // best-effort string descriptor reading
        let (manufacturer, product, serial) = match device.open() {
            Ok(handle) => {
                let timeout = Duration::from_millis(200);
                let lang = handle
                    .read_languages(timeout)
                    .ok()
                    .and_then(|l| l.first().copied());
                let m = lang.and_then(|l| handle.read_manufacturer_string(l, &desc, timeout).ok());
                let p = lang.and_then(|l| handle.read_product_string(l, &desc, timeout).ok());
                let s = lang.and_then(|l| handle.read_serial_number_string(l, &desc, timeout).ok());
                (m, p, s)
            }
            Err(_) => (None, None, None),
        };
        let usb_version = {
            let v = desc.usb_version();
            format!("{}.{}.{}", v.major(), v.minor(), v.sub_minor())
        };
        let device_version = {
            let v = desc.device_version();
            format!("{}.{}.{}", v.major(), v.minor(), v.sub_minor())
        };
        out.push(TactrixDeviceInfo {
            bus_number: device.bus_number(),
            address: device.address(),
            vid: desc.vendor_id(),
            pid: desc.product_id(),
            manufacturer,
            product,
            serial,
            usb_version,
            device_version,
            num_interfaces: desc.num_configurations(),
        });
    }
    Ok(out)
}
