# OpenDiag - Open Source VAG Diagnostic Tool (VCDS alternative)

Working name: OpenDiag (change freely). Goal: a modern, cross-platform, open source diagnostic suite for VAG group vehicles (VW, Audi, Seat, Skoda), covering the day-to-day VCDS feature set, built on commodity hardware.

## 0. Ground rules for Claude Code

- Clean-room only. Do NOT copy, scrape or ship Ross-Tech label files (.lbl), VCDS binaries, ODIS data or any proprietary datasets. All module/label/coding data is community-authored in our own open format.
- Protocol knowledge comes from public specs (ISO 14230 KWP2000, ISO 15765 ISO-TP, ISO 14229 UDS, ISO 9141, public KW1281/TP2.0 documentation and open source projects such as KW1281Test). Document every source in `docs/sources.md`.
- Safety first: v1 is read-only by default. Any write (coding, adaptation, output tests, basic settings) sits behind an explicit "unlock writes" mode, mandatory backup of the previous value, and a confirmation step. No flashing or ECU reprogramming in scope.
- Seed/key security access: pluggable algorithm interface only. Ship algorithms only where publicly documented. Do not ship anything that bypasses immobiliser, odometer or tuning protection.
- Every protocol layer must be testable without a car (simulator + recorded capture replay). No hardware, no merge-blocking tests.
- Small commits, conventional commit messages, CI on every PR (fmt, lint, test).

## 1. Tech stack (proposed)

| Layer | Choice | Why |
|---|---|---|
| Core library | Rust (`opendiag-core`) | Timing-critical protocols, memory safe, one codebase for CLI/GUI/embedded-friendly |
| Transport drivers | Rust crates behind a `Transport` trait | Swap hardware without touching protocol code |
| CLI | Rust (`clap`) | Scriptable, first thing to ship, great for testing |
| GUI | Tauri 2 + React/TypeScript | Modern UI, small binaries, Win/Linux/macOS |
| Data/labels | YAML or TOML, JSON Schema validated | Easy community PRs |
| Logging | Parquet or SQLite + CSV export | Graphing, post-processing |
| Docs | mdBook | Protocol docs live in repo |

Alternative if you want faster iteration: Python core (python-can, udsoncan, pyserial) with the same architecture, then port hot paths later. Decide in Phase 0.

## 2. Hardware strategy

Target commodity interfaces, no custom proprietary dongle required.

- CAN: SocketCAN adapters (CANable / candleLight / any gs_usb), J2534 pass-thru devices (e.g. Tactrix OpenPort 2.0), PEAK PCAN via its API.
- K-line (KW1281 / KWP2000 on K-line, older cars): FTDI-based K-line cable with proper 5-baud slow-init handling, or custom ESP32/STM32 firmware with an L9637D transceiver (stretch goal, `firmware/` dir).
- ELM327: supported as a degraded "generic OBD-II only" transport. It cannot do VAG TP2.0 or reliable K-line timing, so do not promise full diagnostics on it. Document this clearly.
- Windows needs care with FTDI latency timer and 5-baud init. Linux is the reference platform for development.

## 3. Architecture

```
GUI (Tauri) / CLI / (future) HTTP API
        |
   opendiag-session   (vehicle scan, module sessions, keep-alive, error recovery)
        |
   opendiag-services  (read DTCs, clear DTCs, measuring blocks, ident, coding, adaptation, output tests)
        |
   opendiag-proto     (KW1281, KWP2000, UDS, TP2.0, ISO-TP)
        |
   opendiag-transport (SocketCAN, J2534, K-line serial, ELM327, Simulator, Replay)
        |
   opendiag-data      (module DB, DTC DB, measuring block labels, coding definitions - open format)
```

Key design points:
- `Transport` trait: `send`, `recv(timeout)`, `set_baud`, `slow_init`, `capabilities()`.
- Protocols are state machines with injected clock, so unit tests are deterministic.
- Auto-detect protocol per module (try UDS, then KWP2000/TP2.0, then KW1281) with a cache per vehicle profile.
- Async runtime (tokio) with strict per-module session locks and TesterPresent keep-alive.

## 4. Phases and milestones

### Phase 0 - Foundations (week 1)
- Repo scaffold, licence (Apache-2.0 or GPL-3.0 - decide; GPL protects against closed forks), CODE_OF_CONDUCT, CONTRIBUTING, CI.
- `docs/sources.md`, `docs/protocols/` skeletons, `docs/legal.md` (clean-room policy).
- Decide Rust vs Python. Spike: send a raw ISO-TP frame via SocketCAN loopback.
- Acceptance: CI green, `opendiag --version` runs, docs build.

### Phase 1 - Transport + simulator (weeks 2-3)
- `Transport` trait, SocketCAN driver, serial K-line driver, **Simulator** transport (virtual ECUs with scriptable responses), **Replay** transport (feeds candump/serial logs).
- Capture tooling: `opendiag capture` writes raw bus/serial logs for building test fixtures.
- Acceptance: loopback tests pass in CI with no hardware; replay of a sample log reproduces expected frames.

