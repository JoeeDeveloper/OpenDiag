# Contributing

1. Read the clean-room policy in [docs/legal.md](docs/legal.md). It is mandatory.
2. Cite any protocol source in [docs/sources.md](docs/sources.md).
3. Use conventional commits (`feat:`, `fix:`, `docs:`, `test:`, `chore:`).
4. Before opening a PR run:
   ```bash
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
5. Tests must not need hardware. Use the Simulator or Replay transports.
6. Writes to a vehicle must be gated behind the unlock mode with backup and confirmation.

Data contributions (modules, DTCs, labels) need a `captured_on` provenance field and must come from your own vehicle or original work.
