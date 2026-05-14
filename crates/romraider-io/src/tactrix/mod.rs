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
