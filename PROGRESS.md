# romraider-rs — общий статус проекта

Сводная таблица по всем крейтам. Детали каждого — в соответствующем
`crates/<name>/PROGRESS.md`.

## Сводка по крейтам

| Крейт                   | Назначение                                     | Готовность | PROGRESS                                              |
| ----------------------- | ---------------------------------------------- | :--------: | ----------------------------------------------------- |
| **romraider-core**      | Address, Endian, bytes-utils, errors           | 🟢 75%     | [`crates/romraider-core/PROGRESS.md`](crates/romraider-core/PROGRESS.md)         |
| **romraider-io**        | Transport-trait + serial/elm327/j2534          | 🟡 40%     | [`crates/romraider-io/PROGRESS.md`](crates/romraider-io/PROGRESS.md)             |
| **romraider-protocol**  | SSM/OBD/DS2/NCS диалекты                       | 🟡 40%     | [`crates/romraider-protocol/PROGRESS.md`](crates/romraider-protocol/PROGRESS.md) |
| **romraider-defs**      | Парсер + резолв ECU/log_defs XML, scaling      | 🟢 85%     | [`crates/romraider-defs/PROGRESS.md`](crates/romraider-defs/PROGRESS.md)         |
| **romraider-rom**       | ROM-image, decode/encode, checksum             | 🟢 70%     | [`crates/romraider-rom/PROGRESS.md`](crates/romraider-rom/PROGRESS.md)           |
| **romraider-logger**    | Backend логгера + plugins                      | 🟡 45%     | [`crates/romraider-logger/PROGRESS.md`](crates/romraider-logger/PROGRESS.md)     |
| **romraider-cli**       | Headless CLI (debug + smoke-tests + logger + dump-rom) | 🟢 90% | [`crates/romraider-cli/PROGRESS.md`](crates/romraider-cli/PROGRESS.md)         |
| **romraider-gui**       | egui-редактор + логгер с live XY-плотом        | 🟡 70%     | [`crates/romraider-gui/PROGRESS.md`](crates/romraider-gui/PROGRESS.md)           |

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
| 18    | SSM ReadBlock + CLI dump-rom (+MockTransport fix) | следующий коммит |

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

4. **SSM ECU-init на реальном железе:**
   ```bash
   cargo run -p romraider-cli -- ssm-init --port /dev/cu.usbserial-XXX
   ```
   ⚠️ Не тестировал на живой машине — только на mock-транспорте.

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

6. ~~**SSM ReadBlock + dump-rom CLI**~~ ✅ Slice 18
7. **J2534 Open/Connect/ReadMsgs/WriteMsgs** ([io](crates/romraider-io/PROGRESS.md)) — для современных Subaru через CAN
8. **J2534 LibraryLocator** под Win/Linux ([io](crates/romraider-io/PROGRESS.md))
9. **RamTune** — отдельный модуль для flash (опасно, тестировать на ECU-доноре)

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

- **Тестов в воркспейсе:** 134 (passing, 0 failed) на момент `slice-18` (+7: read_block × 3, dump_rom × 4 на MockTransport)
- **Строк Rust-кода:** ~7200 (не считая XML-фикстур)
- **Зависимостей (workspace deps в `Cargo.toml`):** 17
- **Коммитов:** 18 фич-коммитов + начальный + LICENSE + PROGRESS-документация
