## Summary
<!-- What changes and why. Link issues with "Closes #N". -->

## Checklist
- [ ] `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass
- [ ] No test depends on hardware (use Simulator or Replay)
- [ ] Protocol sources are recorded in `docs/sources.md`
- [ ] Clean-room: no Ross-Tech labels, VCDS/ODIS data or other proprietary datasets
- [ ] Any new vehicle write goes through the guarded flow and is added to `crates/opendiag-services/tests/write_safety.rs`
- [ ] Data contributions include provenance and are CC0-1.0

## Verification
<!-- Simulator only, or captured on real hardware? Say which; nothing is verified on a car unless stated. -->
