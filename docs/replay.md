# Replaying a recorded session (`--replay`)

`--replay FILE` runs any vehicle command against a recorded trace instead of a car, so you can develop
and demo with no hardware attached.

```bash
# 1. Record once (against a simulator, or a real module with your own cable):
opendiag dtc read --port /dev/ttyUSB0 --slow-init --trace session.trace

# 2. Replay as often as you like, with no cable:
opendiag dtc read --replay session.trace --slow-init
opendiag scan     --replay session.trace --slow-init
opendiag serve    --replay session.trace --slow-init
```

Say which protocol the recording used, as you would for a live connection: `--slow-init` for KW1281,
`--fast-init` for KWP2000, neither for UDS. `--replay` cannot be combined with `--simulate*`, `--port`,
`--can`, `--elm327` or `--trace`.

## How it answers

Each thing the tool does (send a frame, set a baud rate, start an init) is looked up in the recording
**by content**, searching forward from the last match and then from the start. The replies that followed
it in the recording are returned. So requests can be repeated, and made in a different order from the
recording, as long as each was recorded at least once. This is different from `TraceReplay`
([capture.md](capture.md)), which is strict and is used for regression tests.

## Limits

- **Only what was recorded can be answered.** Anything else fails with `not in recording: the client did
  ...`, naming the request. Nothing is guessed or made up. To replay `dtc read`, record `dtc read`.
- **Replies are the recorded bytes.** A value that changes on a live car (an RPM, a counter) repeats its
  recorded value, so a replay shows a snapshot and not a running vehicle.
- **Matching is by content.** On K-line, identical one-byte acknowledgements are ambiguous. The forward-first
  search handles a session replayed in recorded order; out of order it can pick the wrong one.
- **Timing is ignored.**
- **Read-only, always.** `dtc clear` and `adaptation set` are refused under `--replay` before they plan
  anything or take a backup, so a recording can never be mistaken for a real change.
- **BMW (ENET/HSFZ) is not covered yet.** It runs over TCP, not this transport.
- Like every test here, a replay of a *simulator* recording only proves the code agrees with itself.
  Replaying a recording from a real module is what tests our understanding of the protocol.

A trace holds whatever the vehicle reported (VIN, part numbers, fault history). Read the privacy note in
[capture.md](capture.md) before sharing one.
