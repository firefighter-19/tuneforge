# romraider-rom — Progress

## Цель

Модель ROM-образа: загрузка из файла, чтение/запись байтов по адресу,
декодирование байтов в значения через `storage_type`+`endian` (по
определению таблицы), кодирование обратно для редактирования, расчёт
контрольных сумм перед сохранением.

## Эталон (Java RomRaider) — `com.romraider.maps.*` (36 файлов, 8394 строк)

### Топ-уровень `maps/*`
- `Rom.java` — ROM-образ + список таблиц + кеш byte-array
- `RomID.java` — `<romid>` структура (xml_id, ecu_id, memmodel, etc.)
- `RomChecksum.java` — координатор пересчёта всех контрольных сумм при сохранении
- `Table.java`, `Table1D.java`, `Table2D.java`, `Table3D.java` — модель данных (data-layer)
- `Table1DView.java`, `Table2DView.java`, `Table3DView.java` — Swing-view (UI-layer; в нашем разделении это `romraider-gui`)
- `TableView.java` — общая база view
- `DataCell.java`, `DataCellView.java` — одна ячейка
- `Scale.java` — переехало в `romraider-defs::ResolvedScaling`
- `TableSwitch.java`, `TableSwitchView.java` — switch-таблицы (on/off enum-значения)
- `TableBitwiseSwitch.java`, `TableBitwiseSwitchView.java` — bitmap switches
- `PresetManager.java`, `PresetPanel.java` — пресеты значений (UI)
- `UserLevelException.java` — таблица скрыта для текущего user-level

### `maps/checksum/*` — алгоритмы контрольных сумм (16 файлов)
- `Calculator.java` — общий интерфейс
- `ChecksumManager.java` — выбор реализации
- `ChecksumFactory.java` — фабрика по имени
- `CalculateSTD.java`, `ChecksumSTD.java` — стандартная Subaru
- `CalculateALT2.java`, `ChecksumALT.java`, `ChecksumALT2.java` — альтернативные Subaru
- `ChecksumBYTEXOR.java` — XOR-сумма
- `ChecksumCOPY.java` — копия области (для зеркальных таблиц)
- `ChecksumE38PCM.java` — GM E38 PCM
- `ChecksumMOTRONICDOUBLE.java`, `ChecksumMOTRONICSINGLE.java` — BMW/Audi Motronic
- `NissanChecksum.java`, `NcsCoDec.java` — Nissan

## Статус

### ROM-image I/O

| Фича                                          | Java-аналог                | Rust                        | Статус |
| --------------------------------------------- | -------------------------- | --------------------------- | :----: |
| `RomImage::open(path)`                        | `OpenImageWorker.doInBackground` | `image::open`         |   ✅   |
| `RomImage::from_bytes(Vec<u8>)`               | `Rom` ctor с byte[]        | `image::from_bytes`         |   ✅   |
| `RomImage::save_as(path)`                     | `Rom.saveFile`             | `image::save_as`            |   ✅   |
| `RomImage::read(addr, len) -> &[u8]`          | `Rom.getBinary`            | `image::read`               |   ✅   |
| `RomImage::write(addr, &[u8])`                | `Rom.setData`              | `image::write`              |   ✅   |
| `is_dirty()` флаг                             | `Rom.isAbstract` (по факту) | `image::is_dirty`          |   ✅   |
| `size()` / `raw()` accessors                  | `Rom.getRomSize`           | `image::size/raw`           |   ✅   |
| Bounds-checking при выходе за пределы         | array bounds exception     | `RomError::AddressOutOfRange` |  ✅   |

### Decode (bytes → f64)

| Фича                                          | Java-аналог                | Rust                       | Статус |
| --------------------------------------------- | -------------------------- | -------------------------- | :----: |
| uint8/int8/uint16/int16/uint32/int32/float    | `Settings.STORAGE_TYPE_*` + ByteUtil | `decode::decode_cells` | ✅   |
| `hex` (как uint8) / `char` (как uint8)        | `Scale.formatHex/Char`     | `decode::decode_cells`     |   ✅   |
| Big/little endian                             | `ByteUtil.asInt` flag      | `decode::decode_cells`     |   ✅   |
| Sign extension для int8/int16                 | Java `byte`/`short` natively| explicit cast               |   ✅   |
| IEEE-754 float32                              | `Float.intBitsToFloat`     | `f32::from_bits`           |   ✅   |

### Encode (f64 → bytes)

| Фича                                          | Java-аналог                  | Rust                       | Статус |
| --------------------------------------------- | ---------------------------- | -------------------------- | :----: |
| Все 9 storage_types обратно                   | `Scale.fromReal` + ByteUtil  | `decode::encode_cells`     |   ✅   |
| Clamp в диапазон типа (`u8: 0..=255`)         | inline в `DataCell.setValue` | `encode::encode_one`       |   ✅   |
| Round-to-nearest                              | `Math.round`                 | `f64::round`               |   ✅   |
| f32 narrowing для float-cells                 | `Float.floatToIntBits`       | `f32::to_bits`             |   ✅   |

