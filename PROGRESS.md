# romraider-rs — общий статус проекта

Сводная таблица по всем крейтам. Детали каждого — в соответствующем
`crates/<name>/PROGRESS.md`.

## Сводка по крейтам

| Крейт                   | Назначение                                     | Готовность | PROGRESS                                              |
| ----------------------- | ---------------------------------------------- | :--------: | ----------------------------------------------------- |
| **romraider-core**      | Address, Endian, bytes-utils, errors           | 🟢 75%     | [`crates/romraider-core/PROGRESS.md`](crates/romraider-core/PROGRESS.md)         |
| **romraider-io**        | Transport-trait + serial/elm327/j2534/tactrix  | 🟢 65%     | [`crates/romraider-io/PROGRESS.md`](crates/romraider-io/PROGRESS.md)             |
| **romraider-protocol**  | SSM/OBD/DS2/NCS диалекты                       | 🟡 45%     | [`crates/romraider-protocol/PROGRESS.md`](crates/romraider-protocol/PROGRESS.md) |
| **romraider-defs**      | Парсер + резолв ECU/log_defs XML, scaling      | 🟢 88%     | [`crates/romraider-defs/PROGRESS.md`](crates/romraider-defs/PROGRESS.md)         |
| **romraider-rom**       | ROM-image, decode/encode, checksum             | 🟢 75%     | [`crates/romraider-rom/PROGRESS.md`](crates/romraider-rom/PROGRESS.md)           |
| **romraider-logger**    | Backend логгера + plugins                      | 🟡 45%     | [`crates/romraider-logger/PROGRESS.md`](crates/romraider-logger/PROGRESS.md)     |
| **romraider-cli**       | Headless CLI (debug + smoke-tests + logger + dump-rom + tactrix) | 🟢 90% | [`crates/romraider-cli/PROGRESS.md`](crates/romraider-cli/PROGRESS.md) |
| **romraider-gui**       | egui-редактор (diff/heatmap/undo/changes) + логгер | 🟢 75%   | [`crates/romraider-gui/PROGRESS.md`](crates/romraider-gui/PROGRESS.md)           |

**Легенда:** 🟢 ≥60% — 🟡 30–60% — 🔴 <30%

Проценты — субъективная оценка «по сравнению с минимально-полезным
эквивалентом апстрима» для типичных пользовательских сценариев.

## Сводка по слайсам (сделано)

| Слайс | Тема                                              | Коммит        |
| ----- | ------------------------------------------------- | ------------- |
| 1     | SSM ECU-init end-to-end через MockTransport       | `fb70f98`     |
| 2     | Парс `<roms>` ECU-определений                     | `c4e7973`     |
| 3     | Резолв ROM/table/scaling наследования + типизация | `0d5d6ba`     |
| 4     | Компиляция и eval scaling-формул (meval)          | `6f33021`     |
| 5     | Парс `log_defs.xml` (логгер-определения)          | `3fd5e80`     |
| 6     | ROM bytes ↔ resolved table (read_table)           | `4588fb0`     |
| 7     | GUI MVP: open ROM/def, picker, read-only грид     | `9500ec4`     |
| 8     | GUI editing: DragValue + write-back, Save As      | (закоммичено)    |
| 9     | Subaru classic checksum (`checksum fix` tables)   | (закоммичено)    |
| 10    | Auto-fix checksum в GUI Save + status-индикатор   | (закоммичено)    |
| 11    | Compare ROMs в GUI (diff coloring + Values/Diff)  | (закоммичено)    |
| 12    | Heatmap-раскраска ячеек (cool→warm по value)      | (закоммичено)    |
| 13    | Undo/Redo + Edit-меню + Ctrl+Z/Y хоткеи           | (закоммичено)    |
| 14    | Tooltip ячеек (addr/raw/real/Δ/formula)           | (закоммичено)    |
| 15    | Switch + BitwiseSwitch UI (radio + checkboxes)    | (закоммичено)    |
| 16    | Logger backbone: resolve include + [value] + CLI  | (закоммичено)    |
| 17    | GUI live XY-plot: worker thread + mpsc + Plot     | (закоммичено)    |
| 18    | SSM ReadBlock + CLI dump-rom (+MockTransport fix) | (закоммичено)    |
| 19    | TactrixTransport (rusb, ati/ata/ato/atf/att/atc/atz) — live SSM init на 2007 Forester XT | следующий коммит |
| 20    | Live `ssm-init --tactrix` end-to-end (ROM `4E42504007`, SSM_ID `A2 10 11`, 96 cap-байт) | следующий коммит |
| 22    | Defs integration на реальной фикстуре (1 МБ SH7058 ROM): float-endian fix, multi-byte switch parser, GUI modified-highlight + Changes-since-open | следующий коммит |
| 23    | **Mac-native ROM dump через UDS/ISO15765**: CAN seed/key RE (firmware-specific 16-word round-key table из ROM `0x05972C`), kernel-upload (64× SID 0xB6), kernel handshake/read_memory, Tactrix CAN multi-frame strip → dump 1 МБ за 43.7 с, SHA-256 = `3016ce24…` совпадает с EcuFlash-fixture-ом byte-for-byte | следующий коммит |
| 24    | **Live datalogger через OBD-II Mode 01 / CAN**: K-Line SSM2 `ReadAddresses` на 2007 USDM Forester блокируется анти-fuzz (init OK, дальше silent) → pivot на CAN. Новый `obd2`-модуль (40 SAE J1979 PID-ов от Engine Load до Relative Accel Pedal), `discover_supported_pids` через bitmap chain (0x00/0x20/0x40/...), CLI `logger-can --list[--probe] | --params NAMES | --all-supported`. Live проверено на машине: ECU поддержал 42 PID-а, 40 из них в нашей таблице — 4 Hz polling всех сразу, lambda/coolant/RPM/MAF/voltage реалистичны | следующий коммит |

