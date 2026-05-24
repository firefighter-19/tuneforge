# romraider-rs

[![CI](https://github.com/firefighter-19/romraider-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/firefighter-19/romraider-rs/actions/workflows/ci.yml)
[![License: GPL-2.0+](https://img.shields.io/badge/License-GPL--2.0%2B-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org/)

**Languages:** [English](README.md) · **Русский**

**Mac-native Rust-инструмент для тюнинга Subaru ECU: редактор прошивок +
логгер + ROM-dump через Tactrix Openport 2.0, всё в одном бинарнике.**

Объединяет в одном приложении две вещи, для которых на Mac исторически
приходилось держать Windows-VM:

- **Редактор + логгер в духе [RomRaider](https://github.com/RomRaider/RomRaider)**
  (Java-приложение, ломаное на 64-bit OS) — открыть `.bin`, поправить таблицы
  по `ecu_defs.xml`, починить checksum, посмотреть/сравнить ROM-ы, поснимать
  лог по SSM2.
- **Dump ROM в духе [EcuFlash](https://www.tactrix.com/index.php?Itemid=58)**
  (freeware от Tactrix, тоже Windows-only) — снять полный 1 МБ образ с ECU
  через kernel-upload по UDS-over-CAN. Реализовано целиком на Rust, без
  J2534-DLL и без эмуляции.

> **Это производная работа.** RomRaider — open-source проект сообщества
> RomRaider.com с 2006 года, под GNU GPL v2.0+. EcuFlash — freeware от
> Tactrix; код её мы не используем, но функционально воспроизводим dump-flow
> (UDS sequence, seed/key, encrypted kernel) на основе reverse-engineering-а
> capture-ов EcuFlash-сессий. Этот репозиторий распространяется под GPL v2.0+
> (модуль `romraider-kernel` — под GPL v3.0+, изолированно). См.
> [Происхождение и лицензия](#происхождение-и-лицензия).

## Что работает прямо сейчас

Подтверждено на живой машине (2007 USDM Subaru Forester XT, ROM `4E42504007`,
Tactrix Openport 2.0 на Mac ARM):

- ✅ **SSM2 ECU-init по K-Line** через `romraider-cli ssm-init --tactrix`
- ✅ **GUI-редактор**: открыть `forester-xt-2007-4E42504007.bin` + `ecu_defs.xml`,
  отредактировать Target Boost / Wastegate Duty, авто-fix checksum, Save As;
  diff двух ROM-ов с heatmap, undo/redo, Changes-since-open, tooltip с raw/Δ
- ✅ **Headless-инспекция** ROM-ов и определений (`inspect-def`, `read-table`)
- ✅ **SSM-логгер** с резолвом `log_defs.xml`, live XY-plot в GUI
- ✅ **Полный 1 МБ ROM-dump через UDS-over-CAN** за ~44 с — byte-for-byte
  совпадает с дампом EcuFlash:
  ```bash
  sudo cargo run -p romraider-cli --features kernel-upload -- \
      dump-rom-can --output /tmp/forester-mac-dump.bin
  ```

## Что **не** делает (и пока не будет)

- ❌ **Flash-write в ECU**. Это explicit non-goal. У автора одна машина и нет
  donor-ECU; риск кирпича превышает пользу. Если когда-то и появится — только
  на доноре. Kernel-upload пишет **только в RAM**, никакого flash-erase/program.
- ❌ **Не-Subaru ECU.** Эталон — Java-RomRaider, тестовая машина — Subaru.
- ❌ **Windows-only paths** (J2534 DLL-emulation, Wine, x86-USB-passthrough)
  — нам интересен именно Mac-native поток.

См. [`PROGRESS.md`](PROGRESS.md) для полной картины + roadmap по слайсам.

## Структура воркспейса

| Crate                | Назначение                                                              |
| -------------------- | ----------------------------------------------------------------------- |
| `romraider-core`     | Общие типы: адреса, endian, ошибки, утилиты для байтов                  |
| `romraider-io`       | Транспорты: serial, J2534, ELM327, **Tactrix Openport 2.0 (rusb, K-Line + CAN)** |
| `romraider-protocol` | Диалекты: SSM, OBD-II, DS2, NCS, RamTune                                 |
| `romraider-defs`     | Парсер XML-определений ECU и логгера                                     |
| `romraider-rom`      | Образ ROM, таблицы 1D/2D/3D, scaling/формулы, патчер контрольных сумм    |
| `romraider-logger`   | Backend логгера, подписки, datalog-файлы, внешние датчики                |
| `romraider-cli`      | Headless CLI — отладка, smoke-test, **dump-rom, ssm-init, peek-vin** и т.п. |
| `romraider-gui`      | GUI на `eframe`/`egui` (редактор + логгер + diff/heatmap/undo/changes)   |
| `romraider-kernel`   | (opt-in, GPL-3.0) Subaru SH7058 kernel-upload + UDS-over-CAN dump-flow   |

## Сборка

```bash
cargo build --workspace                            # без kernel-upload
cargo test  --workspace                            # ~175 тестов
cargo run   -p romraider-cli -- --help             # headless-команды
cargo run   -p romraider-gui                       # редактор + логгер
```

Для **ROM-dump через Tactrix** требуется opt-in feature и `sudo` (libusb
bulk-IO на macOS):

```bash
sudo cargo run -p romraider-cli --features kernel-upload -- \
    dump-rom-can --output ./my-dump.bin
```

`--features kernel-upload` подключает crate `romraider-kernel` (GPL-3.0)
к CLI. По умолчанию он отключён — workspace остаётся под GPL-2.0+ если
этот код не используется.

## Совместимость с оригиналом

* XML-определения ECU (`definitions/`, `customize/`, `i18n/` в апстриме)
  **не модифицируются** и подключаются как есть — это самая ценная часть
  оригинального проекта, накопленная сообществом за 15+ лет.
* Логика протоколов сверяется с `com.romraider.io.protocol.*` из апстрима
  и тестируется на захваченных дампах коммуникаций.
* Алгоритмы checksum копируются из `com.romraider.maps.checksum.*` один в один
  и покрываются юнит-тестами.

## Дорожная карта

См. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Происхождение и лицензия

Этот проект — производная работа от [RomRaider](https://github.com/RomRaider/RomRaider).
Авторские права на оригинальный код принадлежат разработчикам RomRaider.com
(`Copyright (C) 2006-2022 RomRaider.com` — см. заголовки исходников апстрима).
Условия распространения совпадают с оригиналом: **GNU GPL версии 2 или новее**.

**Отдельно — crate `romraider-kernel`** (kernel-upload и UDS-dump): под **GPL v3.0+**,
поскольку использует наработки от GPL-3 проектов
([`fenugrec/nisprog`](https://github.com/fenugrec/nisprog),
[`fenugrec/npkern`](https://github.com/fenugrec/npkern)).
Crate изолирован за feature-flag-ом `kernel-upload`, остальные крейты остаются
под GPL-2.0+.

**О EcuFlash:** мы не используем её код (EcuFlash — freeware от Tactrix, не
open-source). Dump-flow реализован независимо: reverse-engineering Wireshark-
capture-ов EcuFlash-сессий + публичные ресурсы (nisprog/npkern + работа
[james-portman](https://github.com/james-portman/subaru-ecu-flashing)).
Encrypted kernel-binary, который мы загружаем в RAM ECU, изначально пришёл с
OpenECU/Tactrix (`OpenECU Subaru SH7058 OCP CAN Kernel V1.07`); он
переиспользуется как-есть из capture, поскольку контракт между ECU и kernel-ом
жёстко зашит в bootloader. Если правообладатель сочтёт это проблемой —
напишите issue, удалим бинарник и оставим только инструкции «вытащите свой».

Полный текст лицензии — в [`LICENSE`](LICENSE).

Если вы используете определения, словари переменных или checksum-алгоритмы
из апстрим-репозитория RomRaider — обязательно сохраняйте указание авторства
и условий лицензии при дальнейшем распространении.

## Безопасность и предупреждение

Этот инструмент **читает** прошивку и редактирует **файлы на диске**. Он
**не пишет ничего обратно в ECU** — flash-write не реализован и реализован
не будет без donor-ECU для тестов. Тем не менее, при работе с тюнингом ECU
любая ошибка может привести к нестабильной работе двигателя или его
повреждению. Используйте на свой страх и риск; авторы не несут
ответственности за повреждение оборудования или двигателя.
