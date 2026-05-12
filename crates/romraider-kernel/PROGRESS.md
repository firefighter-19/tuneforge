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

Crate **создан как skeleton 2026-05-12**, реализация по фазам:

| Фаза | Модуль                    | Что делается                              | Статус |
|------|---------------------------|-------------------------------------------|:------:|
| 0    | reference setup           | `nisprog` + `npkern` склонированы локально, kernel-binary извлечён в `kernels/` | 🟡 в работе (агент) |
| 1    | `kwp2000`                 | frame builder/parser + NRC + SID-константы | 🟡 константы есть, builder/parser TODO |
| 2    | `seed_key`                | 16-round Feistel + KEYTOGEN/SBOX tables   |   ❌   |
| 3    | `upload`                  | SID 0x81/0x10/0x27/0x34/0x36/0x31 sequence |   ❌   |
| 4    | `kernel_wire`             | SID_DUMP / SID_TP / SID_CONF / SID_RESET  |   ❌   |
| 5    | `dump::dump_rom_via_kernel` | high-level orchestrator                |   ❌   |
| 6    | live-verify               | dump vs `fixtures/forester-xt-2007-4E42504007.bin` byte-by-byte | ❌ |

## Roadmap (Slice 21)

См. также корневой `PROGRESS.md` (`Slice 21` строка в таблице).

**Day 1** — kwp2000 framing + seed/key:
- Реализовать `kwp2000::build_request(sid, &[u8]) -> Vec<u8>` + tests
- Реализовать `kwp2000::parse_response(&[u8]) -> Result<Frame, _>` + NRC-detect
- Порт `sub_genkey()` в `seed_key::compute_key(u32) -> u32`
- Заполнить `KEYTOGEN_INDEX[16]` и `INDEX_TRANSFORMATION[32]` из nisprog
- Test-vectors: 3-5 known seed/key пар (из nisprog или сами сгенерируем на mock-Subaru)

**Day 2** — upload sequence (используем mock-Transport, потом TactrixTransport):
- `start_communication` (SID 0x81)
- `start_diagnostic_session` (SID 0x10, sub `0x85`/`0x86` — уточнить)
- `security_access(compute_key)` (SID 0x27) — request seed, ECU response, send key
- `request_download(addr, len)` (SID 0x34) — формат `34 AH AM AL 04 LH LM LL`
- `transfer_data` (SID 0x36) — 128-байт чанки + encrypt (4-round Feistel?)
- `start_routine(0x01_01)` (SID 0x31) — handover

**Day 3** — kernel-side wire + verify:
- `kernel_wire::dump_block(addr, len)` — SID_DUMP
- `kernel_wire::switch_baud(new_baud)` (optional, для скорости)
- `dump::dump_rom_via_kernel()` orchestrator
- CLI: `dump-rom-kernel --tactrix --mcu sh7058 --output /tmp/dump.bin`
- Verify: `cmp dump.bin fixtures/forester-xt-2007-4E42504007.bin` → должен быть exit 0

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
