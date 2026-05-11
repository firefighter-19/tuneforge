# romraider-gui — Progress

## Цель

Desktop GUI на `eframe`/`egui`: открыть ROM, выбрать определение, редактировать
таблицы, смотреть живые данные с ECU. Аналог Swing-приложения апстрима, но
переписанный с нуля под immediate-mode UI.

## Эталон (Java RomRaider) — несколько пакетов

### `editor/ecu/` (3 файла) — главный фрейм редактора
- `ECUEditor.java` — `JFrame` главного окна
- `ECUEditorManager.java` — singleton-доступ к редактору
- `OpenImageWorker.java` — async-загрузка ROM (`SwingWorker`)

### `swing/` (41 файл) — UI-компоненты редактора
- `ECUEditorMenuBar.java`, `ECUEditorToolBar.java`, `TableMenuBar.java`, `TableMenuItem.java`, `TableToolBar.java` — меню и тулбары
- `MDIDesktopPane.java` — multi-document interface (несколько таблиц открыты как поддокументы)
- `TableFrame.java` — окно одной таблицы внутри MDI
- `Table1DView.java`, `Table2DView.java`, `Table3DView.java` — рендереры (в `maps/`)
- `DataCellView.java` — одна ячейка (color, value, editable)
- `RomTree.java`, `RomTreeRootNode.java`, `CategoryTreeNode.java`, `TableTreeNode.java`, `TableChooserTreeNode.java` — дерево таблиц
- `RomCellRenderer.java`, `RomFilterPanel.java`, `RomPropertyPanel.java` — UI ROM-properties
- `TablePropertyPanel.java` — настройки конкретной таблицы
- `ScalesTableModel.java`, `SwitchStateTableModel.java`, `ParameterIdsTableModel.java` — model-view-controller для tables
- `CompareImagesForm.java` — diff двух ROM (важная фича!)
- `DataflowFrame.java` — окно симулятора (отдельно)
- `JProgressPane.java`, `DebugPanel.java`, `JTableChooser.java`, `SettingsForm.java` — utility-формы
- `SetFont.java`, `LookAndFeelManager.java` — стили
- `ECUEditorNumberField.java` — input-поле с проверкой
- `CustomToolbarLayout.java`, `AbstractFrame.java` — базовые компоненты
- `DefinitionFilter.java`, `DefinitionManager.java` — выбор/менеджмент определений
- `ECUImageFilter.java`, `GenericFileFilter.java` — фильтры файлов
- `menubar/Menu.java`, `MenuItem.java`, `RadioButtonMenuItem.java`, `action/*` — меню-actions
- `util/NumberVerifier.java`, `SelectionVerifier.java` — input validators

### `logger/ecu/ui/` (~50 файлов) — UI логгера
- `paramlist/` — селектор параметров для логгирования
- `swing/` — основные виджеты
- `tab/` — табы (Dashboard, Graph, Data, etc.)
- `handler/` — обработчики для разных типов отображения:
  - `dash/` — gauge-dashboard
  - `dataflow/` — dataflow-граф
  - `dyno/` — dyno-расчёты в реалтайме
  - `file/` — запись в файл
  - `graph/` — XY-график
  - `injector/` — injector duty/scaling helper
  - `livedata/` — таблица live-данных
  - `maf/` — MAF scaling tool
  - `table/` — таблица-просмотр
- `playback/` — воспроизведение датлогов

### `dataflowSimulation/` (4 файла) — симулятор dataflow
- `DataflowSimulation.java`, `CalculationAction.java`, `GenericAction.java`, `TableAction.java` —
  упрощённый дино-симулятор: подаёшь параметры, получаешь прогнозируемое поведение

### `ramtune/test/` (9 файлов) — RamTune (запись прошивки)
- `RamTuneTestApp.java` — отдельный Swing-window
- `command/executor/CommandExecutor*.java` — выполнение команд
- `command/generator/*` — генерация SSM read/write/init команд

## Статус

### Главное окно