## Запланированные слайсы

| Слайс | Тема                                                                | Статус |
| ----- | ------------------------------------------------------------------- | :----: |
| 24b   | **PID batching для скорости** — реализовано: `obd2::read_pids_batched()` шлёт `01 <pid1>..<pid6>` (max 6 per SAE J1979), парсит multi-frame ISO-TP response by-echo (robust к ordering и pid-dropout). `logger_can_cmd` чанкует подписки по 6, с graceful-fallback на single-PID если ECU отвергает batch (NRC 0x13). Ожидаемый speedup ~5-6× для большой подписки (40 PIDs: 4 Hz → ~22 Hz). 4 unit-теста на синтетических responses (mixed lengths, reorder, NRC, zero-pad). | 🟢 готово |
| 24c   | **Subaru-specific extended params через UDS Mode 0x23 ReadMemoryByAddress** — knock correction, fine-grained AFR, individual cyl data, через firmware-specific RAM-адреса из `<ecuparams>` блока в апстрим `logger.xml` (для нашего ROM `4E42504007`) | 🔵 опц. |
| 25    | **Generic protocol abstraction** — единый `EcuClient` trait, auto-detect протокола (K-Line vs CAN) через ECU-init capability-байты, `--protocol auto\|ssm\|can\|kwp` в CLI. Уберёт разделение `dump-rom` vs `dump-rom-can`, `peek-vin` vs `ssm-init` и т.п. | 🔵 запланирован |
| 26    | **GUI ECU-tools menu** — реализован (Slice 26): `ECU` menu в menubar (Read ROM from ECU…, View ECU Info…, disabled Write/Erase placeholders), модал «Read ROM» с multi-phase progress (Phase A/B/C/E + log), worker thread + mpsc, save-as dialog после dump-а. Изолирован за feature-flag-ом `ecu-tools` (тянет GPL-3 `romraider-kernel`). **Refactor**: orchestrator-функция `dump_rom_via_can()` + `peek_ecu_info()` вынесена в `romraider-kernel::orchestrator` — теперь reusable между CLI и GUI. Решение по `sudo` — простейший вариант: `sudo cargo run -p romraider-gui --features ecu-tools` + clear-prompt в Error-state, documented. Helper-binary с IPC откладывается до distribution-этапа | 🟢 готово |
| 27    | **K-Line quirks для pre-anti-fuzz Subaru** — наблюдение на 2004 JDM Legacy B4 (ROM `3C44106006`, SSM-ID `A1 10 08`, 48 cap-bytes): SSM2 ECU-init (`0xBF`) работает **только с заведённым мотором**, а `0xA0`/`0xA8` чтения работают **только с заглушенным мотором** (engine ON генерирует K-Line collisions с другими ECU на шине). На этой машине ни one shot чтения, ни kernel-upload **не работают пока двигатель запущен** → live-logger через K-Line невозможен. Для других подобных машин (2003-2005 JDM) надо: (a) `dump-rom --tactrix` с engine off — работает, продуцирует ground-truth ROM; (b) DTC reading через SSM2 ReadBlock из known RAM-адресов (~30 строк нового `dtc-ssm`-cmd); (c) опционально KWP2000-on-K-Line module (`0x18 ReadDTCByStatus`, `0x21/0x22/0x23` reads) для машин где SSM2 read блокирован полностью | 🔵 опц. |