### Чтение/запись таблиц через definition

| Фича                                          | Java-аналог                    | Rust                            | Статус |
| --------------------------------------------- | ------------------------------ | ------------------------------- | :----: |
| `read_table(&ResolvedTable)` — auto-count     | `Table.populateTable`          | `image::read_table`             |   ✅   |
| `read_cells(table, count)` — explicit count   | `Table.readDataFromBin`        | `image::read_cells`             |   ✅   |
| `write_table(table, values)`                  | `Table.setData`                | `image::write_table`            |   ✅   |
| `write_cells(table, values)`                  | `Table.applyData`              | `image::write_cells`            |   ✅   |
| Size-mismatch detection                       | `TableException`               | `RomError::DecodeSizeMismatch`  |   ✅   |
| Missing storage_address/type ошибки           | `NullPointerException` в Java  | `RomError::TableMissingField`   |   ✅   |

### Контрольные суммы

| Фича                                          | Java-аналог                      | Rust                                  | Статус |
| --------------------------------------------- | -------------------------------- | ------------------------------------- | :----: |
| `ChecksumModule` trait + регистр              | `Calculator` interface           | `checksum::ChecksumModule`            |   ✅   |
| Регистр модулей по имени                      | `ChecksumFactory`                | `checksum::by_name`                   |   ✅   |
| Subaru STD checksum (SH7055)                  | `ChecksumSTD` / `CalculateSTD`   | `subaru_8bit::Subaru8Bit` (заглушка) |   🟡   |
| Subaru ALT (SH7058)                           | `ChecksumALT2` / `CalculateALT2` | `subaru_32bit::Subaru32Bit` (заглушка)|  🟡   |
| Subaru 32-bit (SH7059)                        | `ChecksumALT`                    | —                                     |   ❌   |
| BMW/Audi Motronic (single)                    | `ChecksumMOTRONICSINGLE`         | —                                     |   ❌   |
| BMW/Audi Motronic (double)                    | `ChecksumMOTRONICDOUBLE`         | —                                     |   ❌   |
| GM E38 PCM                                    | `ChecksumE38PCM`                 | —                                     |   ❌   |
| XOR checksum                                  | `ChecksumBYTEXOR`                | —                                     |   ❌   |
| COPY (mirror area)                            | `ChecksumCOPY`                   | —                                     |   ❌   |
| Nissan checksum                               | `NissanChecksum` / `NcsCoDec`    | —                                     |   ❌   |
| `verify()` перед сохранением                  | `RomChecksum.verifyChecksums`    | (trait method есть, реализаций нет)   |   ❌   |
| `fix()` пересчёт при изменениях               | `RomChecksum.calculateChecksums` | (trait method есть, реализаций нет)   |   ❌   |

### Switch-таблицы (on/off enum-значения)

| Фича                                          | Java-аналог                     | Rust          | Статус |
| --------------------------------------------- | ------------------------------- | ------------- | :----: |
| `TableSwitch` (один байт = один из enum)       | `TableSwitch.java`              | —             |   ❌   |
| `TableBitwiseSwitch` (битовая карта флагов)    | `TableBitwiseSwitch.java`       | —             |   ❌   |
| Парсятся в `romraider-defs`, но не отображаются | inline в `Rom`               | —             |   ❌   |

### Прочее

| Фича                                          | Java-аналог            | Rust                        | Статус |
| --------------------------------------------- | ---------------------- | --------------------------- | :----: |
| `RomID` структура (xml_id + ecu_id + …)       | `RomID.java`           | (живёт в `romraider-defs::RomId`) | ✅ |
| User-level (1=Beginner..5=Developer) фильтр   | `UserLevelException`   | — (есть поле в `ResolvedTable`, не применяется) | 🟡 |
| Presets (готовые значения для switch)          | `PresetManager`        | —                           |   ❌   |
| Compare two ROMs                              | `CompareImagesForm`    | — (это GUI-фича, не rom)    |   ❌   |

## TODO

### Критический путь — реальные ROM
- [ ] **Реализовать Subaru STD/ALT2 checksums** — без них `save_as` после редактирования даёт ROM, который ECU не примет
- [ ] **Auto-detect checksum-модуля по `<rom><checksum>`** — порт из `RomChecksum.java`
- [ ] **`verify()` после загрузки + `fix()` перед сохранением** — UX-цепочка «открыл, отредактировал, сохранил, ECU принял»

### Расширение покрытия
- [ ] **Switch-таблицы** — отображение и редактирование bit-флагов
- [ ] **TableBitwiseSwitch** — групповые ON/OFF опции
- [ ] **Другие checksum-семейства** — BMW Motronic, GM E38, Nissan — по запросу
- [ ] **User-level фильтр** — скрывать таблицы выше выбранного уровня сложности

### Не критично
- [ ] PresetManager — готовые наборы значений для swich-таблиц
- [ ] Memory-mapping для ROM с страничной адресацией (некоторые старые ECU)
