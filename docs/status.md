# Status: what is verified and what is not

Last reviewed 2026-11-13. This is deliberately blunt.

## What the tests prove

~180 automated tests pass on Windows, Ubuntu and macOS CI. They prove the code does what its authors
*understood the protocols to be*, against:

- simulators written by the same authors from the same documents (so a shared misunderstanding passes);
- one published TP2.0 capture, replayed byte for byte;
- robustness runs (random bytes into every decoder, ~1,250 corrupted sessions through every client)
  that found no panics or hangs.

## No hardware for now

Development is running entirely against simulators until a cable and a vehicle are available again (months).
That makes the simulators the only oracle, so they are deliberately adversarial: virtual modules enforce the
protocol rules and record every mistake, a fault-injecting transport corrupts traffic, and property tests check
safety claims (notably "no write without a durable backup") rather than examples. The limit of this approach is
unchanged: **a simulator written from the same documents as the code cannot reveal a shared misunderstanding.**
Real captures are the only thing that can, which is why `--trace`/`capture` and `TraceReplay` exist.

## What nothing has proved

**No part of this has ever talked to a real module.** In particular:

- **KW1281** timing (`ack_delay` 2 ms, `block_gap` 25 ms) is a placeholder. Real modules may need different
  values, and a known real-world quirk (a repeated sync byte mid-session) is not handled.
- **5-baud init** timing through a Windows USB serial driver is untested; break pulses can jitter.
- **TP2.0** follows reverse-engineered community notes. Two interpretations beyond them are ours: the module's
  T3 is the gap we leave between our packets, and its block size is how many we send before waiting for an ACK.
  The client errors (rather than guesses) if a module sends data before acknowledging ours.
- **KWP2000 / UDS** services follow the ISO structure, not captures of VAG modules; VAG-specific behaviour
  (response codes, session numbers, identification options) will differ.
- **SocketCAN** has compiled and passed unit tests on CI but has never opened a real interface.
- **Cable setup** (DTR high / RTS low) is copied from another tool's behaviour, untested here.

## Known weaknesses

- **Both writes go through a guarded flow** ([safety.md](safety.md)): adaptation changes and fault-code clearing
  need a confirmation bound to the exact change shown, take a durable backup/snapshot *before* anything is sent,
  verify by reading back, and cannot be called around it (the module-level write functions need a token only the
  flow can create). There is no `--yes` bypass on either. Two honest limits: the protocol-level methods
  (`Kw1281Client::clear_fault_codes` and friends) are still public for tests and tools, each gated by `WriteMode`;
  and the guard is only as good as its tests, which are simulator-based. Short/long coding is not implemented,
  so nothing else writes.
- **The adaptation save reply format is unconfirmed**, so success is decided by reading the value back. Nothing
  about adaptation has run against a real module.
- **Capture/trace tooling is new and itself unverified on hardware.** `--trace` and `opendiag capture` exist
  ([capture.md](capture.md)) but have only run against simulators; the serial and CAN capture paths have never
  heard a real bus. The only committed trace fixture is synthetic.
- Fault status bytes and measuring-block formulas are shown raw; there is no decoding data yet.
- KW1281 has no desync recovery. (The KWP2000 services used to be duplicated between K-line and TP2.0; they
  now share one `KwpService` trait.)
- **KWP2000 in `scan`/`dtc` is VAG-unverified.** It follows ISO 14230-3: the session mode (0x89, from the public
  TP2.0 capture), the identification option (0x9B) and the fault-code display (5-digit decimal, raw status byte)
  are assumptions. Identification is shown raw with no part number claimed. There is no live data or adaptation
  over KWP2000 yet.
- `Drop` on a dead link can block for one byte timeout (~1 s) while it tries to end the session.
- `serde_yaml` is unmaintained upstream; plan a migration. YAML alias expansion was measured and is bounded.
- CI: the macOS runner sometimes fails to start (capacity), unrelated to the code. `main` has no branch
  protection or required checks.

## Reports

`opendiag report` produces a standalone HTML file. It is checked structurally by tests (escaping with hostile
input, a Content-Security-Policy that forbids scripts and network loads, no external references, a simulated-data
banner). **Its visual layout has not been inspected in real browsers or print preview**, and there is no direct
PDF output (print to PDF from a browser). It covers identification and fault codes only: no live data, history or
vehicle-level summary yet.

## Not started (from the plan)

J2534 and ELM327 transports, PCAN, coding/adaptation/output tests, backup/restore,
the Tauri GUI, packaging/signing, vehicle profiles, mdBook docs, WASM plugins.

## First hardware session: a safe checklist

Use a **bench module** (out of the car, powered from a 12 V supply, K-line wired to the cable) or a car with
the ignition on and engine off. Read-only steps only.

1. `opendiag ports`: confirm the cable appears. Note the port name.
2. `opendiag probe --port <PORT> --address 0x01 --slow-init --trace probe.trace` (a module that never
   answers is normal at first; try `--baud 9600`, then `--no-echo`). **Always add `--trace`.**
3. If that connects: `opendiag scan --port <PORT> --slow-init --trace scan.trace`, then `opendiag dtc read ...`,
   then `opendiag log --groups 1 --samples 5 ...` (each with `--trace`).
4. **Do not run `dtc clear` until reads are proven reliable** on that module.
5. Keep the exact command, the console output, the `.trace` file and the adapter model. A failure with those is
   debuggable; without them it is not. Read the trace for your VIN before sharing it
   ([capture.md](capture.md)).
