# Changelog

All notable changes to tuneforge are documented here.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [SemVer](https://semver.org/) — minor bumps are tied to
slice-clusters during 0.x; the project is **pre-1.0** so any release may
include breaking changes.

## [Unreleased]

_Nothing yet — add items here as you land them._

## [0.3.0] — 2026-05-25

### Changed

- **Rebranded from `romraider-rs` to `tuneforge`.** The workspace
  outgrew "Java port" framing — Mac-native Tactrix USB stack,
  kernel-upload over CAN, SSM3-CAN abstraction, egui GUI are all
  original work; only the XML def format stays RomRaider-compatible.
  All 9 crates renamed (`romraider-* → tuneforge-*`), binary names
  updated (`romraider → tuneforge`, `romraider-gui → tuneforge-gui`),
  ~300 `use romraider_*` imports rewritten across the workspace.
- **Coalesced undo** in the ROM editor — a single drag is now one undo
  step instead of one per intermediate frame (~60/sec while dragging).
- **Cell tooltip** in the ROM editor now includes the table description
  from `ecu_defs.xml` (`<table description="…">`).
- **Pre-built binaries via `cargo-dist`** — `.github/workflows/release.yml`
  builds macOS ARM + x86 binaries on tag push, ships shell installer.
- **README polish** — Install / Quick start / Screenshots / Why I built
  this sections; install instructions cover both pre-built binaries
  (`curl | sh`) and `cargo install --git` from source.

## [0.2.0] — 2026-05-25

### Added

- **Unified `EcuClient` trait** (`tuneforge-protocol::client`) abstracting
  the protocol choice between K-Line SSM2 and SSM3 over CAN. CLI now
  exposes `--protocol auto|kline|can` on `ssm-init` and `dump-rom` with
  auto-detect probing through Tactrix.

### Changed

- Removed duplicate K-Line dump-rom handler
  (`dump_rom_tactrix_continuous`, `do_dump_rom`) — both paths now go
  through the trait.

## [0.1.0] — 2026-05-24

Initial public version. Closed Slices 1–33 of the original roadmap:

### Added

- **ROM editor** (egui-based GUI): open `.bin` + `ecu_defs.xml`, edit
  tables against scaling formulas, auto-fix Subaru classic checksum,
  Save As, Undo/Redo, ROM compare with diff coloring, heatmap, cell
  tooltips, Changes-since-open summary.
- **Live logger**: SSM2 K-Line + SSM3-CAN, XY-plot in GUI, CSV
  datalogging with auto-timestamped paths.
- **Headless CLI**: `ssm-init`, `dump-rom`, `dump-rom-can`,
  `dump-rom-kernel`, `peek-vin`, `peek-cvn`, `peek-rom`, `peek-ram-can`,
  `peek-seed`, `dtc-can`, `freeze-frame-can`, `logger`, `logger-can`,
  `logger-ssm-can`, `inspect-def`, `inspect-rom`, `inspect-log`,
  `read-table`, `tactrix-info`, `ports`.
- **Mac-native Tactrix USB transport** (`tuneforge-io::tactrix`) via
  `rusb` — no J2534 DLL anywhere; supports both ISO9141 (K-Line) and
  ISO15765 (CAN @ 500 kbps).
- **Subaru SecurityAccess seed/key** (`tuneforge-kernel::seed_key`) —
  Feistel cipher with firmware-resident round-keys extracted from
  Wireshark captures of EcuFlash sessions.
- **Full UDS-over-CAN ROM dump** via kernel-upload — produces
  byte-identical output to EcuFlash (~44 s for 1 MiB on a 2007 Forester
  XT). Kernel-upload code isolated under GPL-3.0+ in `tuneforge-kernel`
  crate, gated behind `--features kernel-upload`.
- **DTC tooling** — OBD-II Mode 03 (stored) + 07 (pending) + 0A
  (permanent), Mode 04 clear, 324 Subaru-specific DTC descriptions
  extracted from upstream `ecu_defs.xml`.
- **Freeze Frame** (Mode 02) snapshot at the triggering DTC moment.
- **GitHub Actions CI** — `build-and-test` (Ubuntu + macOS), `lint`
  (clippy `-D warnings` + rustfmt). Pre-built binaries via cargo-dist
  added in v0.3.0.

[Unreleased]: https://github.com/firefighter-19/tuneforge/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/firefighter-19/tuneforge/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/firefighter-19/tuneforge/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/firefighter-19/tuneforge/releases/tag/v0.1.0
