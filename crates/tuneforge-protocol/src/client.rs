//! Унифицированный ECU-клиент абстрагирующий протокол (K-Line SSM2 vs SSM3-CAN).
//!
//! Зачем это: у нас исторически две параллельные API-поверхности для одной и
//! той же логики Subaru-SSM — `ssm::{ecu_init, read_block, dump_rom}` для
//! K-Line и `subaru::{ecu_init_can, read_block_can, …}` для CAN. Подписи
//! пересекаются на 90 %, но возвращают разные типы (`EcuInitResponse` vs
//! `Vec<u8>`), из-за чего CLI приходится держать два набора команд
//! (`dump-rom` vs `dump-rom-can`) с дублирующим кодом.
//!
//! [`EcuClient`] вытаскивает общий интерфейс наверх. Транспорт + протокол
//! инкапсулирован за trait-объектом, CLI просто получает `Box<dyn EcuClient>`
//! на основе `--protocol auto|kline|can|kernel` флага.
//!
//! Семантически семейство SSM одно: ECU init возвращает 3 байта SSM ID,
//! 5 байт ROM ID и capability bitmap — байт-в-байт идентично между K-Line и
//! CAN-вариантами. Различия только на wire-уровне (envelope/framing).

use std::time::Duration;

use tuneforge_core::Address;
use tuneforge_io::transport::Transport;

use crate::error::ProtocolResult;
use crate::ssm::{self, EcuInitResponse};
use crate::subaru;

/// Унифицированный интерфейс к Subaru-ECU независимо от транспорта.
///
/// **Семейство:** только SSM (Subaru Select Monitor). Для других вендоров
/// (BMW DS2, Nissan NCS) — отдельный trait, у них принципиально другая
/// семантика ECU init и адресации.
pub trait EcuClient {
    /// Человекочитаемое имя протокола (для логов и ошибок).
    fn description(&self) -> &'static str;

    /// Послать ECU init и распарсить ответ.
    ///
    /// Возвращает SSM ID, ROM ID и capability bitmap. Должен быть выполнен
    /// **первым** в сессии — на CAN без `ecu_init_can` любая `0xAx`-команда
    /// блокируется через NRC `0x12`.
    fn init(&mut self, timeout: Duration) -> ProtocolResult<EcuInitResponse>;

    /// Прочитать `count` последовательных байт начиная с `addr`.
    ///
    /// Имплементация сама решает chunking на wire-уровне (multi-frame ISO-TP,
    /// P3 guard time и т.п.). Caller просто указывает «прочитай N байт».
    fn read_block(&mut self, addr: u32, count: usize, timeout: Duration)
        -> ProtocolResult<Vec<u8>>;

    /// Дамп `length` байт начиная с `start`, по `chunk_size` за один запрос
    /// (real-life: 128 для K-Line, 32-64 для CAN из-за ISO-TP overhead).
    /// После каждой удачной пачки зовётся `progress(bytes_read, total)`.
    ///
    /// Дефолтная имплементация — цикл [`read_block`]; конкретные клиенты
    /// могут override-ить если у них есть native bulk-read команда с
    /// собственным timing-ом (например, K-Line с P3 guard time).
    fn dump_rom(
        &mut self,
        start: u32,
        length: usize,
        chunk_size: usize,
        timeout: Duration,
        progress: &mut dyn FnMut(usize, usize),
    ) -> ProtocolResult<Vec<u8>> {
        if length == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(length);
        progress(0, length);
        while out.len() < length {
            let remaining = length - out.len();
            let n = remaining.min(chunk_size);
            let addr = start + out.len() as u32;
            let chunk = self.read_block(addr, n, timeout)?;
            out.extend_from_slice(&chunk);
            progress(out.len(), length);
        }
        Ok(out)
    }
}

// ─── K-Line SSM2 ──────────────────────────────────────────────────────────

