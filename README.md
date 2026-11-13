# OpenDiag

An open source, cross-platform diagnostic suite for VAG group vehicles (VW, Audi, Seat, Skoda): a VCDS alternative on commodity hardware.

**Status: early, simulator-tested, never run against a real car.** The protocol stack, a CLI and an open data format exist and pass about 300 automated tests, but every test uses a simulator or a published capture. Read [docs/status.md](docs/status.md) before connecting anything to a vehicle.

## What works today

| Area | State |
|---|---|
| Protocols | ISO-TP, UDS, KWP2000 (K-line), KW1281 (5-baud init), TP2.0, all against simulators |
| Transports | Serial K-line (Windows/Linux/macOS), SocketCAN (Linux), ELM327-style adapters (degraded, UDS over 11-bit CAN), Simulator, Replay |
| Modules | `scan` and `dtc read/clear` work on KW1281 (`--slow-init`), KWP2000 on K-line (`--fast-init`) and KWP2000 over TP2.0 on CAN (`--can`, Linux); live data and adaptation are KW1281 only |
| CLI | `scan`, `dtc read/clear`, `report` (standalone HTML), `serve` (read-only local web page), `doctor` (support summary), `log` (live data to CSV), `adaptation read/set`, `backup list/restore`, `probe`, `ports`, `capture`, `--trace` on all hardware commands, `data validate/schema` |
| Data | Open YAML format with validator and generated JSON Schema (CC0-1.0) |

## Install

Download the archive for your platform from the project's Releases page, unpack it, and run `opendiag` from inside
the unpacked folder (it finds the open data files in `./data`). Check the download against `SHA256SUMS`. The
binaries are not code-signed, so Windows and macOS will warn; see [docs/releasing.md](docs/releasing.md).
In PowerShell, run it as `.\opendiag.exe scan --simulate-kw1281`: PowerShell does not run programs from the current folder unless you prefix them with `.\`.
Or build from source with `cargo build --release -p opendiag-cli`.

Try it without a car:

```bash
cargo run -p opendiag-cli -- scan --simulate-kw1281
cargo run -p opendiag-cli -- dtc read --simulate
cargo run -p opendiag-cli -- log --simulate-kw1281 --groups 1,2 --samples 4 --interval-ms 100
cargo run -p opendiag-cli -- report --simulate-kw1281 --out report.html   # then open it in a browser
```

`serve` starts a read-only web page for the connected module, reachable from this computer only:

```bash
cargo run -p opendiag-cli -- serve --simulate-kw1281   # prints a URL with a one-time token; open it
```

It shows identification, fault codes and raw live data. It cannot clear codes or change anything; see
[docs/web.md](docs/web.md).

`report` writes one self-contained HTML file (no scripts, nothing loaded from the network, dark mode and print
styles included). Print it to PDF from your browser to share it. It may contain a VIN, so read it before sending it
on. Reports from the built-in virtual modules carry a "SIMULATED DATA" banner.

## Help wanted: testers with a car

OpenDiag has never been run against a real vehicle, and the author cannot test on one for a long time. If you
have a K-line cable, a SocketCAN adapter or an ELM327-style adapter and a car, a careful **read-only** test and a
report is the most useful contribution possible. Start with [docs/testing-guide.md](docs/testing-guide.md); it
covers what to run, what to attach and what never to run. `opendiag doctor --out doctor.txt` makes a summary for
your report (it contacts nothing and contains no VIN).

## Safety

- Read-only by default. Writes are limited to clearing trouble codes and KW1281 adaptation changes. Both take a durable backup or snapshot first, need a typed confirmation of the exact change shown (there is no `--yes`), and are verified by reading back; see [docs/safety.md](docs/safety.md). Nothing flashes or reprograms.
- Clean-room: no proprietary data. See [docs/legal.md](docs/legal.md) and [docs/sources.md](docs/sources.md).
- Roadmap: [PROJECT_PLAN.md](PROJECT_PLAN.md).

Code is GPL-3.0-or-later; contributed data is CC0-1.0. See [CONTRIBUTING.md](CONTRIBUTING.md).
