# romraider-rs

Экспериментальный порт [RomRaider](https://github.com/RomRaider/RomRaider) на Rust.
Цель — постепенно перенести функциональность Java-приложения, начиная с
низкоуровневого I/O и заканчивая GUI, не ломая существующий проект.

Существующий Java-проект остаётся в соседней директории (`../RomRaider`)
и является эталонной реализацией, на которую сверяются все портированные модули.

## Структура воркспейса

| Crate                | Назначение                                                              |
| -------------------- | ----------------------------------------------------------------------- |
| `romraider-core`     | Общие типы: адреса, endian, ошибки, утилиты для байтов                  |
| `romraider-io`       | Транспорты: serial-порт, J2534 (через libloading), ELM327                |
| `romraider-protocol` | Диалекты: SSM, OBD-II, DS2, NCS, RamTune                                 |
| `romraider-defs`     | Парсер XML-определений ECU и логгера                                     |
| `romraider-rom`      | Образ ROM, таблицы 1D/2D/3D, scaling/формулы, патчер контрольных сумм    |
| `romraider-logger`   | Backend логгера, подписки, datalog-файлы, внешние датчики                |
| `romraider-cli`      | Headless CLI — для отладки I/O до появления GUI                          |
| `romraider-gui`      | GUI на `eframe`/`egui` (редактор + логгер)                               |

## Сборка

```bash
cargo build --workspace
cargo test  --workspace
cargo run   -p romraider-cli -- --help
cargo run   -p romraider-gui
```

## Совместимость с оригиналом

* XML-определения ECU (`../RomRaider/definitions`, `../RomRaider/customize`,
  `../RomRaider/i18n`) **не модифицируются** и подключаются как есть —
  это самая ценная часть оригинального проекта.
* Логика протоколов сверяется с `com.romraider.io.protocol.*` и тестируется
  на захваченных дампах коммуникаций.
* Алгоритмы checksum для всех ECU копируются из `com.romraider.maps.checksum.*`
  один в один и покрываются юнит-тестами.

## Дорожная карта

См. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Лицензия

GPL-2.0-or-later — как у оригинального RomRaider.