/// SSM2 поверх serial K-Line (4800 baud). Работает через `SerialTransport`
/// или Tactrix в `PROTO_ISO9141` режиме.
///
/// Поддерживается Subaru 2002-2006. На 2007+ ECU блокирует K-Line через
/// анти-fuzz, нужен kernel-upload путь.
pub struct KLineSsmClient {
    transport: Box<dyn Transport>,
}

impl KLineSsmClient {
    #[must_use]
    pub fn new(transport: Box<dyn Transport>) -> Self {
        Self { transport }
    }

    /// Доступ к нижележащему транспорту для специальных случаев (например,
    /// смена baud-rate перед kernel-handshake-ом).
    pub fn transport_mut(&mut self) -> &mut dyn Transport {
        &mut *self.transport
    }
}

impl EcuClient for KLineSsmClient {
    fn description(&self) -> &'static str {
        "K-Line SSM2 (Subaru 2002-2006)"
    }

    fn init(&mut self, timeout: Duration) -> ProtocolResult<EcuInitResponse> {
        ssm::ecu_init(&mut *self.transport, timeout)
    }

    fn read_block(
        &mut self,
        addr: u32,
        count: usize,
        timeout: Duration,
    ) -> ProtocolResult<Vec<u8>> {
        ssm::read_block(&mut *self.transport, Address::new(addr), count, timeout)
    }

    /// Override: используем `ssm::dump_rom` который вставляет P3 guard time
    /// (50 ms) между блоками — реальные K-Line ECU не успевают за
    /// back-to-back-ReadBlock иначе.
    fn dump_rom(
        &mut self,
        start: u32,
        length: usize,
        chunk_size: usize,
        timeout: Duration,
        progress: &mut dyn FnMut(usize, usize),
    ) -> ProtocolResult<Vec<u8>> {
        ssm::dump_rom(
            &mut *self.transport,
            Address::new(start),
            length,
            chunk_size,
            timeout,
            progress,
        )
    }
}

// ─── SSM3 over CAN ────────────────────────────────────────────────────────

/// SSM3 поверх ISO15765/CAN (500 kbps). Через Tactrix в `PROTO_ISO15765`
/// режиме. Tactrix сам делает ISO-TP framing (First/Consecutive/Flow Control).
///
/// Поддерживается Subaru 2007+. На «default»-сессии можно читать tuner-grade
/// параметры (Knock, A/F learning, AVCS), но ROM-адреса (`0xFFFF*`)
/// блокируются анти-fuzz-ом — нужен kernel-upload.
pub struct CanSsmClient {
    transport: Box<dyn Transport>,
}

impl CanSsmClient {
    #[must_use]
    pub fn new(transport: Box<dyn Transport>) -> Self {
        Self { transport }
    }

    pub fn transport_mut(&mut self) -> &mut dyn Transport {
        &mut *self.transport
    }
}

impl EcuClient for CanSsmClient {
    fn description(&self) -> &'static str {
        "Subaru SSM3 over CAN (2007+)"
    }

    fn init(&mut self, timeout: Duration) -> ProtocolResult<EcuInitResponse> {
        let raw = subaru::ecu_init_can(&mut *self.transport, timeout)?;
        // Wire-формат `EA <ECU info bytes>` — после `EA`-echo идут ровно те же
        // байты что у K-Line ECU init: 3 SSM ID + 5 ROM ID + capability bitmap.
        ssm::decode_ecu_init(&raw)
    }

    fn read_block(
        &mut self,
        addr: u32,
        count: usize,
        timeout: Duration,
    ) -> ProtocolResult<Vec<u8>> {
        subaru::read_block_can(&mut *self.transport, addr, count, timeout)
    }
}

