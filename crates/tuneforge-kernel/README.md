# tuneforge-kernel

Kernel-upload путь для **дампа ROM** с Subaru ECU на базе Renesas SH7055/SH7058
(под управлением UDS-style команд через K-Line). Прямой `SSM2 ReadBlock (0xA0)`
на этих ECU возвращает stub-`0xFF` из-за анти-fuzz защиты — единственный
рабочий способ снять реальную прошивку это:

1. **Войти в KWP2000 programming-режим** через `SID 0x10`.
2. **Пройти seed/key challenge** (`SID 0x27`) — 16-round Feistel-cipher.
3. **Залить kernel-binary в RAM ECU** (`SID 0x34` requestDownload → `SID 0x36`
   transferData по 128 байт) на адрес `0xFFFF6000` (SH7055) или `0xFFFF3000`
   (SH7058).
4. **Передать управление kernel-у** через `SID 0x31` startRoutine (`01 01`),
   ECU прыгает на trampoline в RAM.
5. **Общаться с kernel-side wire protocol** — собственные SID-ы (`SID_DUMP`,
   `SID_TP`, `SID_CONF`), кернел стримит ROM 32-байтными блоками, опционально
   на повышенном baud-rate.

## Лицензия

**GPL-3.0-or-later** (в отличие от остального workspace, MIT/GPL-2.0-or-later).
Это связано с прямой порт-имплементацией из:

- [`fenugrec/nisprog`](https://github.com/fenugrec/nisprog) — C-эталон seed/key
  algorithm и upload-последовательности.
- [`fenugrec/npkern`](https://github.com/fenugrec/npkern) — RAM-loader kernel
  для SH7055/SH7058 (бинари в `kernels/`).

Оба апстрима — GPL-3.0; наш порт сохраняет ту же лицензию.

## Изоляция от остального workspace

Крейт **не подключается по умолчанию** в `tuneforge-cli` — нужно собрать с
`--features kernel-upload`. Это сделано чтобы:

- main-tree (MIT/GPL-2-or-later) оставался lint-clean относительно GPL-3-only;
- пользователь явно opt-in выбирает kernel-flash возможность;
- в случае брика ECU контракт «опасные операции включены пользователем» легче
  отстоять.

## Что НЕ делает этот крейт

- **НЕ пишет в flash** — только RAM (0xFFFFxxxx). Сам по себе kernel-upload
  не может «закирпичить» ECU при штатной работе; нужно явно отправить
  `SID_FLREQ` / `SID_FLASH`, чего наш код не делает.
- **НЕ умеет ECU-семьи, отличные от Subaru SH7055/SH7058** — Nissan и прочие
  используют **тот же** Feistel-algo и upload-flow, но другие константы / kernel-binary.

## Roadmap

См. [`PROGRESS.md`](PROGRESS.md).
