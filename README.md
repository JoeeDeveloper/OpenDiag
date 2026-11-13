# OpenDiag

An open source, cross-platform diagnostic suite for VAG group vehicles (VW, Audi, Seat, Skoda): a VCDS alternative on commodity hardware.

**Status:** Phase 0 (foundations). Nothing talks to a car yet.

- Read-only by default. Writes need an explicit unlock, a backup and a confirmation.
- Clean-room: no proprietary data. See [docs/legal.md](docs/legal.md).
- Roadmap: [PROJECT_PLAN.md](PROJECT_PLAN.md).

```bash
cargo run -p opendiag-cli -- --version
```

Licensed under GPL-3.0-or-later. See [CONTRIBUTING.md](CONTRIBUTING.md).
