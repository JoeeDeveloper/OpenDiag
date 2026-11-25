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

## Replaying the web dashboard

Record exactly what the page asks for by tracing `serve`, then use the page for a while (identification,
fault codes, start live data) and stop with Ctrl-C:

```bash
opendiag serve --port /dev/ttyUSB0 --slow-init --trace page.trace
opendiag serve --replay page.trace --slow-init        # later, no cable
```

The page shows a **REPLAY** banner, and a report made under `--replay` is labelled *REPLAYED DATA*. Each live
reading in the recording is played back in order, and the recording loops when it runs out.

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
- **Matching is by content.** On K-line (KW1281) the recording is a stream of single bytes, so identical
  acknowledgements are ambiguous. Replaying in recorded order works. Asking for something that was *not*
  recorded does **not** reliably give the clean `not in recording` error there: it can surface as a protocol
  error (for example "module acknowledged 0x03 with 0x09") or, in principle, as the wrong reply. Record
  everything you intend to replay. CAN/UDS matches whole frames and gives the clean error.
- **Timing is ignored.**
- **Read-only, always.** `dtc clear` and `adaptation set` are refused under `--replay` before they plan
  anything or take a backup, so a recording can never be mistaken for a real change.
- **BMW (ENET/HSFZ) is not covered yet.** It runs over TCP, not this transport.
- Like every test here, a replay of a *simulator* recording only proves the code agrees with itself.
  Replaying a recording from a real module is what tests our understanding of the protocol.

A trace holds whatever the vehicle reported (VIN, part numbers, fault history). Read the privacy note in
[capture.md](capture.md) before sharing one.

## BMW (HSFZ) sessions

`opendiag bmw ... --record FILE` writes the raw HSFZ frames crossing the TCP connection to a trace
(`TX raw ..` to the gateway, `RX raw ..` from it); `--replay FILE` runs any read-only `bmw` command
(`ident`, `did`, `scan`, `dtc read`, `block read`, `coding show`, `live`, `serve`) against it with no
network. `--record` also works with `--simulate`, which is how the test fixtures are made.

```bash
opendiag bmw ident --ip 192.168.0.10 --record car.trace
opendiag bmw ident --replay car.trace
```

Same rules as the K-line replay: requests are matched by content, a request that was never
recorded is an error naming it (nothing is invented), replies repeat the recorded bytes, timing is
ignored, and writes (`dtc clear`, `block write/restore`, `coding set`) are refused. A recording holds
one connection; to replay several commands, record each and concatenate the files. UDP discovery
is not recorded: pass `--target` when replaying.
