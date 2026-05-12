# romraider-io — Progress

## Цель

Транспортный слой: байтовый канал к ECU или посреднику (cable/dongle).
Реализации `Transport`-трейта подменяемы — высокоуровневые протоколы из
`romraider-protocol` не знают, что под ними: реальный COM-порт, ELM327-донгл
или mock.

## Эталон (Java RomRaider) — `com.romraider.io.*` (59 файлов, 11274 строк)

### `connection/*` (5 файлов) — общие свойства подключения
- `ConnectionManager.java` — интерфейс «открой канал»
- `ConnectionManagerFactory.java` — диспетчер по типу транспорта
- `ConnectionProperties.java` — базовые свойства (baud, timeout)
- `KwpConnectionProperties.java`, `KwpSerialConnectionProperties.java` — KWP-2000 настройки
- `SerialConnectionProperties.java` — параметры serial-порта

### `serial/*` (8 файлов) — последовательный порт
- `connection/SerialConnection.java`, `SerialConnectionImpl.java`, `SerialConnectionManager.java`
- `connection/TestSerialConnection.java`, `TestSerialConnection2.java` — моки
- `port/SerialPortDiscoverer.java`, `SerialPortDiscovererImpl.java` — `lsblk`/Windows COM enum
- `port/SerialPortRefreshListener.java`, `SerialPortRefresher.java` — авто-обновление списка портов

### `elm327/*` (2 файла) — ELM327 OBD-донгл
- `ElmConnection.java`
- `ElmConnectionManager.java`

### `j2534/api/*` (15 файлов) — драйверы программаторов
- `J2534.java` — главный интерфейс
- `J2534Impl.java` — реализация через JNA
- `J2534Library.java`, `J2534LibraryLocator.java` — поиск нативной DLL
- `J2534_v0404.java` — конкретная версия SAE J2534
- `J2534TransportFactory.java`
- `J2534ConnectionISO14230.java`, `ISO15765.java`, `ISO9141.java` — диалекты
- `ConfigItem.java`, `J2534Exception.java`, `Version.java`
- `TestJ2534.java`, `TestJ2534Can.java`, `TestJ2534IsoTp.java`, `TestJ2534NCS.java`, `TestJ2534OBD.java` — тестовые утилиты

### `protocol/*` — переехало в **отдельный крейт `romraider-protocol`**
См. [`../romraider-protocol/PROGRESS.md`](../romraider-protocol/PROGRESS.md).

## Статус

| Фича                                  | Java-аналог                            | Rust                              | Статус |
| ------------------------------------- | -------------------------------------- | --------------------------------- | :----: |
| `Transport` trait (write/read/purge)  | `ConnectionManager`                    | `transport.rs`                    |   ✅   |
| Serial-порт (open, read, write)       | `SerialConnectionImpl`                 | `serial.rs` (через `serialport`)  |   ✅   |
| `SerialConfig` defaults (SSM 4800-8N1)| `SerialConnectionProperties`           | `serial::SerialConfig::ssm`       |   ✅   |
| Enum доступных COM-портов             | `SerialPortDiscovererImpl`             | через CLI команду `ports` (используем `serialport::available_ports`) | ✅ |
| Auto-refresh списка портов            | `SerialPortRefresher`                  | —                                 |   ⏸   |
| ELM327 — AT-команды                   | `ElmConnection`                        | `elm327::Elm327::at`              |   ✅   |
| ELM327 — кадрирование `>`-prompt      | inline в `ElmConnection`               | `Elm327::read_until_prompt`       |   ✅   |
| ELM327 — auto-detect протокола (ATSP) | в `ElmConnectionManager`               | —                                 |   ❌   |
| J2534 — динамическая загрузка DLL/SO  | `J2534LibraryLocator`                  | `j2534::Library::open` (libloading) |  ✅   |
| **Tactrix Openport 2.0 (USB-bulk)**   | через J2534 DLL (Windows-only в апстриме) | `tactrix::TactrixTransport` через rusb (libusb), нативно на Mac ARM (CDC ACM, claim iface 1) | ✅ |
| Tactrix protocol парсер               | inline в Tactrix DLL                   | `tactrix::protocol::parse_frame` + 15 unit-тестов (ack/identify/error/filter-ack/K-line bin/CAN bin) | ✅ |
| Tactrix handshake (ati/ata/ato/atf)    | inline в Tactrix DLL                   | `TactrixTransport::open` (ISO-9141 + `NO_CHECKSUM`-flag для SSM K-line) |  ✅   |
| Tactrix K-line channel reset (atc/ato/atf cycle) | в J2534-stack-е              | `TactrixTransport::reset_channel` — без re-open USB, для session-per-chunk dump | ✅ |
| Live SSM ECU-init на 2007 Forester XT | через RomRaider Java                   | подтверждено 2026-05-11 (ROM `4E42504007`)  |  ✅   |
| J2534 — vtable PassThru функций       | `J2534_v0404` через JNA                | `j2534::api::PassThruVtable`      |   ✅   |
| J2534 — Open/Close устройство         | `J2534Impl.open/close`                 | `Device::open` (stub, unimplemented) | 🟡   |
| J2534 — Connect/Disconnect канал      | `J2534Impl.connect/disconnect`         | —                                 |   ❌   |
| J2534 — WriteMsgs/ReadMsgs            | `J2534Impl.writeMsgs/readMsgs`         | —                                 |   ❌   |
| J2534 — Ioctl (filters, timing)       | `J2534Impl.ioctl`                      | —                                 |   ❌   |
| J2534 — поиск установленных программаторов | `J2534LibraryLocator` через Windows registry | —                          |   ❌   |
| J2534 — ISO14230/15765/9141 wrappers  | `J2534Connection*.java`                | —                                 |   ❌   |
| KWP-2000 connection properties        | `KwpConnectionProperties`              | —                                 |   ⏸   |
| `MockTransport` (для тестов)          | `TestSerialConnection`                 | `mock::MockTransport` (feature `mock`) | ✅ |
| Структурированные ошибки              | `J2534Exception` etc                   | `IoError` enum                    |   ✅   |

