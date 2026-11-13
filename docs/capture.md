# Capturing and tracing sessions

When something goes wrong with real hardware, the most useful thing you can have is a byte-level record
of what was sent and received. OpenDiag can record two ways.

## 1. Trace our own session: `--trace FILE`

Add `--trace session.trace` to `scan`, `dtc read`, `dtc clear`, `log` or `probe`:

```bash
opendiag scan --port COM3 --slow-init --trace session.trace
```

Every byte sent and received is written with a timestamp, flushed line by line, so the file is valid even if
the program crashes or you press Ctrl-C. A failure to write the trace never disturbs the diagnostic session;
it prints a warning at the end instead.

## 2. Listen without sending: `opendiag capture`

```bash
opendiag capture --port COM3 --baud 10400 --out listen.trace --duration-secs 60
opendiag capture --can can0 --out can.trace          # Linux only; stop with Ctrl-C
```

This is receive-only: it never transmits, so it cannot disturb a bus. It records what it hears (for example
another tester talking to a module, through a cable wired in as a tap). Quiet periods are not logged.

## Format

Plain text, one event per line, easy to read and diff:

```text
# opendiag trace v1
0.000074 EV slow_init 01
0.000140 RX raw 55 01 8A
0.000185 TX raw 75
0.010000 TX can 7E0#0210030000000000
0.012000 RX can 7E8#065003003201F4
0.500000 EV recv_timeout 1000
0.500100 EV error recv: device unplugged
```

- Time is seconds since the trace started. `TX` is what we sent, `RX` what we received.
- `raw` lines are K-line bytes. `can` lines use `candump -L` notation (3 hex digits for 11-bit ids, 8 for 29-bit).
- `EV` lines are events: `slow_init`, `fast_init`, `baud`, `recv_timeout` (how long we waited for nothing) and `error`.
- Lines starting with `#` are comments.

## Privacy: read it before you share it

A trace contains whatever the vehicle reported, including **VIN, part numbers and fault history**. Before
attaching one to an issue, open it and remove or mask anything you do not want public. If you post a trace,
say which adapter, vehicle (model and year) and module you used.

## Using a trace as a test

A recording can be replayed strictly with `TraceReplay` (in `opendiag-transport`): the client under test must
send exactly the recorded bytes, in order, and gets the recorded replies. Any difference is an error that
names the recorded line. `crates/opendiag-services/tests/trace_replay.rs` does this with
`tests/fixtures/kw1281_sim_session.trace`.

That fixture is **synthetic**, recorded from the built-in virtual module. A trace captured from a real
vehicle drops into the same test: copy it to `tests/fixtures/`, add a test that runs the client against it,
and the real behaviour becomes a permanent regression check. Contributed traces must follow the rules in
[legal.md](legal.md): only your own vehicle, and nothing derived from proprietary tools.