| Фича                                          | Java-аналог                | Rust                            | Статус |
| --------------------------------------------- | -------------------------- | ------------------------------- | :----: |
| Окно `eframe::App`                            | `ECUEditor` (JFrame)       | `App` в `app.rs`                |   ✅   |
| Топ-меню                                      | `ECUEditorMenuBar`         | `egui::menu::bar` в `app.rs`    |   ✅   |
| Tab: Editor / Logger                          | `MDIDesktopPane`-stub      | `Tab` enum + selectable_value   |  🟡 примитивно |
| File → Open ROM…                              | `ECUEditor.openRom`        | через `rfd::FileDialog::pick_file` | ✅ |
| File → Open Def…                              | `DefinitionManager`        | через `rfd::FileDialog`         |   ✅   |
| File → Save ROM As…                           | `ECUEditor.saveRom`        | `editor::save_rom_as`           |   ✅   |
| Авто-fix Subaru-checksum при сохранении        | `Rom.saveFile` + popup     | `save_rom_as` зовёт `subaru_classic::fix` |   ✅   |
| Status-индикатор checksum (N/M ✓/✗)           | popup при ошибке           | `render_status` + `checksum_summary` |   ✅   |
| Кнопка «Fix now» при инвалидных                | popup-dialog               | `fix_checksums_now`             |   ✅   |
| Notice-сообщение «Saved to …»                  | таскбар JFrame             | `self.notice` поле + render     |   ✅   |
| Quit                                          | `ECUEditor.close`          | `ViewportCommand::Close`        |   ✅   |
| File → Save (in-place, не As…)                | `ECUEditor.saveRom`        | — (есть только Save As)         |   ❌   |
| File → Close ROM                              | `ECUEditor.closeRom`       | —                               |   ❌   |
| File → Recent (последние открытые)            | `Settings.recentRoms`      | —                               |   ❌   |
| Edit → Undo / Redo                            | undo-manager               | `UndoLog` + Edit-меню + Ctrl+Z/Y/Shift+Z | ✅ |
| Help → About                                  | `AboutForm`                | —                               |   ❌   |
| Settings dialog                               | `SettingsForm`             | —                               |   ❌   |
| Multi-document interface (несколько ROM)      | `MDIDesktopPane`           | — (одна таблица в фокусе)       |   ❌   |

### Sidebar — ROM picker + дерево таблиц

| Фича                                          | Java-аналог                          | Rust                                 | Статус |
| --------------------------------------------- | ------------------------------------ | ------------------------------------ | :----: |
| ROM-ID selector (ComboBox)                    | `RomTreeRootNode`                    | `egui::ComboBox` в `render_sidebar`  |   ✅   |
| Дерево таблиц по категории                    | `RomTree` + `CategoryTreeNode`       | `egui::CollapsingHeader` по `BTreeMap` |  ✅   |
| Выбор таблицы — `selectable_label`            | `TableTreeNode`                      | `render_table_tree`                  |   ✅   |
| Иконки рядом с типами таблиц                  | `RomCellRenderer`                    | —                                    |   ❌   |
| Поиск по таблицам                             | `TableChooserTreeNode` (Ctrl+F)      | —                                    |   ❌   |
| Раскраска по user-level                       | `RomCellRenderer`                    | —                                    |   ❌   |
| Фильтр по user-level                          | `RomFilterPanel`                     | —                                    |   ❌   |
| Drag-n-drop таблицу в сравнение               | `CompareImagesForm`                  | —                                    |   ❌   |

### Окно таблицы

