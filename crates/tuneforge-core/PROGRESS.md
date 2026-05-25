# tuneforge-core — Progress

## Цель

Общие примитивы воркспейса: типы адресов, endianness, утилиты для срезов
байтов, базовые ошибки. Не зависит ни от чего, кроме `serde`/`thiserror`/`byteorder`.

## Эталон (Java RomRaider)

Java-апстрим не имеет отдельного «core»-модуля — общие утилиты разбросаны
по `com.romraider.util.*` (1609 строк, 21 файл) и частично `com.romraider.maps.*`.

### `com.romraider.util.*`
- `Address.java` — нет в Java; адреса представлены как `int` в `Table.java`
- `AxisRange.java` — диапазон значений оси (для UI-валидации)
- `BitWise.java` — bit-level операции
- `ByteUtil.java` — конверсия `byte[]` → `int`/`long`, big/little-endian
- `ColorScaler.java` — раскраска ячеек (cool→warm), UI-only
- `FormatFilename.java` — нормализация путей
- `HexUtil.java` — `hex2int`, `asHex`, hex-парсинг
- `JEPUtil.java` — обёртка вокруг JEP expression-парсера (мы используем meval, лежит в `tuneforge-defs`)
- `LogManager.java` — конфигурация log4j
- `MD5Checksum.java` — расчёт MD5 (для отслеживания изменений файла)
- `NumberUtil.java` — locale-aware парсинг чисел
- `ObjectCloner.java` — deep-clone через serialization
- `ParamChecker.java` — `checkNotNull`/`checkNotNullOrEmpty`
- `Platform.java` — детект OS (Windows/Linux/Mac)
- `ResourceUtil.java` — загрузка i18n-ресурсов
- `SaxParserFactory.java` — фабрика SAX-парсера
- `SettingsManager.java` — чтение/запись настроек XML
- `ThreadCheckingRepaintManager.java` — проверка Swing EDT (UI-only)
- `ThreadUtil.java` — `sleep` + базовые thread-helpers
- `proxy/Proxifier.java`, `proxy/TimerWrapper.java`, `proxy/Wrapper.java` — generic invocation proxy (для измерения времени)

### `com.romraider.maps.*` (часть, переехавшая концептуально в core)
- Нет явного `Address`-типа в Java; `Table` использует `int storageAddress`.
  Мы выделили `Address(u32)` в core, чтобы централизовать парсинг hex-строк.

## Статус

| Фича                                  | Java-аналог                  | Rust                  | Статус |
| ------------------------------------- | ---------------------------- | --------------------- | :----: |
| `Address(u32)` + hex `FromStr`        | `int storageAddress`         | `address.rs`          |   ✅   |
| `Endian` (Big/Little) + read/write u16/u32 | `ByteUtil` методы       | `endian.rs`           |   ✅   |
| Slice/bounds helpers                  | `Arrays.copyOfRange`         | `bytes.rs`            |   ✅   |
| Hex-dump форматирование               | `HexUtil.asHex`              | `bytes::hex_dump`     |   ✅   |
| Hex-parse в `u32`/`u64`               | `HexUtil.hex2int`            | `Address::from_str`   |  🟡 только для Address |
| Bit-wise манипуляции                  | `BitWise.java`               | —                     |   ❌   |
| Color-scaling (cool→warm)             | `ColorScaler.java`           | — (UI-only, в gui)    |   ⏸   |
| Locale-aware парсинг чисел            | `NumberUtil.java`            | — (`f64::from_str`)   |   ⏸   |
| OS detection (Win/Linux/Mac)          | `Platform.java`              | — (`cfg!(target_os)`) |   ⏸   |
| Resource bundle (i18n)                | `ResourceUtil.java`          | —                     |   ❌   |
| MD5 checksum файла                    | `MD5Checksum.java`           | —                     |   ❌   |
| Deep object cloning                   | `ObjectCloner.java`          | — (`Clone` derive)    |   ⏸   |
| `ParamChecker` (null-checks)          | `ParamChecker.java`          | — (`Option`/`Result`) |   ⏸   |
| Settings persistence                  | `SettingsManager.java`       | —                     |   ❌   |
| Tracing/logging                       | `LogManager` + log4j         | через `tracing` в потребителях | ✅ |
| `AxisRange` (валидация min/max)       | `AxisRange.java`             | — (живёт в defs ScalingBase) | 🟡 |

**Легенда:** ✅ полностью — 🟡 частично — ❌ не начато — ⏸ отложено (язык даёт идиоматическую замену)

## Что точно НЕ переносим

- `ThreadCheckingRepaintManager` — Swing-специфика, в egui не нужно
- `JEPUtil` — JEP-специфическая обёртка; в `defs::expression` используется meval
- `proxy/*` — динамическая прокси-обёртка; в Rust замещается traits + generics
- `SaxParserFactory` — quick-xml не нуждается в фабрике

## TODO для будущих слайсов

- [ ] i18n: загрузка строк из bundle (нужно ли вообще? GUI пока English-only)
- [ ] MD5 для отслеживания «файл изменился» (можно через `md5` crate, ~10 строк)
- [ ] `Address` арифметика: текущий `offset(i64)` хорош, но нет `wrapping_offset` для адресных пространств с маппингом
