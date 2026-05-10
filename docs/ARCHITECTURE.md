# Архитектура romraider-rs

Этот документ фиксирует структуру воркспейса и план переезда оригинального
Java-проекта (`../RomRaider`) на Rust. Делается **итерационно**: сперва
ядро (I/O + протоколы + ROM-модель), потом логгер, потом GUI. Старый
Java-проект всё это время остаётся рабочим эталоном.

## Слои

```
┌──────────────────────────────────────────────────────────────┐
│ romraider-gui  (eframe/egui)                                  │
│   editor panel  ·  logger panel  ·  file dialogs             │
└──────────────────────────────────────────────────────────────┘
            │                              │
            ▼                              ▼
┌──────────────────────┐   ┌──────────────────────────────────┐
│ romraider-rom        │   │ romraider-logger                 │
│ ROM image            │   │ session, datalog, external sens. │
│ Tables 1D/2D/3D      │   │                                  │
│ Checksum modules     │   │                                  │
└──────────────────────┘   └──────────────────────────────────┘
            │                              │
            ▼                              ▼
┌──────────────────────┐   ┌──────────────────────────────────┐
│ romraider-defs       │   │ romraider-protocol               │
│ ECU/logger XML       │   │ SSM · OBD-II · DS2 · NCS         │
│ scaling, axes        │   │                                  │
└──────────────────────┘   └──────────────────────────────────┘
                                          │
                                          ▼
                              ┌──────────────────────────────┐
                              │ romraider-io                 │
                              │ Transport trait              │
                              │ serial · ELM327 · J2534      │
                              └──────────────────────────────┘
                                          │
                                          ▼
                              ┌──────────────────────────────┐
                              │ romraider-core               │
                              │ Address · Endian · errors    │
                              └──────────────────────────────┘
```

Правило зависимостей: **строго сверху вниз**. `core` ни от кого не зависит,
GUI зависит от всего, логгер не знает про GUI.

## Маппинг Java → Rust

| Java-пакет (`com.romraider.*`)            | Rust-крейт                              |
| ----------------------------------------- | --------------------------------------- |
| `io.connection`, `io.serial`              | `romraider-io::serial`                  |
| `io.elm327`                               | `romraider-io::elm327`                  |
| `io.j2534.*`                              | `romraider-io::j2534`                   |
| `io.protocol.ssm`                         | `romraider-protocol::ssm`               |
| `io.protocol.obd`                         | `romraider-protocol::obd2`              |
| `io.protocol.ds2`                         | `romraider-protocol::ds2`               |
| `io.protocol.ncs`                         | `romraider-protocol::ncs`               |
| `xml.*` + `definitions/*.xml`             | `romraider-defs` (XML — без изменений)  |
| `maps.*`                                  | `romraider-rom::table`                  |
| `maps.checksum.*`                         | `romraider-rom::checksum`               |
| `logger.ecu.*`                            | `romraider-logger::session`             |
| `logger.external.*` + `plugins/*.plugin`  | `romraider-logger::external` + WASM/ABI |
| `swing.*`, `editor.*`                     | `romraider-gui` (полная замена на egui) |
| `dataflowSimulation`                      | TBD — отдельный крейт после MVP         |

## Дорожная карта

### Этап 1 — ядро (≈2–4 мес)

- [x] Скелет воркспейса
- [ ] Полный SSM-протокол с тестами на захваченных дампах из Java-логгера
- [ ] Парсер `ecu_defs.xml` через `quick-xml::de`
- [ ] Парсер `logger.xml`
- [ ] Реализация `RomImage::read/write` с проверкой границ
- [ ] Загрузка таблиц 1D/2D/3D со scaling (линейный)
- [ ] Парсер произвольных выражений (через `evalexpr` или `meval`)
- [ ] Юнит-тесты на checksum для одной семьи ECU (Subaru 32-bit)

### Этап 2 — логгер (≈3–4 мес)

- [ ] Полный цикл `LoggerSession::run`: запрос → парс → broadcast
- [ ] Datalog в CSV-формате, совместимом с Java-RomRaider
- [ ] Внешние датчики: AEM, Innovate (минимум 2 для смоук-теста)
- [ ] Headless-режим в `romraider-cli logger --params=…`

### Этап 3 — GUI (≈6 мес)

- [ ] Open/Save ROM через `rfd`
- [ ] Просмотр таблиц 1D/2D в `egui_extras::TableBuilder`
- [ ] Heatmap 3D-таблиц (egui_plot heatmap или собственный widget)
- [ ] 3D-визуализация (вместо Java3D) — отдельный модуль на `wgpu`
- [ ] Интеграция с `LoggerSession` через broadcast-канал
- [ ] Сравнение двух ROM (порт `CompareImagesForm`)
- [ ] Локализация (i18n из оригинала, ресурсы перенести как есть)

### Этап 4 — равенство фич (≈6+ мес)

- [ ] Reflasher (RamTune) — это самая опасная часть, делать после полного покрытия тестами SSM
- [ ] DS2/NCS — на железе или дампах коммьюнити
- [ ] Плагинная система: WASM-runtime для пользовательских внешних датчиков
- [ ] Бандлы (Linux AppImage / macOS .app / Windows .msi) — `cargo dist`

## Что точно **не** меняется

- XML-определения ECU (`../RomRaider/definitions`, `../RomRaider/customize`)
- DTD-схемы (`*.dtd`) — это контракт с коммьюнити
- i18n-ресурсы (`../RomRaider/i18n`) — формат и ключи
- Формат datalog-файлов

## Открытые вопросы

- **3D-визуализация:** `wgpu`-окно встроено в `egui` через
  [`egui_wgpu::Callback`], но нужно прототипировать — мини-MVP до того, как
  закладывать архитектуру.
- **Плагины:** WASM (быстрая разработка, портируемость) vs нативные `.so`/`.dll`
  через trait-objects (быстрее, но требует один компилятор). Решить по итогам
  опроса коммьюнити RomRaider.
- **Реалтайм-логгер на high-speed CAN:** нужен ли отдельный поток с
  `std::thread::Builder::spawn` и `priority`-API ОС? Замерять после этапа 2.
