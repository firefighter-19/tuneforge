# Архитектура tuneforge

Документ описывает **текущее состояние** проекта — структуру воркспейса,
зависимости между крейтами, ключевые архитектурные решения. Для
slice-by-slice progress смотри [`../PROGRESS.md`](../PROGRESS.md).

> **Статус (2026-05-15):** core + IO + protocols + ROM + defs + logger
> + kernel-upload + GUI (editor + logger + ECU-tools) — все слои живые
> и протестированы на железе. Закрыты слайсы 1-26. Полный list — в
> `PROGRESS.md`.

## Слои и зависимости

```text
┌───────────────────────────────────────────────────────────────┐
│ tuneforge-gui  (eframe/egui)                                  │
│   editor panel · logger panel · ecu-tools panel (Slice 26)    │
│   File / Edit / ECU menubar                                   │
└───────────────────────────────────────────────────────────────┘
        │            │             │
        ▼            ▼             ▼ (feature "ecu-tools")
┌──────────────┐  ┌──────────────┐  ┌──────────────────────────────┐
│ tuneforge-   │  │ tuneforge-   │  │ tuneforge-kernel             │
│ rom          │  │ logger       │  │ (GPL-3.0, opt-in)            │
│ tables 1D/2D │  │ session/poll │  │ K-Line + CAN kernel-upload   │
│ 3D, scaling, │  │ datalog      │  │ seed/key (Feistel + RE'd     │
│ checksum     │  │ XY-plot feed │  │ round-keys), orchestrator    │
└──────────────┘  └──────────────┘  └──────────────────────────────┘
        │            │             │
        └────────────┼─────────────┤
                     ▼             ▼
┌──────────────┐  ┌──────────────────────────────────────────────┐
│ tuneforge-   │  │ tuneforge-protocol                           │
│ defs         │  │ ssm (K-Line) · obd2 (Mode 01/09) ·           │
│ ECU/logger   │  │ uds (Mode 0x23) · subaru (SSM3-CAN A8/AA) ·  │
│ XML, scaling │  │ ds2 · ncs                                    │
└──────────────┘  └──────────────────────────────────────────────┘
                              │
                              ▼
                  ┌──────────────────────────────┐
                  │ tuneforge-io                 │
                  │ Transport trait              │
                  │ serial · ELM327 · J2534 ·    │
                  │ tactrix (rusb K-Line + CAN)  │
                  │ mock (for tests)             │
                  └──────────────────────────────┘
                              │
                              ▼
                  ┌──────────────────────────────┐
                  │ tuneforge-core               │
                  │ Address · Endian · bytes ·   │
                  │ errors · format helpers      │
                  └──────────────────────────────┘
```

**Правило:** зависимости **строго сверху вниз**. `core` ни от кого не
зависит, GUI зависит от всего нужного, логгер не знает про GUI,
протоколы не знают про ROM/логгер. CLI (`tuneforge-cli`) — параллельный
бинарный крейт, делит deps с GUI кроме `egui`.

## Workspace layout

Все крейты наследуют единую версию из `[workspace.package].version` в корневом
`Cargo.toml` (см. `version = { workspace = true }` в каждом `crates/*/Cargo.toml`).
Версионирование — SemVer, минорные бампы привязаны к slice-кластерам.

