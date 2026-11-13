# OpenDiag

An open source, cross-platform diagnostic suite for VAG group vehicles (VW, Audi, Seat, Skoda): a VCDS alternative on commodity hardware.

**Status: early, simulator-tested, never run against a real car.** The protocol stack, a CLI and an open data format exist and pass ~180 automated tests, but every test uses a simulator or a published capture. Read [docs/status.md](docs/status.md) before connecting anything to a vehicle.

## What works today

| Area | State |
|---|---|
| Protocols | ISO-TP, UDS, KWP2000 (K-line), KW1281 (5-baud init), TP2.0, all against simulators |
| Transports | Serial K-line (Windows/Linux/macOS), SocketCAN (Linux), Simulator, Replay |
| CLI | `scan`, `dtc read/clear`, `log` (live data to CSV), `adaptation read/set`, `backup list/restore`, `probe`, `ports`, `capture`, `--trace` on all hardware commands, `data validate/schema` |
| Data | Open YAML format with validator and generated JSON Schema (CC0-1.0) |

Try it without a car:

```bash
cargo run -p opendiag-cli -- scan --simulate-kw1281
cargo run -p opendiag-cli -- dtc read --simulate
cargo run -p opendiag-cli -- log --simulate-kw1281 --groups 1,2 --samples 4 --interval-ms 100
```

## Safety

- Read-only by default. Writes are limited to clearing trouble codes and KW1281 adaptation changes. Adaptation changes always take a durable backup first, need a typed confirmation of the exact diff (no `--yes`), and are verified by reading back; see [docs/safety.md](docs/safety.md). Nothing flashes or reprograms.
- Clean-room: no proprietary data. See [docs/legal.md](docs/legal.md) and [docs/sources.md](docs/sources.md).
- Roadmap: [PROJECT_PLAN.md](PROJECT_PLAN.md).

Code is GPL-3.0-or-later; contributed data is CC0-1.0. See [CONTRIBUTING.md](CONTRIBUTING.md).
