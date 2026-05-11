# romraider-defs — Progress

## Цель

Парсер и резолвер XML-определений RomRaider: ECU-таблицы (`<roms>`) и
логгер-параметры (`<ecus>` в `log_defs.xml`). После резолва наследования
и компиляции scaling-выражений отдаёт типизированную модель, готовую для
использования `romraider-rom` и логгера.

## Эталон (Java RomRaider) — `com.romraider.xml.*` (14 файлов, 4338 строк) + часть `maps.*`

### `xml/*`
- `DOMRomUnmarshaller.java` — главный парсер ECU-XML (`<roms>` → `Rom`)
- `DOMHelper.java` — DOM-утилиты
- `RomAttributeParser.java` — парсинг атрибутов (storageaddress, sizes, etc.)
- `TableScaleUnmarshaller.java` — парсинг `<scaling>` элементов
- `RomNotFoundException.java`, `TableIsOmittedException.java` — типизированные ошибки
- `DOMSettingsBuilder.java`, `DOMSettingsUnmarshaller.java` — settings.xml (UI prefs)
- `ConversionLayer/` (5 файлов) — конверсия из других форматов:
  - `XDFConversionLayer.java` — TunerPro XDF
  - `VDFConversionLayer.java` — формат `.vdf`
  - `BMWCodingConversionLayer.java`, `BMWConversionRomNodeManager.java` — BMW codings
  - `ConversionLayerFactory.java` — диспетчер

### `maps/*` (часть, относящаяся к определениям, не к ROM-байтам)
- `Scale.java` — scaling-определение (expression, format, units)
- `RomID.java` — `<romid>` структура

### `com.romraider.util.JEPUtil` — обёртка над JEP expression-парсером
В Java JEP парсит `expression="(x-760)*.01933677"` в AST для eval. У нас
эту роль играет `meval` в модуле `expression.rs`.

### Формат XML (актуальный в апстриме, отличается от DTD)
Мы зафиксировали реальный формат `<roms>` в `tests/fixtures/scalingbase_test.xml`
и `LearningAirflowRanges.xml`. DTD (`ecu_defs.dtd`) давно отстал от
production-формата.

## Статус

### Парсинг `<roms>` (ECU-определения)

| Фича                                          | Java-аналог                          | Rust                       | Статус |
| --------------------------------------------- | ------------------------------------ | -------------------------- | :----: |
| Парс `<roms>` → `Vec<RomDefinition>`          | `DOMRomUnmarshaller.unmarshallRoms`  | `parser::parse_str`        |   ✅   |
| `<rom>` атрибуты + nested `<romid>`           | `DOMRomUnmarshaller`                 | `RomDefinition`, `RomId`   |   ✅   |
| `<table>` рекурсивный (3D/2D/1D + nested axes)| `DOMRomUnmarshaller.unmarshallTable` | `TableDef` с `nested`      |   ✅   |
| `<scaling>` inline-or-by-base                 | `TableScaleUnmarshaller`             | `ScalingRef`               |   ✅   |
| `<scalingbase>` верхнего уровня               | `Scale`-templates                    | `ScalingBase`              |   ✅   |
| `<data>` метки для Static-осей                | `unmarshallStaticData`               | `TableDef.data: Vec<String>` |   ✅  |
| `<state>` для `type="Switch"`                 | `addPresetValue`                     | `TableDef.states: Vec<SwitchState>` + `data_bytes()` | ✅ |
| `<bit>` для `type="BitwiseSwitch"`            | `setPresetValues`                    | `TableDef.bits: Vec<SwitchBit>` + `bit_position()` | ✅ |
| Парс из reader/string/file                    | `DocumentBuilder.parse`              | `parse_reader/str/file`    |   ✅   |
| Helper-методы (`find_rom_by_xml_id`, `find_scaling_base`) | inline в `Rom.java`     | `RomsDocument` impl        |   ✅   |
| Обнаружение malformed XML                     | `SAXException`                       | `DefError::Xml`            |   ✅   |

### Резолв наследования

| Фича                                          | Java-аналог                              | Rust                          | Статус |
| --------------------------------------------- | ---------------------------------------- | ----------------------------- | :----: |
| ROM-base chain `<rom base="…">`               | `DOMRomUnmarshaller.processRomBase`      | `Resolver::rom_base_chain`    |   ✅   |
| Merge table-маппинг по name                   | `Rom.applyTablesToTables`                | `Resolver::resolve_rom`       |   ✅   |
| Table-base `<table base="…">`                 | `unmarshallTableBase`                    | `Resolver::resolve_table_base`|   ✅   |
| Merge nested axes по `kind` или `name`        | inline                                   | `merge_nested`                |   ✅   |
| Scaling-base `<scaling base="…">`             | `TableScaleUnmarshaller.applyScalingBase`| `Resolver::materialize_scaling`|  ✅   |
| Inline-scaling override-ит base               | inline                                   | `materialize_scaling`         |   ✅   |
| Cycle detection (rom + table)                 | StackOverflow в Java                     | `DefError::Cycle`             |   ✅   |
| Missing base reference                        | `RomNotFoundException`                   | `DefError::BaseNotFound`      |   ✅   |

### Типизация

