# romraider-cli — Progress

## Цель

Headless CLI для отладки I/O, проверки XML-определений и просмотра/чтения
ROM-таблиц без GUI. Удобен как:
- **смоук-тест** интеграции (parse XML → resolve → read ROM → scale)
- **подспорье для CI** (валидация определений автоматически)
- **первая встреча с реальным железом** (`ssm-init` показывает живой ответ ECU)

## Эталон (Java RomRaider)

**Прямого CLI-аналога нет.** Java-RomRaider — это GUI-приложение
(`ECUExec.java` и `EcuLoggerExec.java` запускают Swing-окна). Все
инструменты доступны только через UI.

Однако в апстриме есть отдельные test-executable'ы:
- `com.romraider.io.j2534.api.TestJ2534*` — проверка J2534-драйвера
- `com.romraider.ramtune.test.RamTuneTestApp` — minimal Swing-UI для тестов RamTune
- `com.romraider.io.serial.connection.TestSerialConnection` — открыть serial и поговорить

Эти test-executable'ы наш CLI заменяет в более общем виде.

## Статус

| Команда                                       | Назначение                                | Реализация                            | Статус |
| --------------------------------------------- | ----------------------------------------- | ------------------------------------- | :----: |
| `ports`                                       | список доступных COM-портов               | `serialport::available_ports`         |   ✅   |
| `ssm-init --port <p> [--baud N] [--timeout-ms N]` | SSM ECU-Init → SSM ID + ROM ID + caps | `protocol::ssm::ecu_init` через `SerialTransport` | ✅ |
| `inspect-rom <file>`                          | размер + первые 16 байт ROM-файла         | `RomImage::open`                      |   ✅   |
| `inspect-def <file>`                          | сводка по XML: convert-factors + ROMs     | `defs::parse_file`                    |   ✅   |
| `inspect-def --resolve [--rom <xmlid>]`       | + материализованные таблицы               | `defs::resolve`                       |   ✅   |
| `inspect-def --resolve --sample-byte <N>`     | + пример конверсии byte→real              | `defs::CompiledScaling::to_real`      |   ✅   |
| `inspect-log <file> [--ecu <id>]`             | сводка по `log_defs.xml`                  | `defs::parse_log_file`                |   ✅   |
| `read-table <rom> --def <def.xml> --rom-id <id> --table <name>` | прочитать таблицу из ROM | `RomImage::read_table` + scaling | ✅ |

## Не реализовано

| Команда (могла бы быть)                       | Аргументы                                   | Зачем                                    | Статус |
| --------------------------------------------- | ------------------------------------------- | ---------------------------------------- | :----: |
| `dump-rom`                                    | `--port <p> --start 0x000000 --length N --output <bin> [--chunk-size 128]` | дамп прошивки с ECU по SSM `ReadBlock` с прогрессом каждые 5% | ✅ |
| `write-table`                                 | `--rom <bin> --def <xml> --rom-id <id> --table <name> --values <csv>` | bulk-редактирование таблицы без GUI | ❌ |
| `verify-checksum` / `fix-checksum`            | `--rom <bin> --def <xml>`                   | проверка/пересчёт checksum-ов            |   ❌   |
| `compare-roms`                                | `--a <bin1> --b <bin2> --def <xml>`         | diff двух ROM-файлов по таблицам         |   ❌   |
| `convert-def`                                 | `--from xdf --to xml <input>`               | импорт XDF/VDF/BMW в наш формат          |   ❌   |
| `logger`                                      | `--port <p> --def <log_defs.xml> --ecu <id> --params <id1,id2> --out <csv> [--duration-secs N --interval-ms M]` | headless логгер в CSV через SSM | ✅ |
| `read-dtc`                                    | `--port <p>`                                | чтение DTC-кодов                         |   ❌   |
| `flash-rom`                                   | `--port <p> --rom <bin>`                    | reflash (через J2534, опасная операция!) |   ❌   |
| `print-tree`                                  | `--def <xml> --rom-id <id> [--format json]` | дерево таблиц в JSON для других скриптов |   ❌   |

## Особенности и conventions

| Особенность                                   | Статус                                      |
| --------------------------------------------- | :-----------------------------------------: |
| `--help` для каждой команды (clap-derive)     | ✅                                          |
| Структурированные ошибки через `anyhow`       | ✅                                          |
| Tracing через `RUST_LOG` env                  | ✅                                          |
| Выход с ненулевым кодом при ошибке            | ✅ (`Result<()>` из `main`)                 |
| Цветной вывод                                 | ❌ (можно добавить `owo-colors`)            |
| JSON-режим для machine-readable вывода        | ❌                                          |
| Прогресс-индикатор для длинных операций       | ❌ (нужен будет для `dump-rom`)             |

## TODO

### Сразу полезно
- [ ] **`dump-rom`** — нужно прямо вместе с SSM `ReadBlock` в `romraider-protocol`
- [ ] **`verify-checksum` / `fix-checksum`** — пользователь должен иметь возможность проверить ROM до записи, не открывая GUI
- [ ] **`write-table`** — для скриптовых правок

### Удобно
- [ ] **`compare-roms`** — diff в табличном виде, очень полезно при reverse-engineering
- [ ] **`logger` headless** — для записи long-run-датлогов на удалённом устройстве (Raspberry Pi с CAN)
- [ ] **`print-tree --format json`** — для интеграции с другими инструментами

### Опасно но важно
- [ ] **`flash-rom`** — последний по списку; требует серьёзного тестирования (запорешь ECU — поедешь на эвакуаторе)
