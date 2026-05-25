# tuneforge

[![CI](https://github.com/firefighter-19/tuneforge/actions/workflows/ci.yml/badge.svg)](https://github.com/firefighter-19/tuneforge/actions/workflows/ci.yml)
[![License: GPL-2.0+](https://img.shields.io/badge/License-GPL--2.0%2B-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org/)

**Languages:** **English** · [Русский](README.ru.md)

**Mac-native Rust toolkit for Subaru ECU tuning: ROM editor + datalogger +
ROM dump over Tactrix Openport 2.0, all in a single binary.**

Combines two things that have historically required a Windows VM on Mac:

- **Editor + logger in the spirit of [RomRaider](https://github.com/RomRaider/RomRaider)**
  (a Java app that's broken on modern 64-bit OSes) — open a `.bin`, edit
  tables against `ecu_defs.xml`, auto-fix checksums, view/diff ROMs, and
  record SSM2 datalogs.
- **ROM dump in the spirit of [EcuFlash](https://www.tactrix.com/index.php?Itemid=58)**
  (Tactrix freeware, also Windows-only) — pull a full 1 MiB image from the
  ECU via kernel upload over UDS-over-CAN. Implemented entirely in Rust, with
  no J2534 DLL and no emulation layer.

> **This is derivative work.** RomRaider is an open-source community project
> from RomRaider.com (since 2006), licensed under GNU GPL v2.0+. EcuFlash is
> freeware from Tactrix; we do not use its code, but we functionally
> reproduce its dump flow (UDS sequence, seed/key, encrypted kernel) by
> reverse-engineering Wireshark captures of EcuFlash sessions. This
> repository is distributed under GPL v2.0+; the `tuneforge-kernel` crate is
> isolated under GPL v3.0+. See [Origin and license](#origin-and-license).

## What works today

Verified on a live car (2007 USDM Subaru Forester XT, ROM `4E42504007`,
Tactrix Openport 2.0 on Mac ARM):

- ✅ **SSM2 ECU init over K-Line** via `tuneforge-cli ssm-init --tactrix`
- ✅ **GUI editor**: open `forester-xt-2007-4E42504007.bin` + `ecu_defs.xml`,
  edit Target Boost / Wastegate Duty, auto-fix checksum, Save As; ROM diff
  with heatmap, undo/redo, Changes-since-open, cell tooltips with raw/Δ
- ✅ **Headless inspection** of ROMs and definitions (`inspect-def`, `read-table`)
- ✅ **SSM logger** with `log_defs.xml` resolution and a live XY plot in the GUI
- ✅ **Full 1 MiB ROM dump over UDS-over-CAN** in ~44 s — byte-for-byte
  identical to the EcuFlash dump:
  ```bash
  sudo cargo run -p tuneforge-cli --features kernel-upload -- \
      dump-rom-can --output /tmp/forester-mac-dump.bin
  ```

## What it does **not** do (and won't, for now)

- ❌ **Flash writes back to the ECU.** This is an explicit non-goal. The
  author has only one car and no donor ECU; brick risk outweighs benefit. If
  this ever changes, it will only happen on a donor first. Kernel upload
  writes **to RAM only** — no flash erase/program.
- ❌ **Non-Subaru ECUs.** The reference is Java RomRaider; the test platform
  is Subaru.
- ❌ **Windows-only paths** (J2534 DLL emulation, Wine, x86 USB passthrough)
  — the goal is specifically a Mac-native pipeline.

See [`PROGRESS.md`](PROGRESS.md) for the full picture and slice-by-slice roadmap.

## Workspace layout

| Crate                | Purpose                                                                  |
| -------------------- | ------------------------------------------------------------------------ |
| `tuneforge-core`     | Shared types: addresses, endianness, errors, byte utilities              |
| `tuneforge-io`       | Transports: serial, J2534, ELM327, **Tactrix Openport 2.0 (rusb, K-Line + CAN)** |
| `tuneforge-protocol` | Dialects: SSM, OBD-II, DS2, NCS, RamTune                                 |
| `tuneforge-defs`     | Parser for ECU and logger XML definitions                                |
| `tuneforge-rom`      | ROM image, 1D/2D/3D tables, scaling/formulas, checksum patcher           |
| `tuneforge-logger`   | Logger backend, subscriptions, datalog files, external sensors           |
| `tuneforge-cli`      | Headless CLI — debug, smoke-test, **dump-rom, ssm-init, peek-vin**, etc. |
| `tuneforge-gui`      | GUI on `eframe`/`egui` (editor + logger + diff/heatmap/undo/changes)     |
| `tuneforge-kernel`   | (opt-in, GPL-3.0) Subaru SH7058 kernel upload + UDS-over-CAN dump flow   |

## Build

```bash
cargo build --workspace                            # without kernel-upload
cargo test  --workspace                            # ~175 tests
cargo run   -p tuneforge-cli -- --help             # headless commands
cargo run   -p tuneforge-gui                       # editor + logger
```

The **ROM dump via Tactrix** requires an opt-in feature and `sudo` (for
libusb bulk I/O on macOS):

```bash
sudo cargo run -p tuneforge-cli --features kernel-upload -- \
    dump-rom-can --output ./my-dump.bin
```

`--features kernel-upload` enables the `tuneforge-kernel` crate (GPL-3.0)
inside the CLI. It is disabled by default — the workspace stays under
GPL-2.0+ when this code is not used.

## Compatibility with the original

* The upstream ECU XML definitions (`definitions/`, `customize/`, `i18n/`)
  **are not modified** and are consumed as-is — this is the most valuable
  part of the original project, accumulated by the community over 15+ years.
* Protocol logic is checked against `com.romraider.io.protocol.*` from
  upstream and verified against captured communication dumps.
* Checksum algorithms are copied from `com.romraider.maps.checksum.*`
  one-to-one and covered by unit tests.

## Roadmap

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Origin and license

This project is derivative work based on
[RomRaider](https://github.com/RomRaider/RomRaider). Copyright in the
original code belongs to the RomRaider.com developers
(`Copyright (C) 2006-2022 RomRaider.com` — see headers in upstream sources).
Distribution terms match the original: **GNU GPL version 2 or later**.

**The `tuneforge-kernel` crate** (kernel upload and UDS dump) is licensed
separately under **GPL v3.0+**, because it builds on work from GPL-3
projects ([`fenugrec/nisprog`](https://github.com/fenugrec/nisprog),
[`fenugrec/npkern`](https://github.com/fenugrec/npkern)). The crate is
gated behind the `kernel-upload` feature flag; the remaining crates stay
under GPL-2.0+.

**About EcuFlash:** we do not use EcuFlash code (EcuFlash is Tactrix
freeware, not open source). The dump flow was implemented independently
through reverse engineering of Wireshark captures of EcuFlash sessions,
combined with public resources (nisprog/npkern and
[james-portman's work](https://github.com/james-portman/subaru-ecu-flashing)).
The encrypted kernel binary we upload into the ECU's RAM originally comes
from OpenECU/Tactrix (`OpenECU Subaru SH7058 OCP CAN Kernel V1.07`); it is
reused as-is from a capture because the contract between the ECU and the
kernel is hardcoded in the bootloader. If the rights holder considers this
a problem — please open an issue and we will remove the binary and leave
only instructions on "extract your own".

The full license text is in [`LICENSE`](LICENSE).

If you redistribute upstream RomRaider definitions, variable dictionaries,
or checksum algorithms, keep the original attribution and license terms
intact.

## Safety and disclaimer

This tool **reads** firmware and edits **files on disk**. It does **not
write anything back to the ECU** — flash write is not implemented and
won't be without a donor ECU for testing. Even so, when tuning an ECU any
mistake can lead to unstable engine operation or engine damage. Use at
your own risk; the authors accept no responsibility for damage to hardware
or engines.