| Crate                | Лицензия       | Описание                                                                      |
| -------------------- | -------------- | ----------------------------------------------------------------------------- |
| `tuneforge-core`     | GPL-2.0+       | Общие типы: `Address`, `Endian`, `bytes::hex_dump`, `RomError`                |
| `tuneforge-io`       | GPL-2.0+       | `Transport`-trait + impls: serial (serialport), J2534, ELM327, **Tactrix** (rusb K-Line + CAN), MockTransport |
| `tuneforge-protocol` | GPL-2.0+       | SSM2 K-Line, OBD-II Mode 01/09 (CAN), UDS ISO-14229, Subaru SSM3-CAN, DS2, NCS |
| `tuneforge-defs`     | GPL-2.0+       | Парсер `ecu_defs.xml` + `log_defs.xml`, scaling-формулы (meval), include-резолв |
| `tuneforge-rom`      | GPL-2.0+       | `RomImage`, 1D/2D/3D таблицы, checksum (Subaru STD/ALT/4-byte)                 |
| `tuneforge-logger`   | GPL-2.0+       | `LoggerSession::poll_once`/`run`, CSV-datalog, broadcast-канал                |
| `tuneforge-kernel`   | **GPL-3.0+**   | Kernel-upload (K-Line + CAN), seed/key Feistel + RE'd round-keys, orchestrator (Slice 23). **Opt-in за feature flag-ом** — workspace остаётся под GPL-2.0+ если не подключён. |
| `tuneforge-cli`      | GPL-2.0+       | Headless CLI: `ssm-init`, `dump-rom[-can]`, `logger[-can][-ssm-can]`, `inspect-*`, `peek-*`, `dtc-can` |
| `tuneforge-gui`      | GPL-2.0+       | egui-приложение: Editor (ROM редактор) + Logger (XY-plot) + ECU-tools (опц.) |

## Маппинг Java → Rust

(Java-пакеты из `../RomRaider/src/main/java/com/romraider/*`.)

| Java-пакет                                | Rust-крейт                                                   |
| ----------------------------------------- | ------------------------------------------------------------ |
| `io.connection`, `io.serial`              | `tuneforge-io::serial`                                       |
| `io.elm327`                               | `tuneforge-io::elm327`                                       |
| `io.j2534.*`                              | `tuneforge-io::j2534`                                        |
| **(нет аналога)**                         | `tuneforge-io::tactrix` (Mac-native через rusb, без J2534)   |
| `io.protocol.ssm.iso9141`                 | `tuneforge-protocol::ssm` (K-Line)                           |
| `io.protocol.ssm.iso15765`                | `tuneforge-protocol::subaru` (SSM3-CAN, cmd 0xAA/0xA8/0xA0)  |
| `io.protocol.obd`                         | `tuneforge-protocol::obd2` (Mode 01/09, PID-таблица)         |
| **(нет аналога)**                         | `tuneforge-protocol::uds` (ISO-14229 Mode 0x23)              |
| `io.protocol.ds2` / `ncs`                 | `tuneforge-protocol::ds2` / `ncs` (заглушки, неактивны)      |
| `xml.*` + `definitions/*.xml`             | `tuneforge-defs` (XML **не модифицируются**, парсятся as-is) |
| `maps.*`                                  | `tuneforge-rom::table`                                       |
| `maps.checksum.*`                         | `tuneforge-rom::checksum`                                    |
| `logger.ecu.*`                            | `tuneforge-logger::session` (+ async-broadcast)              |
| `logger.external.*`                       | TBD (внешние датчики AEM/Innovate)                           |
| `editor.*` + `swing.*`                    | `tuneforge-gui::panels::editor` (egui, полностью переписан)  |
| **(нет аналога)**                         | `tuneforge-gui::panels::logger` (XY-plot live через mpsc)    |
| **(нет аналога)**                         | `tuneforge-gui::panels::ecu_tools` (Slice 26: Read ROM modal) |
| `ramtune.*`                               | NOT planned (flash explicit non-goal — нет donor-ECU)        |
| **(нет аналога)**                         | `tuneforge-kernel` (K-Line npkern + CAN kernel-upload-flow)  |

## Ключевые архитектурные решения

### 1. **`tuneforge-kernel` изолирован под GPL-3.0**