| Фича                                          | Java-аналог                          | Rust                                 | Статус |
| --------------------------------------------- | ------------------------------------ | ------------------------------------ | :----: |
| 3D-таблица: X сверху, Y слева, data в центре  | `Table3DView`                        | `render_3d` в `editor.rs`            |   ✅   |
| 2D/1D-таблица плоской сеткой                  | `Table2DView`/`Table1DView`          | `render_flat`                        |   ✅   |
| Static-axis (текстовые метки)                 | static labels                        | `render_static_axis`                 |   ✅   |
| Метаданные сверху (kind/storage/dims/units)   | `TablePropertyPanel`                 | `render_table_header`                |   ✅   |
| Description под заголовком                    | `Table.getDescription`               | inline в `render_table_header`       |   ✅   |
| Редактирование ячеек — `DragValue`            | `DataCellView` с key-listener        | `egui::DragValue` per cell           |   ✅   |
| Write-back при изменении                      | `Table.setRealValue` + `Rom.setData` | `write_back` через `to_byte`         |   ✅   |
| Dirty-индикатор `* modified`                  | заголовок окна с `*`                 | в `render_status`                    |   ✅   |
| Precision из `format="0.00"`                  | `Scale.formatter`                    | `precision_from_format`              |   ✅   |
| Speed из `fine_increment` для DragValue       | стрелки и shift-clicks               | `cell_speed`                         |   ✅   |
| **Heatmap-раскраска** (cool→warm по value)    | `DataCellView.setColor` + `ColorScaler` | `heat_color` + `heatmap_range` (toggle в статусе) | ✅ |
| Heatmap-диапазон из `scaling.min/max`         | `Scale.min/max`                      | `heatmap_range` (с fallback на auto) |   ✅   |
| Min/Max validation при вводе                  | `Scale.min/max` clamp                | —                                    |   ❌   |
| Copy/Paste значений в Excel-формате           | `Table.copy/paste` + `Transferable`  | —                                    |   ❌   |
| Multi-cell selection + bulk-edit (+10%, =N)   | `TableMenuItem` actions              | —                                    |   ❌   |
| Undo/Redo для редактирования (с history limit) | undo-manager                        | `UndoLog` (MAX=100), записи на каждый `write_back` | ✅ |
| Switch-таблицы UI (radio-buttons)             | `TableSwitchView`                    | `render_switch` + radio-group + undo-aware write | ✅ |
| Bitwise switch UI (checkboxes для битов)      | `TableBitwiseSwitchView`             | `render_bitwise_switch` + checkbox per bit | ✅ |
| Reset to base values                          | `Table.resetToOriginal`              | —                                    |   ❌   |
| Tab-keyboard navigation между ячейками        | Swing default                        | partial via DragValue                |  🟡   |
| **Tooltip на hover** — addr/raw/real/Δ/formula | `Table.getDescription` popup        | `CellTooltip` + `cell_tooltip_ui` через `on_hover_ui` | ✅ |
| Ctrl+1/2/3 переключение views (data/raw/hex)  | `TableToolBar`                       | —                                    |   ❌   |
| Right-click context menu                      | `TableMenuItem` popup                | —                                    |   ❌   |

### Сравнение двух ROM (Compare)

| Фича                                          | Java-аналог              | Rust                                | Статус |
| --------------------------------------------- | ------------------------ | ----------------------------------- | :----: |
| Открыть второй ROM как «base»                 | `CompareImagesForm`      | `EditorPanel::load_compare_rom`     |   ✅   |
| File → Open/Close Compare ROM menu items      | `CompareImagesForm`-menu | `app.rs` File-меню                  |   ✅   |
| Подсветка отличий в таблицах (bg color)       | `Table.compare` + bg     | `render_cell` + `diff_bg`           |   ✅   |
| Diff-режим: показывать ΔX                     | toggle в Compare-form    | `DisplayMode::{Values,Diff}` toggle |   ✅   |
| Threshold для «значимого» отличия             | `Settings.compareSensitivity` | EPSILON 1e-9 в `diff_bg`       |  🟡 hardcoded |
| Подсветка таблиц в дереве (есть/нет изменений)| `RomTree.markChanged`    | —                                   |   ❌   |
| Процентный режим diff (%)                     | toggle в Compare-form    | —                                   |   ❌   |
| Сравнение axes                                | `Table.compare`          | —                                   |   ❌   |
| Экспорт diff-отчёта                           | `CompareImagesForm` save | —                                   |   ❌   |

### Логгер UI (нужен ли отдельный layout?)

