# romraider-protocol — Progress

## Цель

Диалекты диагностических протоколов ECU. Каждый протокол собирает запросы
как `Vec<u8>` и парсит ответы, опираясь на `Transport`-трейт из
`romraider-io` для байтового канала.

## Эталон (Java RomRaider) — `com.romraider.io.protocol.*`

В Java протоколы живут под `io/`, у нас вынесены в отдельный крейт для
чистого разделения «канал ↔ диалект».

### Общий каркас
- `Protocol.java` — главный интерфейс протокола (build request, validate response)
- `ProtocolFactory.java` — диспетчер «Subaru SSM2 → SSMProtocol, BMW → DS2Protocol …»
- `ProtocolDS2.java`, `ProtocolNCS.java` — высокоуровневые ports

### SSM (Subaru Select Monitor) — `protocol/ssm/`
**iso9141 (Subaru SSM2 over K-Line @ 4800 baud)** — 4 файла:
- `SSMProtocol.java` — `0x80 dst src len cmd [payload] checksum`
- `SSMChecksumCalculator.java` — `sum mod 256`
- `SSMResponseProcessor.java` — парсер ответа
- `SSMLoggerProtocol.java` — wrapper для logger-сессий

**iso15765 (SSM3 over CAN)** — 3 файла:
- `SSMProtocol.java` — CAN-frame версия с теми же командами
- `SSMResponseProcessor.java`
- `SSMLoggerProtocol.java`

### OBD-II — `protocol/obd/iso15765/` (3 файла)
- `OBDProtocol.java` — Mode 01/03/04/07/09
- `OBDLoggerProtocol.java`
- `OBDResponseProcessor.java`

### DS2 (BMW) — `protocol/ds2/iso9141/` (4 файла)
- `DS2Protocol.java`, `DS2LoggerProtocol.java`, `DS2ResponseProcessor.java`
- `DS2ChecksumCalculator.java` — XOR-checksum

### NCS (Nissan Consult) — `protocol/ncs/`
**iso14230 (K-Line)** — 4 файла:
- `NCSProtocol.java`, `NCSLoggerProtocol.java`, `NCSResponseProcessor.java`
- `NCSChecksumCalculator.java`

**iso15765 (CAN)** — 3 файла:
- `NCSProtocol.java`, `NCSLoggerProtocol.java`, `NCSResponseProcessor.java`

## Статус

### SSM (Subaru) — самый зрелый

| Фича                                          | Java-аналог                       | Rust                          | Статус |
| --------------------------------------------- | --------------------------------- | ----------------------------- | :----: |
| Кадр: `80 dst src len cmd … cksm`             | `SSMProtocol.constructWriteMemoryRequest` | `ssm::build_request`     |   ✅   |
| Checksum (sum mod 256)                        | `SSMChecksumCalculator`           | `ssm::checksum`               |   ✅   |
| ECU-init request (`0xBF`)                     | `SSMEcuInit`                      | `ssm::ecu_init`               |   ✅   |
| ECU-init response decode (SSM ID + ROM ID + caps) | `SSMEcuInitImpl`               | `ssm::decode_ecu_init`        |   ✅   |
| Read by address (`0xA8`, до 255 байт за раз)  | `SSMProtocol.constructReadMemoryRequest` | `ssm::read_addresses`     |   ✅   |
| Read by block (`0xA0`) — 1..=254 байт          | `SSMProtocol.constructReadMemoryRequest` | `ssm::read_block` + `READ_BLOCK_MAX` константа | ✅ |
| Полный дамп ROM с прогресс-callback           | сборка вручную в Java RomRaider  | `ssm::dump_rom(transport, start, length, chunk_size, progress_cb)` с P3-guard `sleep(50ms)` между chunk-ами | ✅ |
| ⚠️ **Anti-fuzz защита ECU на ReadBlock**       | (не доступно из Java апстрима)   | подтверждено на 2007 Forester XT 2026-05-12: ECU отдаёт `0xFF`-stub вместо реального ROM. Для полного dump нужен kernel-upload (см. roadmap) | 🚨 |
| Write by address (`0xB8`)                     | `SSMProtocol.constructWriteAddressRequest` | —                       |   ❌   |
| Write by block (`0xB0`)                       | `SSMProtocol.constructWriteMemoryRequest` | —                        |   ❌   |
| Response validation (length+checksum)         | `SSMResponseProcessor`            | `ssm::parse_response`         |   ✅   |
| Frame re-assembly из chunked serial-read      | inline                            | `ssm::read_complete_frame`    |   ✅   |
| Negative response (NRC) handling              | `SSMResponseProcessor`            | — (есть variant в ProtocolError, но не используется) | 🟡 |
| SSM3 (CAN/ISO15765)                           | `protocol/ssm/iso15765/*`         | —                             |   ❌   |
| MockTransport-тесты на канонических кадрах    | —                                 | 13 unit-тестов в `ssm.rs`     |   ✅   |