### Phase 2 - Protocol stack (weeks 4-7)
- ISO-TP (ISO 15765-2) full: segmentation, flow control, STmin/BS.
- UDS (ISO 14229): sessions, ReadDataByIdentifier, ReadDTCInformation, ClearDiagnosticInformation, TesterPresent, SecurityAccess (pluggable), WriteDataByIdentifier, RoutineControl.
- KWP2000 over CAN (TP2.0) and over K-line; KW1281 (slow init 5-baud, block protocol, ack handling, group readings).
- Property tests / fuzzing on parsers. Timing tests with fake clock.
- Acceptance: simulator ECUs for each protocol respond correctly; fuzzers find no panics.

### Phase 3 - Read-only diagnostics MVP (weeks 8-10) - CLI first
- Auto-scan: enumerate modules (by address list per platform), read ident (part number, software version, coding, shop code, VIN).
- Read DTCs with status bytes (stored/pending/intermittent), freeze frame where supported.
- Live data / measuring blocks, logged to file, CSV export.
- Clear DTCs (the one "write" allowed in MVP, behind confirmation).
- Open data format v0: module list, DTC descriptions, measuring block definitions. JSON Schema + validator + contribution guide.
- Acceptance: on a real car (test vehicle: 9N3 Polo GTI, plus a late-model UDS car if available), full scan + DTC read matches known-good output.

### Phase 4 - GUI (weeks 11-14)
- Tauri app: vehicle scan screen, module list, DTC view, live data with real-time graphs (uPlot or similar), log record/playback, dark mode.
- Connection wizard with adapter detection and diagnostics ("is my cable working?").
- Acceptance: non-developer can connect, scan, read and export a report (PDF/HTML) without touching the CLI.

### Phase 5 - Advanced functions, gated (weeks 15-20)
- Long coding / short coding: bitfield editor with per-byte/bit labels from the open data set, mandatory backup + diff + confirm.
- Adaptation channels read/write, basic settings, output tests (actuator tests) with safety interlocks (engine off, speed zero where required).
- Login/security access via pluggable algorithms (documented ones only).
- Coding profiles: save/restore full module coding per vehicle.
- Acceptance: round-trip read, write, verify, restore on simulator and on a sacrificial/non-critical module on a real car.

### Phase 6 - Community and polish (ongoing)
- Data contribution pipeline: PR templates, CI that validates data files, "captured on" provenance field.
- Vehicle profiles per platform (PQ24/PQ35/PQ46, MQB, MLB, etc).
- Plugins (WASM or scripting) for custom tests.
- Packaging: Windows installer, AppImage/deb, Homebrew. Signed releases.
- Optional: ESP32/STM32 firmware adapter, headless "diag server" on a Pi/homelab with web UI.

## 5. Repo layout

```
opendiag/
  crates/
    opendiag-core/
    opendiag-transport/
    opendiag-proto/
    opendiag-services/
    opendiag-data/
    opendiag-cli/
  app/                 # Tauri + React
  data/                # open module/DTC/coding definitions (YAML) + schema
  firmware/            # optional adapter firmware
  docs/
  tests/fixtures/      # captures, simulator scripts
  CLAUDE.md
```

## 6. Testing strategy

- Unit: protocol state machines with fake clock.
- Integration: simulator ECUs (scripted), replay of real captures.
- Hardware-in-loop (manual, documented checklist): scan + DTC + live data on real cars before each release.
- Data validation in CI: schema checks, duplicate address/ID detection.
- Fuzz: ISO-TP reassembly, KWP/UDS parsers.

## 7. Risks

| Risk | Mitigation |
|---|---|
| Bricking modules via bad coding write | Read-only default, backup, diff, confirm, simulator-tested write paths |
| Legal exposure from proprietary data | Clean-room policy, provenance on every data file, no Ross-Tech/ODIS content |
| Timing issues on K-line / Windows USB latency | Linux reference, FTDI latency tuning docs, optional MCU adapter |
| Data coverage (labels) is the real moat | Make contributing trivial, capture tools, schema + CI, prioritise common platforms first |
| Scope creep into tuning/flashing | Explicitly out of scope in v1 |

## 8. Definition of done for v1.0

- Full scan, DTC read/clear, live data, logging on PQ-era (KWP/KW1281/TP2.0) and UDS cars.
- Coding/adaptation with backup/restore on at least one verified platform.
- Cross-platform GUI + CLI, installers, docs, 3 hardware transports supported.
- Open data set covering the top ~10 modules on common platforms, with a clear contribution path.

## 9. First prompts to give Claude Code

1. "Read PROJECT_PLAN.md. Scaffold the Rust workspace and CI for Phase 0. Create CLAUDE.md summarising the ground rules in section 0."
2. "Implement the Transport trait, Simulator and Replay transports with tests (Phase 1)."
3. "Implement ISO-TP with a fake-clock test suite (Phase 2, part 1)."
4. Continue phase by phase. Ask for a short plan and acceptance tests before each phase.
