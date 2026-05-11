# romraider-logger — Progress

## Цель

Backend логгера: цикл «запрос ECU → ответ → семпл → подписчики»; запись
datalog-файлов; подписка GUI/CLI на поток данных; интеграция с внешними
датчиками (AEM/Innovate/PLX/Phidget). Не содержит UI — это просто движок.

## Эталон (Java RomRaider) — `com.romraider.logger.*` (338 файлов, 37 048 строк)

**Это самый большой пакет апстрима.** В Java логгер включает в себя и UI,
и движок, и плагины. Мы держим UI в `romraider-gui`, а здесь — только
backend + плагинные адаптеры.

### `logger/ecu/EcuLogger.java` (~2 700 строк)
Главный класс логгера-приложения в Java (фактически контроллер). Координирует
все слои: чтение определений, опрос ECU, отрисовку, файлы. У нас разделено.

### `logger/ecu/comms/*` — коммуникация с ECU
- `controller/` — управляющий цикл (`LoggerController`, `LoggerControllerImpl`)
- `io/` — `LoggerIo`, `LoggerIoImpl` — оборачивает Transport
- `query/` — `Query`, `QueryManager`, `QueryRange` — пачкование запросов
- `manager/` — `QueryManagerImpl` — диспетчер запросов
- `learning/` — `LearningTableValues` — обучаемые таблицы (A/F learn, etc.)
- `globaladjust/` — глобальные настройки (boost target, fuel pressure)
- `readcodes/` — чтение DTC кодов
- `reset/` — reset ECU memory

### `logger/ecu/definition/*`
Парсер `log_defs.xml` (наш `romraider-defs::logger`).

### `logger/ecu/profile/*`
Пользовательский профиль логгера (какие параметры выбраны, gauge layout).

### `logger/ecu/ui/*` — Swing UI логгера
Будет реализовано в `romraider-gui` (или вообще отложено — GUI-логгер ещё не начат).
Включает: data registration broker, status indicator, gauges, plots, parameter tabs.

### `logger/external/*` — плагины внешних датчиков (13 плагинов)
- `core/` (16 файлов) — общий каркас плагинов
- `aem/` (9), `aem2/` (4) — AEM wideband O2
- `apsx/` (3) — APSX wideband
- `ecotrons/` (10) — Ecotrons ALM
- `fourteenpoint7/` (8) — 14Point7 SLC
- `innovate/` (16) — Innovate LC-1/2, MTX-L
- `mrf/` (5) — MRF Tuning
- `phidget/` (10) — Phidget Interface Kit (универсальный)
- `plx/` (13) — PLX wideband
- `te/` (9) — TechEdge
- `txs/` (6) — TXS Tuner/UTec
- `zt2/` (9) — Zeitronix ZT-2

### `logger/car/util/` — car-specific вычисления
- Калькуляторы dyno-эффективности (нагрузка, мощность) — на основе входных параметров

## Статус

### Каркас и публичный API

| Фича                                          | Java-аналог                       | Rust                             | Статус |
| --------------------------------------------- | --------------------------------- | -------------------------------- | :----: |
| `LoggerSession` — главный цикл                | `LoggerController`                | `session::LoggerSession` (заглушка) | 🟡 |
| `SessionConfig` (poll interval, timeout)      | `LoggerSettings`                  | `session::SessionConfig`         |   ✅   |
| Broadcast канал семплов подписчикам           | Java listener-pattern             | `tokio::sync::broadcast`         |   ✅   |
| `Sample`/`SampleValue` структуры              | `LoggerData`/`LoggerDataValue`    | `sample::{Sample,SampleValue}`   |   ✅   |
| Структурированные ошибки                      | various `LoggerException`-classes | `LoggerError` enum               |   ✅   |

### Цикл опроса ECU

| Фича                                          | Java-аналог                          | Rust          | Статус |
| --------------------------------------------- | ------------------------------------ | ------------- | :----: |
| Сборка multi-address SSM-запроса из подписок  | `Query.addAddress` + `QueryManager`  | —             |   ❌   |
| Отправка + парс через `Transport`             | `LoggerIoImpl.send`                  | —             |   ❌   |
| Применение scaling к каждому полю             | `EcuParameterImpl.getResult`         | — (есть в defs::CompiledScaling, но не вшито в цикл) | 🟡 |
| Полудуплексный handshake                      | `LoggerControllerImpl.refresh`       | —             |   ❌   |
| Auto-detect частоты (max poll rate)           | inline в `LoggerController`          | —             |   ❌   |
| Retry on read-timeout                         | inline                               | —             |   ❌   |

### Datalog-файлы

