# romraider-kernel — Progress

## Цель

Дамп ROM с Subaru SH7055/SH7058 ECU через **kernel-upload**: заливаем RAM-
resident kernel-binary (от `fenugrec/npkern`) и общаемся с ним по его
собственному wire-протоколу. Это единственный надёжный путь снять полную
прошивку — прямой `SSM2 ReadBlock` (0xA0) на этих ECU блокируется
анти-fuzz защитой и возвращает stub-`0xFF`.

## Эталон

- [`fenugrec/nisprog`](https://github.com/fenugrec/nisprog) — C-реализация
  upload-sequence + seed/key Feistel (GPL-3.0)
- [`fenugrec/npkern`](https://github.com/fenugrec/npkern) — RAM-loader kernel,
  прекомпилированные `.bin` в `precompiled/` (GPL-3.0)
- [`miikasyvanen/FastECU`](https://github.com/miikasyvanen/FastECU) — C++
  cross-check для seed/key констант
- [`james-portman/subaru-ecu-flashing/encryption.py`](https://github.com/james-portman/subaru-ecu-flashing/blob/master/encryption.py)
  — Python cross-check

## Статус

✅ **Slice 23 закрыт 2026-05-13**: Mac-native ROM dump через UDS/ISO15765 работает
end-to-end, SHA-256 дампа совпал byte-for-byte с EcuFlash-fixture-ом.

| Фаза | Модуль                    | Что делается                              | Статус |
|------|---------------------------|-------------------------------------------|:------:|
| 0    | reference setup           | `nisprog` + `npkern` склонированы; encrypted CAN kernel V1.07 (8 КБ) в `kernels/openecu_can_v1.07_encrypted.bin` извлечён из EcuFlash capture | ✅ |
| 1    | `kwp2000`                 | frame builder/parser + NRC + SID-константы | ✅ |
| 2    | `seed_key` (K-Line)       | 16-round Feistel + `KEYTABLE_GENKEY` (универсальная) |  ✅  |
| 2b   | `seed_key` (CAN)          | тот же Feistel + firmware-specific `KEYTABLE_GENKEY_CAN` (round-keys из ROM `0x05972C`), верификация 8 live-парами | ✅ |
| 3    | `upload` (K-Line)         | SID 0x81/0x10/0x27/0x34/0x36/0x31 sequence | 🟢 готов, но **не применим** к 2007 USDM Forester XT — ECU на этом авто использует UDS-on-CAN |
| 3b   | `can_upload`              | UDS-on-CAN: `10 03` → `27 01/02` → `10 02` → `34 04 33` → 64× `B6` → `37` → `31 01 02 02 02` | ✅ |
| 4    | `kernel_wire` (K-Line)    | SID_DUMP / SID_TP / SID_CONF / SID_RESET  | ❌ (не реализован, K-Line не применим к этой машине) |
| 4b   | `can_wire`                | 1-byte protocol: `01`/`81` handshake, `03`/`83` read_memory @ 2 КБ chunks | ✅ |
| 5    | `dump-rom-can` CLI        | orchestrator: OBD-II wake → ExtSession → SecAccess → ProgSession → upload+jump → kernel handshake → 512× read_memory | ✅ |
| 6    | live-verify               | 1 МБ дамп за 43.7 с, SHA-256 = `3016ce24…`, ✅ совпал с `fixtures/forester-xt-2007-4E42504007.bin` byte-by-byte | ✅ |

## Что осталось / next slices

- **Slice 24** — generic protocol abstraction: один CLI-entry `dump-rom`, который
  сам выбирает K-Line или CAN по ROM-ID / capability-байтам. Сейчас две команды
  (`dump-rom` и `dump-rom-can`) — пользователь выбирает руками.
- **Other ECUs**: round-key table вытащена для одного firmware (ROM `4E42504007`).
  Для других ROM-ID понадобится либо capture seed/key пар + brute-force, либо
  паттерн-поиск S-box `[05 06 07 01 09 0C ...]` + 16-word table рядом.
- **K-Line kernel-upload для машин 2002–2005** (где ECU действительно отвечает
  на KWP2000 K-Line) — `upload.rs` готов, нужна только тест-машина.

## Лицензирование

- Этот крейт: **GPL-3.0-or-later** (`Cargo.toml::license`).
- Kernel binaries в `kernels/` — **GPL-3.0** (как в `npkern/`).
- Workspace главный лицензирован как `GPL-2.0-or-later`; добавление GPL-3.0
  крейта не «заражает» MIT-/GPL-2-only-крейты, потому что они на этот не зависят.
- В `romraider-cli` подключение этого крейта — **под feature-flag `kernel-upload`**,
  чтобы пользователь явно opt-in выбирал «зальём kernel в RAM ECU».

## Безопасность

- **Только RAM-writes** (адреса `0xFFFFxxxx`). Никакого flash-write, никакого
  риска брика ECU при штатной работе кода.
- Single-failure лимит ECU: после **2 неудачных seed/key** ECU блокирует
  security access на **10 секунд** (NRC 0x37) — это лечится ожиданием.
- Сам по себе kernel-upload не пишет в flash. Чтобы реально что-то залить,
  нужно отправить `SID_FLREQ` + `SID_FLASH` — этих команд в крейте нет.