/// Быстрая проверка «ECU отвечает на этом клиенте?» — дёргает [`EcuClient::init`]
/// с заданным таймаутом и swallow-ит ошибку. Возвращает `Some(info)` если
/// ECU ответил, `None` если нет ответа в timeout-е.
///
/// Использование в CLI auto-detect: открыть транспорт в K-Line режиме →
/// `probe(...)` → если `None`, переоткрыть в CAN режиме → `probe(...)`.
/// Init безопасен (read-only), последствий для ECU не имеет.
pub fn probe(client: &mut dyn EcuClient, timeout: Duration) -> Option<EcuInitResponse> {
    client.init(timeout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuneforge_io::mock::MockTransport;

    // ── K-Line wire-helpers (дублирует pack_response из ssm::tests; держим
    //    локально чтобы не делать `pub(crate)` ради тестов) ───────────────
    const HEADER: u8 = 0x80;
    const TOOL_ADDR: u8 = 0xF0;
    const ECU_ADDR: u8 = 0x10;

    fn kline_checksum(bytes: &[u8]) -> u8 {
        bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b))
    }

    /// Собрать K-Line кадр-ответ ECU вокруг `inner` (cmd-echo + data).
    fn pack_kline_response(inner: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(inner.len() + 5);
        frame.push(HEADER);
        frame.push(TOOL_ADDR);
        frame.push(ECU_ADDR);
        frame.push(u8::try_from(inner.len()).expect("inner < 255"));
        frame.extend_from_slice(inner);
        frame.push(kline_checksum(&frame));
        frame
    }

    /// Собрать CAN кадр-ответ ECU: `<response_id_BE>` + `<command_echo>` + `data`.
    fn pack_can_response(data: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(4 + data.len());
        frame.extend_from_slice(&subaru::CAN_RESPONSE_ID.to_be_bytes());
        frame.extend_from_slice(data);
        frame
    }

    fn timeout() -> Duration {
        Duration::from_millis(100)
    }

    // ── K-Line client ──────────────────────────────────────────────────

    #[test]
    fn kline_init_parses_ssm_rom_capabilities() {
        // `0xFF` = response byte для `EcuInit` (`0xBF | 0x40`).
        let mut inner = vec![0xFF];
        inner.extend_from_slice(&[0xAA, 0xBB, 0xCC]); // SSM ID
        inner.extend_from_slice(b"AB123"); // ROM ID
        inner.extend_from_slice(&[0xF1, 0x02]); // capabilities
        let resp = pack_kline_response(&inner);

        let mock = MockTransport::with_responses([resp]);
        let mut client = KLineSsmClient::new(Box::new(mock));
        let info = client.init(timeout()).unwrap();

        assert_eq!(info.ssm_id, [0xAA, 0xBB, 0xCC]);
        assert_eq!(&info.rom_id, b"AB123");
        assert_eq!(info.capabilities, vec![0xF1, 0x02]);
    }

    #[test]
    fn kline_read_block_returns_payload() {
        // `0xE0` = response byte для `ReadBlock` (`0xA0 | 0x40`).
        let mut inner = vec![0xE0];
        inner.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        let resp = pack_kline_response(&inner);

        let mock = MockTransport::with_responses([resp]);
        let mut client = KLineSsmClient::new(Box::new(mock));
        let bytes = client.read_block(0x0001_0000, 4, timeout()).unwrap();

        assert_eq!(bytes, vec![0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn kline_dump_rom_loops_with_progress() {
        // Two chunks of 4 bytes each → 8 bytes total.
        let mut r1 = vec![0xE0];
        r1.extend_from_slice(&[0xAA; 4]);
        let mut r2 = vec![0xE0];
        r2.extend_from_slice(&[0xBB; 4]);

        let mock =
            MockTransport::with_responses([pack_kline_response(&r1), pack_kline_response(&r2)]);
        let mut client = KLineSsmClient::new(Box::new(mock));

        let mut progress_calls: Vec<(usize, usize)> = Vec::new();
        // chunk_size = 4 → length = 8 → 2 итерации.
        // NB: ssm::dump_rom вставит 50 ms P3-guard между итерациями, для
        // unit-теста это терпимо.
        let out = client
            .dump_rom(0x0001_0000, 8, 4, timeout(), &mut |done, total| {
                progress_calls.push((done, total));
            })
            .unwrap();

        assert_eq!(out, vec![0xAA, 0xAA, 0xAA, 0xAA, 0xBB, 0xBB, 0xBB, 0xBB]);
        // Первый вызов с (0, 8), потом (4, 8), потом (8, 8).
        assert_eq!(progress_calls.first(), Some(&(0, 8)));
        assert_eq!(progress_calls.last(), Some(&(8, 8)));
    }

    // ── CAN client ─────────────────────────────────────────────────────

    #[test]
    fn can_init_parses_same_ecu_init_layout_as_kline() {
        // `0xEA` = response для CAN ECU init.
        let mut data = vec![0xEA];
        data.extend_from_slice(&[0x12, 0x34, 0x56]); // SSM ID
        data.extend_from_slice(b"NB504"); // ROM ID
        data.extend_from_slice(&[0xFF, 0x80, 0x00]); // capabilities
        let resp = pack_can_response(&data);

        let mock = MockTransport::with_responses([resp]);
        let mut client = CanSsmClient::new(Box::new(mock));
        let info = client.init(timeout()).unwrap();

        assert_eq!(info.ssm_id, [0x12, 0x34, 0x56]);
        assert_eq!(&info.rom_id, b"NB504");
        assert_eq!(info.capabilities, vec![0xFF, 0x80, 0x00]);
    }

    #[test]
    fn can_read_block_returns_bytes() {
        // CAN: запрашиваем 3 байта → шлём 3 address-а → response `0xE8 <b1> <b2> <b3>`.
        let mut data = vec![0xE8];
        data.extend_from_slice(&[0xDE, 0xAD, 0xBE]);
        let resp = pack_can_response(&data);

        let mock = MockTransport::with_responses([resp]);
        let mut client = CanSsmClient::new(Box::new(mock));
        let bytes = client.read_block(0x0000_8000, 3, timeout()).unwrap();

        assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE]);
    }

    #[test]
    fn can_dump_rom_uses_default_loop() {
        // 2 chunks of 2 bytes via default trait dump_rom.
        let resp1 = pack_can_response(&[0xE8, 0x01, 0x02]);
        let resp2 = pack_can_response(&[0xE8, 0x03, 0x04]);

        let mock = MockTransport::with_responses([resp1, resp2]);
        let mut client = CanSsmClient::new(Box::new(mock));

        let mut progress_calls = Vec::new();
        let out = client
            .dump_rom(0x0000_2000, 4, 2, timeout(), &mut |d, t| {
                progress_calls.push((d, t));
            })
            .unwrap();

        assert_eq!(out, vec![0x01, 0x02, 0x03, 0x04]);
        assert_eq!(progress_calls.first(), Some(&(0, 4)));
        assert_eq!(progress_calls.last(), Some(&(4, 4)));
    }

    // ── Trait object compatibility ─────────────────────────────────────

    #[test]
    fn ecu_client_is_dyn_compatible() {
        // Compile-time check: trait должен быть object-safe, иначе CLI
        // не сможет хранить `Box<dyn EcuClient>` после `--protocol` диспатча.
        fn accept_dyn(_: Box<dyn EcuClient>) {}
        let mock = MockTransport::new();
        accept_dyn(Box::new(KLineSsmClient::new(Box::new(mock))));
    }

    // ── probe() helper ─────────────────────────────────────────────────

    #[test]
    fn probe_returns_some_on_ecu_response() {
        let mut inner = vec![0xFF];
        inner.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        inner.extend_from_slice(b"AB123");
        let mock = MockTransport::with_responses([pack_kline_response(&inner)]);
        let mut client = KLineSsmClient::new(Box::new(mock));
        let result = probe(&mut client, timeout());
        assert!(result.is_some());
        assert_eq!(&result.unwrap().rom_id, b"AB123");
    }

    #[test]
    fn probe_returns_none_on_silent_transport() {
        let mock = MockTransport::new(); // pending пустой → ReadTimeout
        let mut client = KLineSsmClient::new(Box::new(mock));
        assert!(probe(&mut client, timeout()).is_none());
    }
}
