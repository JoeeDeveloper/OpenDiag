# CLAUDE.md

Guidance for Claude Code in this repo. Full roadmap: `PROJECT_PLAN.md`.

OpenDiag is an open source, cross-platform diagnostic suite for VAG group vehicles (VW, Audi, Seat, Skoda), a VCDS alternative built on commodity hardware. Stack: Rust workspace (`crates/`), later Tauri 2 + React (`app/`). Licence: GPL-3.0-or-later.

## Ground rules (non-negotiable)

1. **Clean-room only.** Never copy, scrape or ship Ross-Tech label files (.lbl), VCDS binaries, ODIS data or any proprietary dataset. All module/label/coding data is community-authored in our own open format, with a provenance field.
2. **Sources.** Protocol knowledge comes from public specs (ISO 14230 KWP2000, ISO 15765 ISO-TP, ISO 14229 UDS, ISO 9141, public KW1281/TP2.0 docs, open source projects such as KW1281Test). Record every source in `docs/sources.md`.
3. **Safety first.** v1 is read-only by default (`WriteMode::ReadOnly`). Any write (coding, adaptation, output tests, basic settings, clearing DTCs) needs an explicit "unlock writes" mode, a mandatory durable backup/snapshot taken before anything is sent, a confirmation bound to the exact change shown (no `--yes` bypass), and read-back verification. New writes must go through the guarded flow in `opendiag-services` (see `docs/safety.md`): module-level write functions require a `WriteToken` that only that flow can create. `crates/opendiag-services/tests/write_safety.rs` enforces "no write without a durable backup" under corrupted traffic; any new write path must be added to it. No flashing or ECU reprogramming.
4. **Security access** is a pluggable algorithm interface only. Ship algorithms only where publicly documented. Never add anything that bypasses immobiliser, odometer or tuning protection.
5. **Testable without a car.** Every protocol layer needs a simulator and capture-replay tests. Never make a merge-blocking test depend on hardware.
6. **Small commits**, conventional commit messages, CI on every PR (fmt, clippy, test).

## Architecture

```
CLI / GUI / (future) HTTP API
  opendiag-services  (DTCs, measuring blocks, ident, coding, output tests)
  opendiag-proto     (KW1281, KWP2000, UDS, TP2.0, ISO-TP)
  opendiag-transport (SocketCAN, J2534, K-line, ELM327, Simulator, Replay)
  opendiag-data      (module/DTC/label/coding DB, open format)
  opendiag-core      (shared types, errors)
```

- `Transport` trait: `send`, `recv(timeout)`, `set_baud`, `slow_init`, `capabilities()`.
- Protocols are state machines with an injected clock so tests are deterministic.
- ELM327 is a degraded generic OBD-II transport only; don't promise full VAG diagnostics on it.
- Linux is the reference platform; Windows needs FTDI latency and 5-baud init care.

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p opendiag-cli -- --version
```

## Build and publish

This dev box has no Windows/macOS cross-compile toolchain (and no sudo to install one), so a local
`cargo build --release` only produces a Linux binary -- not something Joe's Windows desktop can run from the
`builds` share. Real distributable builds come from CI, via `.github/workflows/release.yml` (triggered by
pushing a `v*` tag), which cross-compiles for Windows (msvc), Linux (musl) and macOS (x86_64 + aarch64) and
attaches them to a GitHub Release. To publish one to `/home/jga/builds/OpenDiag/`:

```bash
scripts/publish_release.sh [tag]   # defaults to the latest release; verifies the SHA256SUMS first
```

It extracts the Windows build (ready to run, `.exe` already executable) into `<short-hash-of-the-tag>/` and
`latest/`, and keeps the other platforms' archives alongside it in `other-platforms/`. Windows path:
`N:\OpenDiag\latest\opendiag.exe`.

## Workflow

Before each phase, produce a short plan and acceptance tests, then implement.

Status: phases 0-2 are largely done (transports, ISO-TP, UDS, KWP2000, KW1281, serial and SocketCAN drivers). Phase 3 (read-only MVP) is in progress: the CLI works against a virtual ECU, and `probe` can talk to a real K-line module. BMW F-series (ENET/HSFZ, `opendiag-bmw`, see `docs/bmw.md`): step 1 read-only, simulator-tested only. Never bundle BMW proprietary data. Done and simulator-tested: TP2.0, KWP2000/KW1281 scan and DTCs, KW1281 live-data logging, `--replay` of recorded traces (K-line, CAN and BMW HSFZ), the `serve` dashboard, the open data format v0, raw `log --ids` over KWP2000/UDS. Not done: decoded KWP2000 measuring blocks (no public source), GUI. Nothing has been verified against a real car yet.