## Что точно НЕ переносим

- Свойства per-OS pinout в `SerialConnectionProperties` — у нас все
  параметры в `SerialConfig`, OS-логика на стороне `serialport` crate
- `TestJ2534*` (тестовые executables) — заменяются `cargo test`

## Важные нюансы, найденные при live-тестировании (Slice 19-20)

- **Subaru SSM K-line через Tactrix требует `ISO-9141` + `ISO9141_NO_CHECKSUM` (flag 0x200)**,
  не `ISO-14230`. KWP-2000 framing ломает SSM. См. `J2534ConnectionISO9141.java` в Java-апстриме.
- **Channel-ID в Tactrix wire-protocol == protocol-id**: `aro` после `ato` приходит без digit,
  присвоение делает J2534-обёртка (`*pChannelID = protocolID;` в dschultzca/j2534). Все
  последующие команды (`att`, `atc`, `atf`) используют тот же ID, что и protocol.
- **Обязателен `atf` PASS-фильтр** перед любым TX/RX, иначе Tactrix молча выкидывает RX.
- **K-line бинарные кадры не содержат timestamp** (только CAN/ISO15765); payload идёт
  сразу после kind-байта. См. `parse_binary` в `tactrix::protocol`.
- **Длинные K-line ответы (>32 байт) приходят несколькими `NORM_MSG`-чанками**, обрамлёнными
  `NORM_MSG_START_IND (0x80)` и `RX_MSG_END_IND (0x40)`. `TactrixTransport::read_frame`
  склеивает их в один логический фрейм.
- **🚨 Anti-fuzz защита SH7058 ECU:** прямой SSM2 `ReadBlock` (0xA0) на адресах ROM
  возвращает **stub `0xFF FF FF …`**, а не реальные байты прошивки. Также после первого
  успешного RX чтения второй и далее ReadBlock в той же session/channel — игнорируются.
  Подтверждено через сравнение с EcuFlash-дампом: ROM @ 0x0 реально = `00 00 0B 68 FF FF BF A0`
  (boot vector), а ReadBlock отдаёт 128×`0xFF`. Поэтому полный дамп через прямой SSM2 невозможен —
  нужен kernel-upload (см. roadmap Slice 21).

## TODO

### Критический путь для работы с реальным железом
- [x] ~~**Native Mac-транспорт для Tactrix OP 2.0**~~ — `TactrixTransport` через rusb, Slice 19
- [ ] **Slice 21: Kernel-upload-сценарий** — GPLv3 крейт `romraider-kernel`, через npkern/nisprog
  reference. Sequence: KWP2000 fast-init → SID 0x27 seed/key → SID 0x34/0x36 (upload binary
  в `0xFFFF3000`) → SID 0x31 startRoutine → kernel-side `SID_DUMP` для streaming ROM @ ~5.4 KB/s
- [ ] **J2534 — реализовать `Device::open/connect/write_msgs/read_msgs`** — нужно для современных Subaru (CAN/ISO15765)
- [ ] **J2534 — `LibraryLocator` под Windows** (чтение реестра `HKLM\Software\PassThruSupport.04.04`)
- [ ] **J2534 — `LibraryLocator` под Linux/Mac** (поиск в `/usr/lib/passthru/` или подобном)
- [ ] **5-baud init handshake** — некоторые старые ECU требуют, прежде чем SSM-сессия начнётся. Сейчас полагаемся на ELM327, делающий это сам

### UX-улучшения
- [ ] Hot-plug detection последовательных портов (`SerialPortRefresher`-аналог)
- [ ] Auto-detect ECU protocol по ответу на init-запрос
- [ ] Permission helper под Linux (`dialout` group warning)

### Не критично
- [ ] Mock-engine который умеет «отвечать как ECU» — для интеграционных тестов протоколов без железа
