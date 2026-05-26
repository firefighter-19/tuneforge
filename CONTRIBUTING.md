# Contributing to tuneforge

Thanks for considering a contribution. tuneforge is a hobby project that
became a portfolio piece — issues, PRs, and "tested on my car" reports are
all very welcome.

## Scope

**In scope:**
- Bug fixes for existing features (editor, logger, dump-rom, DTC tooling)
- New parser/decoder/logger improvements for **Subaru** ECUs
- Quality-of-life UX improvements in CLI / GUI
- More test coverage (especially fixture-driven integration tests)
- Documentation, examples, screenshots
- ECU compatibility reports (see "Hardware compatibility" below)
- Per-platform polish (Linux build is a wide-open opportunity)

**Out of scope for now (PRs will likely be deferred or closed):**
- **Flash-write to ECU.** Explicit non-goal until a donor ECU is available
  for testing. The brick risk on a single-car project is too high.
- **Non-Subaru ECU support that requires hardware testing I can't do.**
  Skeleton code for other vendors (`ds2.rs`, `ncs.rs`) is fine; PRs that
  claim "works on BMW" without an attached log capture are not.
- **Large refactors without a prior issue discussion.** Open an issue
  first — saves both of us time.

## Quick start

```bash
git clone https://github.com/firefighter-19/tuneforge && cd tuneforge

# macOS prereqs:
brew install libusb pkg-config

# Linux prereqs (Ubuntu/Debian):
sudo apt-get install libusb-1.0-0-dev libudev-dev pkg-config \
    libxkbcommon-dev libwayland-dev libxcb1-dev libgl1-mesa-dev

# Build + test:
cargo build --workspace
cargo test  --workspace                                   # 220+ tests, ~10 s

# Full features (includes GPL-3 kernel-upload + GUI ECU-tools panel):
cargo build --workspace --features tuneforge-cli/kernel-upload,tuneforge-gui/ecu-tools

# Lint gates (CI enforces these):
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

## Conventions

### Commit messages

Conventional Commits (`type: subject`):

```
feat: add freeze-frame support over OBD-II Mode 02
fix: handle truncated multi-frame ISO-TP responses in logger-can
docs: clarify sudo requirement for Tactrix on macOS
refactor: extract dump-rom phases into orchestrator crate
chore: bump version to 0.4.0
test: add fixture-driven roundtrip for log_defs.xml parser
```

Types we use: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`, `ci`.
This matters because `cargo-dist` derives release notes from these.

### Code style

- `cargo fmt` is the source of truth. CI rejects unformatted code.
- `cargo clippy -- -D warnings` is the lint gate. Workspace-level
  `[workspace.lints.clippy]` in `Cargo.toml` documents which pedantic
  lints we intentionally allow and why — read that before adding new
  `#[allow(...)]` annotations to your code.
- Comments **in Russian or English are both fine** at the source level.
  User-facing strings (anything rendered in the GUI / printed by the CLI)
  must be English — localization is planned but not implemented.
- Doc comments on public items: encouraged. `cargo doc --no-deps` should
  produce something useful.

### Testing

- Unit tests live next to the code (`#[cfg(test)] mod tests`).
- Protocol-level integration tests use `tuneforge_io::mock::MockTransport`
  — pre-queue the byte sequences the "device" would return, run your
  code, assert on the writes.
- ROM tests use the committed fixture
  `crates/tuneforge-rom/tests/fixtures/forester-xt-2007-4E42504007.bin`
  (a known-good 1 MiB dump from a 2007 USDM Forester XT).
- Anything that needs real hardware: gate with `#[ignore]` and document
  how to run it in a comment.

### License isolation

The workspace ships under **GPL-2.0+** by default. The `tuneforge-kernel`
crate is **GPL-3.0+** because it derives from GPL-3 upstreams
([`fenugrec/nisprog`](https://github.com/fenugrec/nisprog),
[`fenugrec/npkern`](https://github.com/fenugrec/npkern)). It's pulled in
only when you opt in via `--features kernel-upload` (CLI) /
`--features ecu-tools` (GUI).

If your contribution:
- **Modifies `crates/tuneforge-kernel/`** → it's GPL-3.0+ work, same as
  the rest of that crate. Don't copy GPL-2-only code into it.
- **Modifies any other crate** → keep it GPL-2.0+ compatible. Don't copy
  GPL-3 code (e.g. from npkern) into non-kernel crates.

If you're not sure where your change belongs, ask in the issue.

## Hardware compatibility reports

If you tested tuneforge on a real Subaru, please file an issue using the
**ECU compatibility report** template. Even a "doesn't work, here's the
log" is valuable — it builds a compatibility matrix we can publish.

Minimum useful info:
- Vehicle: year, market (USDM/EDM/JDM/AUDM), trim, transmission
- ECU: ROM ID (5-byte ASCII, shown by `tuneforge ssm-init --tactrix`)
- Cable: Tactrix Openport 2.0 (or other? we haven't tested others)
- OS: macOS version + chip (Apple Silicon / Intel)
- What you ran + outcome (success / hex of failed frame / Wireshark
  capture if you have one)

## Pull-request checklist

Before opening a PR:

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes (default features)
- [ ] `cargo test --workspace --features tuneforge-cli/kernel-upload,tuneforge-gui/ecu-tools` passes
- [ ] New behavior has a test (preferably MockTransport-driven if it's
      protocol code)
- [ ] If you touched the CLI, `tuneforge --help` and `tuneforge <cmd> --help`
      still read sensibly
- [ ] If your change is user-visible, a line in `CHANGELOG.md` under
      `## [Unreleased]`

Keep PRs focused — one feature or fix per PR. CI runs Build + Lint
(required) and macOS-build (advisory but very fast).

## Where to ask

- **Project-specific questions / bug reports / feature requests:** open
  a GitHub issue.
- **Quick "is this approach sane?" questions on a draft PR:** open a
  draft PR and ask in the description.

## Code of conduct

Be kind, don't be a jerk. Standard
[Contributor Covenant](https://www.contributor-covenant.org/) applies in
spirit even though we haven't formalized it. If something feels off,
email or open a private issue.

## A note on AI-assisted contributions

Using LLM tools (Claude Code, Copilot, Cursor, etc.) to help write code
is fine — most of this codebase was written with LLM assistance. But:

- **Read the code you're submitting.** If you can't explain why it
  works, neither of us can review it.
- **No mass auto-generated PRs.** A PR that touches 30 files with
  cosmetic changes is going to be closed.
- **Tests still matter.** "LLM said it would work" is not test coverage.