| Фича                                          | Java-аналог                       | Rust                          | Статус |
| --------------------------------------------- | --------------------------------- | ----------------------------- | :----: |
| `TableKind` enum (1D/2D/3D/X/Y/Static/Switch/BitwiseSwitch) | `Table.Type` enum    | `typed::TableKind` (Switch/BitwiseSwitch case-insensitive) | ✅ |
| `StorageType` enum (9 типов)                  | `Settings.STORAGE_TYPE_*` constants | `typed::StorageType`        |   ✅   |
| Endian enum                                   | hardcoded в `ByteUtil`            | `romraider_core::Endian`      |   ✅   |
| Парс `storageaddress="0xC11D4"` → Address     | `RomAttributeParser.parseHexString`| `Address::from_str`          |   ✅   |
| Парс `sizex/sizey` → u32                      | `Integer.parseInt`                | `parse_opt_u32`               |   ✅   |
| Парс `fineincrement` → f64                    | `Double.parseDouble`              | `parse_opt_f64`               |   ✅   |
| `userlevel` (1..5) → u8                       | `Integer.parseInt`                | `parse_opt_u8`                |   ✅   |

### Scaling expression compilation

| Фича                                          | Java-аналог             | Rust                       | Статус |
| --------------------------------------------- | ----------------------- | -------------------------- | :----: |
| Парс `expression="x*0.5"` в eval-функцию      | `JEPUtil` + JEP-2.4.0   | `meval` через `expression::CompiledScaling::compile` | ✅ |
| `to_real(byte)` (byte → real)                 | `Scale.toReal`          | `CompiledScaling::to_real`  |   ✅   |
| `to_byte(real)` (обратное)                    | `Scale.toByte`          | `CompiledScaling::to_byte`  |   ✅   |
| Round-trip симметричность                     | JEP guarantees          | end-to-end тесты в `tests/` |   ✅   |
| Нормализатор leading-dot decimal (`.84` → `0.84`) | JEP принимает as-is | `normalize_decimal`         |   ✅   |
| Sanity-eval при компиляции (`x=0`)            | —                       | в `ResolvedScaling::compile`|   ✅   |
| Поддержка `GetLogParam("name")` (cross-param ссылки) | JEP с custom function | —                  |   ❌   |
| Поддержка функций `sin/cos/log/exp`           | JEP standard library    | meval умеет, но не используется в тестах | 🟡 |

### Парсинг `log_defs.xml` (логгер)

| Фича                                          | Java-аналог                            | Rust                            | Статус |
| --------------------------------------------- | -------------------------------------- | ------------------------------- | :----: |
| Парс `<ecus>` → `LoggerDocument`              | `EcuDefinitionDocumentLoader`          | `parse_log_str/_file`           |   ✅   |
| `<ecu_tools><convert_factor>`                 | `EcuTools.parseConverters`             | `ConvertFactor`                 |   ✅   |
| `<logprotocols><logprotocol>` (SSM/OBD/DS2/NCS)| `LogProtocols`                        | `LogProtocol`                   |   ✅   |
| `<ecu>` template (`id="base"`)                | `EcuTemplate`                          | `LoggerEcu::is_template`        |   ✅   |
| `<ecu>` concrete (`id="<hex>"`, `include="…"`)| `EcuTemplate.applyInclude`             | `LoggerEcu` + `include` field   |   🟡 структуру парсим, резолв не делаем |
| `<parameter>` (offset/storagetype/expr/metric)| `EcuParameterImpl`                     | `LogParameter`                  |   ✅   |
| `<alt>` альтернативные адреса                 | `EcuAlternateParameter`                | `Alt`                           |   ✅   |
| `find_ecu(id)` / `find_convert_factor(type)`  | inline                                 | `LoggerDocument` impl           |   ✅   |
| Резолв `include="ssmbase"` (merge параметров) | `EcuParameterImpl.applyInclude`        | `LoggerDocument::resolve_ecu` (HashSet cycle-detect, child overrides by id) | ✅ |
| Eval `[value]/4`-формул (другой синтаксис чем `x`) | JEP с переменной `value`         | `compile_log_expr` ([value]→x preprocessor) | ✅ |
| `LogParameter::compile()` (offset→Address, storage→enum) | `EcuParameterImpl` ctor         | `CompiledLogParameter`          |   ✅   |
| Switches (`<switch>`) — bit-флаги             | `EcuSwitchImpl`                        | —                               |   ❌   |
| DTC коды (`<dtcode>`)                         | `EcuDataConverterImpl`                 | —                               |   ❌   |
| `<ecuparam>` — параметры специфичные ECU      | `EcuParameterImpl`                     | —                               |   ❌   |

### Conversion layers

| Фича                              | Java-аналог                      | Rust | Статус |
| --------------------------------- | -------------------------------- | ---- | :----: |
| TunerPro XDF импорт               | `XDFConversionLayer`             | —    |   ❌   |
| VDF импорт                        | `VDFConversionLayer`             | —    |   ❌   |
| BMW codings импорт                | `BMWCodingConversionLayer`       | —    |   ❌   |
| Settings persistence (settings.xml)| `DOMSettingsBuilder/Unmarshaller`| —    |   ❌   |

## TODO

### Высокий приоритет
- [ ] **Резолв `include="ssmbase"` для LoggerEcu** — без этого concrete-ECU считаются «без параметров»; нужен для логгера на реальном железе
- [ ] **Eval логгер-формул с `[value]`** — адаптировать `expression.rs` чтобы понимать оба синтаксиса (`x` для ROM-defs, `[value]` для log-defs)
- [ ] **Switches / DTCs / ecuparams в log_defs** — на случай если будущие апстрим-определения их добавят

### Средний приоритет
- [ ] **`GetLogParam("Engine Speed")` cross-references** — в `[value]*100/GetLogParam("Engine Speed")` нужно подставить значение другого параметра в реалтайме; без этого `inj`-конвертация не работает корректно

### Низкий приоритет
- [ ] Settings.xml персистенс (UI preferences)
- [ ] Conversion layers (XDF/VDF/BMW) — отдельный домен, скорее не нужен сразу