Дополнительные мелкие polish-задачи без отдельного слайса:
- Coalescing undo при drag (один шаг на drag, а не на каждое значение)
- Description в cell tooltip (копировать описание таблицы в hover-popup)
- Save plot, axis labels, port-dropdown в GUI логгере

## Что работает прямо сейчас (E2E сценарии)

1. **Открыть, посмотреть, редактировать ROM-файл с XML-определением:**
   ```bash
   cargo run -p romraider-gui
   # File → Open ROM (.bin) → Open Def (.xml) → выбрать ROM-ID + таблицу
   # Кликнуть в ячейку, поправить значение → File → Save ROM As…
   ```
   ⚠️ checksum НЕ пересчитываются — сохранённый ROM ECU не примет, пока
   не реализуем Subaru STD/ALT2 checksum.

2. **Headless-инспекция определений:**
   ```bash
   cargo run -p romraider-cli -- inspect-def def.xml --resolve --rom A2WC522S --sample-byte 800
   ```
   Показывает разрешённые таблицы child-ROM с примером конверсии.

3. **Headless-чтение таблицы из ROM:**
   ```bash
   cargo run -p romraider-cli -- read-table firmware.bin --def def.xml --rom-id A2WC522S --table "Target Boost A"
   ```

4. **SSM ECU-init на реальном железе (Tactrix Openport 2.0 + 2007 Forester XT):**
   ```bash
   cargo run -p romraider-cli -- ssm-init --tactrix
   ```
   ✅ Подтверждено на живой машине 2026-05-11. ROM `4E42504007`, SSM `A2 10 11`,
   96 байт capabilities; round-trip ~600 ms через K-Line @ 4800 baud.

5. **Mac-native ROM dump через CAN/ISO15765 (Tactrix Openport 2.0 + 2007 Forester XT):**
   ```bash
   sudo cargo run -p romraider-cli --features kernel-upload -- \
       dump-rom-can --output /tmp/forester-mac-dump.bin
   ```
   ✅ Подтверждено на живой машине 2026-05-13. 1 МБ дамп за 43.7 с, SHA-256
   `3016ce24…` совпадает byte-for-byte с `fixtures/forester-xt-2007-4E42504007.bin`
   (= EcuFlash ground-truth). Sequence: OBD-II Mode 09 VIN/CVN → `10 03` →
   `27 01/02` (firmware-specific Feistel round-keys из ROM `0x05972C`) →
   `10 02` → `34 04 33` → 64× `B6` (encrypted kernel V1.07) → `37` →
   `31 01 02 02 02` → kernel `01`/`03` 1-byte protocol @ 2 КБ chunks.

6. **Редактирование реальной прошивки в GUI:**
   ```bash
   cargo run -p romraider-gui
   # File → Open ROM → fixtures/forester-xt-2007-4E42504007.bin
   # Open Def → /Applications/RomRaider/definitions/ecu_defs.xml
   # → выбрать ROM A8DK100P → редактировать Target Boost / Wastegate Duty
   ```
   ✅ 807 resolved tables режутся корректно, осей читаются как float-BE (Subaru-fix),
   изменённые ячейки подсвечены жёлтым, клик по `● modified` → окно
   «Changes since open» со сводкой Before/After/Δ по таблицам.

## Critical path к «по-настоящему пригоден для тюнинга»

Минимальный набор, чтобы можно было РЕАЛЬНО открыть прошивку, поправить
карту и залить обратно:

1. ~~**Subaru классический checksum**~~ ✅ Slice 9
2. ~~**Авто-fix Subaru checksum** + индикатор valid/invalid~~ ✅ Slice 10
3. ~~**Compare two ROMs**~~ ✅ Slice 11
4. ~~**Heatmap-раскраска**~~ ✅ Slice 12
5. ~~**Undo/Redo**~~ ✅ Slice 13 (history=100, drag = много мелких шагов; coalescing — потом)
6. ~~**Tooltip для ячеек**~~ ✅ Slice 14
7. ~~**Switch-таблицы UI**~~ ✅ Slice 15 (radio + checkbox; multi-byte bitwise — отложен)
8. **Coalescing undo** при drag (один шаг на drag, а не на каждое промежуточное значение)
9. **Description в cell tooltip** — копировать описание таблицы в hover-popup