Crate использует наработки от GPL-3 проектов
([fenugrec/nisprog](https://github.com/fenugrec/nisprog),
[fenugrec/npkern](https://github.com/fenugrec/npkern)). Чтобы не
«заразить» workspace под GPL-3, crate подключается **только через
feature-flag-и** (`kernel-upload` в CLI, `ecu-tools` в GUI). По
умолчанию остальные крейты — под **GPL-2.0+** (apстримная лицензия
Java RomRaider).

### 2. **Два транспорта — K-Line и CAN — через единый `Transport` trait**

`tuneforge-io::transport::Transport` имеет `write_all`/`read_frame`/
`purge`/`set_baud`. Все реализации (Serial, Tactrix, Mock) interchangeable.
Tactrix-impl настраивается через `TactrixConfig::ssm()` (K-Line ISO9141)
или `iso15765_500k()` (CAN 500kbps + flow-control filter). Высокоуровневые
flow-ы (kernel upload, logger poll) написаны generic-ом над transport-ом
→ unit-тесты на `MockTransport`, integration-тесты на захваченных дампах.

### 3. **XML — source of truth, не модифицируется**

`ecu_defs.xml` (334+ ROM-ов) и `log_defs.xml` (156 SSM-параметров) —
self-contained артефакты сообщества с 2009 года. Наш парсер консумит
их as-is, никакой регенерации/нормализации. Если в апстриме что-то
обновляется — мы просто берём новую версию файла. Это **ключевая
причина почему проект жизнеспособен** — не нужно мигрировать БД таблиц.

### 4. **Защита от случайной потери данных в редакторе**

Editor-panel хранит **modified-since-open** mask, отрисовывает
изменённые ячейки жёлтым. `File → Save ROM As…` обязательный — нет
silent overwrite. Undo/Redo с history=100. Checksum auto-fix перед
save (Slice 10).

### 5. **Worker thread + mpsc для всего что блокирует**

Логгер polling — worker в отдельном thread, samples через
`std::sync::mpsc::channel`, UI drain-ит каждый frame.
ECU-tools (Read ROM) — то же самое: worker делает 45-сек dump,
шлёт `DumpProgress` events, UI рисует progress-bar. **GUI thread
никогда не блокируется** на USB/serial.

### 6. **Read-only стратегия проекта**

Flash-write **не реализован и не будет** без donor-ECU. Kernel-upload
пишет только в RAM (`0xFFFF3000`), не в flash. В GUI placeholder-пункты
`Write ROM` / `Erase ROM` присутствуют в меню как disabled с tooltip-ом.
Это **намеренный design choice** — авторы предпочитают «работающий
read-only тул» «брикнутому ECU».

### 7. **Mac-native первичный таргет**

Tactrix Openport 2.0 через rusb (libusb), без J2534 DLL и без Wine.
Это **главное отличие** от Java-RomRaider, который на Mac/Linux требует
J2534 эмуляции через DLL-wrappers. Trade-off — `sudo` для USB-bulk
доступа на macOS (kernel security model).

## Open questions

- **GUI sudo на macOS:** сейчас `sudo cargo run -p tuneforge-gui --features ecu-tools` —
  ugly UX, но простое и работает. Долгосрочная альтернатива — отдельный
  helper-binary с правами + IPC. Решить когда будет реальный distribution
  (bundle/app-image).
- **Анти-fuzz ECU 2007+:** некоторые SSM2 read-команды и UDS-сервисы
  блокируются Subaru-защитой в default session, разблокируются только
  через SecurityAccess+ProgrammingSession (что halts engine). Live-логгинг
  ecuparams **невозможен** на этих машинах — это архитектурное
  ограничение Subaru, не баг нашего кода (см. Slice 24c в PROGRESS.md).
- **KWP2000 K-Line:** для JDM-машин 2003-2005 которые могут иметь
  KWP2000 вместо SSM2 — нужен отдельный модуль (Slice 27 опц.). Ждёт
  hardware-pretexta.
- **Плагины внешних датчиков** (AEM/Innovate): WASM-runtime vs нативные
  trait-objects — решить после стабилизации SSM2-логгера.

## Что точно **не** меняется

- XML-определения ECU (`../RomRaider/definitions`, `../RomRaider/customize`)
- DTD-схемы (`*.dtd`) — контракт с коммьюнити
- Алгоритмы checksum — копируются 1:1 из `com.romraider.maps.checksum.*`
- Формат datalog-файлов
- i18n-ресурсы — формат и ключи (когда дойдём до GUI-локализации)