### OBD-II (ISO15765 over CAN)

| Фича                              | Java                          | Rust            | Статус |
| --------------------------------- | ----------------------------- | --------------- | :----: |
| Mode 01 build request             | `OBDProtocol.constructPidReq` | `obd2::build_mode_01` | 🟡 заглушка |
| Mode 01 parse response            | `OBDResponseProcessor`        | `obd2::parse_mode_01` | 🟡 заглушка |
| Mode 03 (DTC read)                | `OBDProtocol.constructDtcRequest` | —           |   ❌   |
| Mode 04 (clear DTC)               | `OBDProtocol`                 | —               |   ❌   |
| Mode 07/09 (mode-7 DTC, VIN)      | `OBDProtocol`                 | —               |   ❌   |
| ISO-TP framing (Flow Control)     | в `J2534ConnectionISO15765`   | —               |   ❌   |

### DS2 (BMW)

| Фича                              | Java                                  | Rust       | Статус |
| --------------------------------- | ------------------------------------- | ---------- | :----: |
| Кадр: `dst len data… xor-checksum`| `DS2Protocol`                         | —          |   ❌   |
| XOR-checksum                      | `DS2ChecksumCalculator`               | —          |   ❌   |
| Read ID/memory/status             | `DS2Protocol.constructRead*Request`   | —          |   ❌   |
| `ds2.rs` — только TODO-комментарии | —                                    | `ds2.rs`   |   ❌   |

### NCS (Nissan Consult)

| Фича                              | Java                       | Rust       | Статус |
| --------------------------------- | -------------------------- | ---------- | :----: |
| 14400 baud init `FF FF EF`        | `NCSProtocol.constructInit`| —          |   ❌   |
| ISO-14230 K-Line кадрирование     | `NCSProtocol`              | —          |   ❌   |
| ISO-15765 CAN-варианты            | `ncs/iso15765/*`           | —          |   ❌   |
| `ncs.rs` — только TODO-комментарии | —                         | `ncs.rs`   |   ❌   |

### Общее

| Фича                              | Java                       | Rust                    | Статус |
| --------------------------------- | -------------------------- | ----------------------- | :----: |
| Структурированные ошибки          | `ProtocolException` family | `ProtocolError` enum    |   ✅   |
| Generic Protocol-trait            | `Protocol.java` interface  | — (по факту каждый протокол — свой модуль) | ⏸ |
| Protocol-factory по типу ECU      | `ProtocolFactory`          | —                       |   ⏸   |

## TODO

### Критически для дампа прошивки
- [x] ~~**SSM `Command::ReadBlock` (0xA0)**~~ — реализовано в Slice 18, но
      **не работает для дампа на 2007 Subaru ECU**: anti-fuzz отдаёт stub-`0xFF`.
- [ ] **Slice 21: Kernel-upload SH7058 path** (отдельный GPLv3-крейт `romraider-kernel`):
  - Реализовать KWP2000-фрейминг поверх K-line transport (он близок к SSM2, но
    с другой структурой `dst src len data... chksm` и другими SID-ами)
  - SID 0x27 securityAccess — 16-round Feistel seed/key transform (взять из nisprog/ssm_backend.c)
  - SID 0x10 startDiagnosticSession + SID 0x34/0x36 (uploadRequest + transferData) для заливки kernel
  - SID 0x31 startRoutine — handover на kernel в RAM @ `0xFFFF3000`
  - Kernel-side wire protocol (`SID_DUMP` блоками по 32 байта при повышенном baud) — отдельный модуль
  - Verify byte-by-byte vs `fixtures/forester-xt-2007-4E42504007.bin`
- [ ] **SSM write address/block** — отдельный слайс, опасный (запись в живой ECU)

### Critically для расширения покрытия
- [ ] **OBD-II Mode 01 полный** — для OBD-II логгера без Subaru-специфики
- [ ] **ISO-TP framing** — если хотим OBD-II/CAN через ELM327 без J2534

### Когда дойдут руки
- [ ] **DS2 (BMW)** — отдельный слайс ~300–500 строк
- [ ] **NCS (Nissan)** — отдельный слайс, формат K-Line и CAN-варианты
- [ ] **SSM3 (CAN)** — портирование `iso15765/SSMProtocol.java`
- [ ] **NRC обработка** — внятные ошибки для негативных ответов ECU
- [ ] **Universal `Protocol`-trait** — для diagnositic factory pattern