С точки зрения editor-критпути для редактирования прошивок Subaru — основное закрыто. Дальше открываются другие домены: **logger backend**, **dump-rom через SSM**, **J2534**.

Для **дампа и реflash** — отдельный долгий путь:

6. ~~**SSM ReadBlock + dump-rom CLI**~~ ✅ Slice 18 (на mock); ⚠️ на реальном Subaru
   `ReadBlock` (0xA0) **возвращает stub-0xFF** из-за анти-fuzz защиты ECU — для
   реального дампа нужен kernel-upload.
7. ~~**Slice 21/23 — kernel-upload для SH7058**~~ ✅ Slice 23:
   - GPLv3 изолированный крейт `romraider-kernel`
   - K-Line path (KWP2000 SID 0x81) **не применим** к 2007 USDM Forester (SID 0x81 timeout,
     `27 01` отказывает): ECU использует CAN/ISO15765, а не KWP2000-on-K-Line
   - **Реальный путь** — UDS-over-CAN @ 500 kbps через Tactrix `ato6` (см. сценарий 5 выше)
   - Round-key table (`0x05972C` в ROM) восстановлена brute-force по 8 (seed,key)-парам из capture
   - Encrypted kernel V1.07 ре-используется from capture (`include_bytes!`)
   - Verify byte-by-byte vs `fixtures/forester-xt-2007-4E42504007.bin` — ✅ SHA-256 совпал
8. **J2534 Open/Connect/ReadMsgs/WriteMsgs** ([io](crates/romraider-io/PROGRESS.md)) — для Linux/Win через CAN
9. **J2534 LibraryLocator** под Win/Linux ([io](crates/romraider-io/PROGRESS.md))
10. **RamTune** — отдельный модуль для flash (опасно, тестировать на ECU-доноре)

Для **полноценного логгера**:

10. ~~**`LoggerSession::poll_once` цикл**~~ ✅ Slice 16 — sync + async broadcast
11. ~~**Резолв `include="ssmbase16"`**~~ ✅ Slice 16
12. ~~**Eval `[value]`-синтаксиса формул**~~ ✅ Slice 16
13. ~~**XY-график подвязан к live samples**~~ ✅ Slice 17 (через std::sync::mpsc — async broadcast пока не нужен)
14. ~~**CLI `logger` команда**~~ ✅ Slice 16

Логгер-критпуть закрыт. Все 5 пунктов реализованы (точечно ещё нужен реальный round-trip на железе и polish-улучшения: пометки осей, save plot, выбор портов из dropdown — но это не блокирует).

## Принципы

- **XML-определения апстрима — не модифицируются.** Они источник истины и
  ценнейший актив сообщества; мы парсим их как есть.
- **Эталон поведения — Java-RomRaider.** Каждая фича сверяется с её
  Java-аналогом (см. ссылку в `PROGRESS.md` соответствующего крейта).
- **Слайсами**, а не «весь модуль за раз»: каждый слайс заканчивается
  работающей фичей end-to-end и набором тестов.
- **Тесты на каждом слое:** unit на синтетических данных + integration на
  фикстурах из апстрима.

## Метрики

- **Тестов в воркспейсе:** ~175 (passing, 0 failed) после Slice 23
  (+1 K-Line seed/key, +1 CAN seed/key 8-pair match, +CAN multi-frame strip path в transport)
- **Строк Rust-кода:** ~9100 (не считая XML/фикстур/8 КБ encrypted-kernel)
- **Зависимостей (workspace deps в `Cargo.toml`):** 18 (rusb добавлен в Slice 19)
- **Тестовых артефактов:** `fixtures/forester-xt-2007-4E42504007.bin` — 1 МБ ground-truth
  ROM (извлечён из `.srf`-дампа EcuFlash), используется в read-table / GUI / dump-rom-can verify
- **Коммитов:** 22 фич-коммита + начальный + LICENSE + PROGRESS-документация (Slice 19/20/22/23
  ещё на staging-е, ждут вашего `git commit`)