| Фича                                          | Java-аналог                          | Rust                          | Статус |
| --------------------------------------------- | ------------------------------------ | ----------------------------- | :----: |
| Logger tab — настройка + live-плот            | logger window                        | `logger.rs`: full LoggerPanel | ✅ |
| Селектор параметров для логгирования          | `paramlist/`                         | checkbox-список из `ResolvedLogEcu` | ✅ |
| XY-график с egui_plot                         | `handler/graph/`                     | Multi-line `Plot` с legend; rolling history по 600 точек | ✅ |
| Worker-thread + mpsc-канал → UI               | `LoggerController` SwingWorker      | `std::thread` + `std::sync::mpsc` + AtomicBool stop-flag | ✅ |
| Загрузка `log_defs.xml` из панели             | `LoggerSettings`                     | rfd-picker + auto-resolve ECU | ✅ |
| Gauge-dashboard (стрелочные / цифровые)       | `handler/dash/`                      | —                             |   ❌   |
| Live-data таблица                             | `handler/livedata/`                  | —                             |   ❌   |
| Запись в файл из UI                           | `handler/file/`                      | —                             |   ❌   |
| Playback `.csv`                               | `playback/`                          | —                             |   ❌   |
| Dyno-расчёты в реалтайме                      | `handler/dyno/`                      | —                             |   ❌   |
| Injector helper                               | `handler/injector/`                  | —                             |   ❌   |
| MAF scaling helper                            | `handler/maf/`                       | —                             |   ❌   |

### Dataflow Simulation

| Фича                              | Java-аналог              | Rust | Статус |
| --------------------------------- | ------------------------ | ---- | :----: |
| Симулятор dataflow (input→output) | `dataflowSimulation/*`   | —    |   ❌   |

### RamTune (reflash)

| Фича                                | Java-аналог             | Rust | Статус |
| ----------------------------------- | ----------------------- | ---- | :----: |
| Отдельное окно для reflash          | `RamTuneTestApp`        | —    |   ❌   |
| ECU-init проверка                   | `EcuInitCommandGenerator` | —  |   ❌   |
| Read prog memory                    | `ReadCommandGenerator`  | —    |   ❌   |
| Write prog memory (опасно!)         | `WriteCommandGenerator` | —    |   ❌   |

### 3D-визуализация (вместо Java3D)

| Фича                              | Java-аналог              | Rust | Статус |
| --------------------------------- | ------------------------ | ---- | :----: |
| 3D-surface plot таблицы           | `Table3DView` + Java3D   | —    |   ❌   |
| Поворот мышью / зум               | Java3D default behaviour | —    |   ❌   |

## TODO

### Сразу полезно (UX)
- [ ] **Heatmap-раскраска ячеек** — для визуального скана таблицы (синий→жёлтый→красный)
- [ ] **Undo/Redo** — без него UX страдает; нужен log изменений в `editor`
- [ ] **Recent files** — Settings-style persistence
- [ ] **In-place Save (Ctrl+S)** — сейчас только Save As
- [x] ~~**Description tooltip для ячеек**~~ — сделано в Slice 14, но без `<description>` (то лежит в header; добавить в tooltip — TODO)

### Важные фичи
- [ ] **Compare two ROMs** — кросс-сравнение, ключевая фича апстрима
- [ ] **Switch-таблицы UI** — без них целый класс ECU-настроек недоступен
- [ ] **Copy/paste в Excel-формат** — экспорт/импорт для офлайн-обработки
- [ ] **Multi-cell selection + bulk-edit** — `Shift+drag`, `Ctrl+A`, `+10%` operation

### Логгер
- [ ] **XY-график живых данных** — egui_plot уже подключён, надо привязать к broadcast-каналу `LoggerSession`
- [ ] **Селектор параметров** для логгирования
- [ ] **Gauge-dashboard** — циферблаты для критичных параметров

### Долгий пробег
- [ ] **3D-surface** через `wgpu`-callback (заменяет Java3D)
- [ ] **RamTune окно** для flash/dump
- [ ] **Settings persistence** (recent, layout, theme)
- [ ] **i18n** (если будет запрос от не-english-users)