| Фича                                          | Java-аналог                  | Rust                       | Статус |
| --------------------------------------------- | ---------------------------- | -------------------------- | :----: |
| CSV-writer                                    | `FileLoggerImpl`             | `datalog::DatalogWriter`   |   ✅   |
| Header с именами параметров                   | `FileLogger.writeHeader`     | `DatalogWriter::write_header` |  ✅   |
| Timestamp в ms                                | `FileLogger.writeData`       | `DatalogWriter::write_sample` |  ✅   |
| Подключение к `LoggerSession::run` цикл       | `FileLoggerListener`         | —                          |   ❌   |
| Авто-флаш / ротация по размеру/времени        | inline                       | —                          |   ❌   |
| Datalog playback (читать .csv обратно)        | `PlaybackManager`            | —                          |   ❌   |
| RomRaider-совместимый формат                  | RomRaider columns/headers    | —                          |  🟡   |

### Внешние датчики

| Фича                                          | Java-аналог                                          | Rust                       | Статус |
| --------------------------------------------- | ---------------------------------------------------- | -------------------------- | :----: |
| `ExternalSensor` trait                        | `ExternalDataSource`/`ExternalDataItem`              | `external::ExternalSensor` (заглушка) | 🟡 |
| AEM wideband O2                               | `logger/external/aem/*`                              | —                          |   ❌   |
| AEM 2 (wifi)                                  | `logger/external/aem2/*`                             | —                          |   ❌   |
| Innovate LC-1/2/MTX-L                         | `logger/external/innovate/*`                         | —                          |   ❌   |
| PLX wideband + multi-sensor                   | `logger/external/plx/*`                              | —                          |   ❌   |
| Ecotrons ALM                                  | `logger/external/ecotrons/*`                         | —                          |   ❌   |
| 14Point7 SLC                                  | `logger/external/fourteenpoint7/*`                   | —                          |   ❌   |
| APSX wideband                                 | `logger/external/apsx/*`                             | —                          |   ❌   |
| TechEdge                                      | `logger/external/te/*`                               | —                          |   ❌   |
| MRF Tuning                                    | `logger/external/mrf/*`                              | —                          |   ❌   |
| TXS Tuner/UTec                                | `logger/external/txs/*`                              | —                          |   ❌   |
| Zeitronix ZT-2                                | `logger/external/zt2/*`                              | —                          |   ❌   |
| Phidget универсальный                         | `logger/external/phidget/*`                          | —                          |   ❌   |
| Plugin-loader (динамическая загрузка)         | `ExternalDataSourceLoader` через `META-INF/services` | — (TODO: WASM или trait-objects) | ❌ |

### Учёт обучаемых таблиц (Learning)

| Фича                                          | Java-аналог                       | Rust | Статус |
| --------------------------------------------- | --------------------------------- | ---- | :----: |
| Reading A/F learning #1/#2/#3                 | `LearningTableValuesImpl`         | —    |   ❌   |
| Reading idle/closed-loop learning             | `LearningTableValuesImpl`         | —    |   ❌   |
| Reset learning tables                         | `comms/reset/`                    | —    |   ❌   |

### DTC коды

| Фича                                | Java-аналог                       | Rust | Статус |
| ----------------------------------- | --------------------------------- | ---- | :----: |
| Read DTC codes                      | `comms/readcodes/`                | —    |   ❌   |
| Clear codes                         | `comms/reset/`                    | —    |   ❌   |

### Car-specific (dyno-расчёты)

| Фича                              | Java-аналог            | Rust | Статус |
| --------------------------------- | ---------------------- | ---- | :----: |
| Wheel torque calculation          | `car/util/Calculator*` | —    |   ❌   |
| Power estimation                  | `car/util/*`           | —    |   ❌   |

## TODO

### Критически (чтобы логгер вообще что-то делал)
- [ ] **Реализовать `LoggerSession::run`** — сборка SSM-запроса из активных подписок, отправка через Transport, парс ответа в `Sample`, broadcast
- [ ] **Подписочный API** (`Subscriber`, `subscribe(parameter_id)`) — для GUI/CLI/datalog
- [ ] **Hook datalog'а к broadcast-каналу** — на каждом семпле писать в файл

### Высокий приоритет
- [ ] **Connect к `defs::LoggerDocument`** — выбор параметров из определений, резолв `include`-шаблонов
- [ ] **Compile-on-demand для `[value]`-формул** — пересборка scaling для логгер-параметров (другой синтаксис чем `x`)
- [ ] **`romraider-cli logger` команда** — headless логгирование в CSV без GUI

### Средний приоритет
- [ ] Один реальный внешний датчик (AEM или Innovate — самые популярные wideband-и)
- [ ] Read DTC codes
- [ ] Playback datalog-файлов

### Низкий приоритет
- [ ] Плагинная архитектура: WASM vs trait-objects (см. ADR в `docs/ARCHITECTURE.md`)
- [ ] Остальные 11 внешних датчиков
- [ ] Learning tables read/reset
- [ ] Car-specific dyno calculations
