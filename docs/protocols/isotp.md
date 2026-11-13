# ISO-TP (ISO 15765-2)

Transport for UDS on classic CAN. Code: `crates/opendiag-proto/src/isotp/`. Source: see [../sources.md](../sources.md).

## Scope

Classic CAN (8-byte frames), normal addressing (one CAN id each way), messages up to **4095 bytes**. Not
implemented: extended or mixed addressing, CAN FD, the escape-length First Frame for messages over 4095 bytes.

## Frames

The first nibble of byte 0 is the type:

| Type | Byte 0 | Content |
|---|---|---|
| Single Frame | `0x0N` | `N` = 1..7 data bytes follow |
| First Frame | `0x1N NN` | 12-bit total length (at least 8), then 6 data bytes |
| Consecutive Frame | `0x2S` | `S` = sequence number 0..15 (starts at 1, wraps), then up to 7 data bytes |
| Flow Control | `0x3F BS ST` | `F` = 0 continue, 1 wait, 2 overflow; block size `BS`; separation time `ST` |

Frames may be padded to 8 bytes with a configurable byte. Decoding rejects empty frames, Single Frame lengths of 0
or above 7, First Frames shorter than 8 bytes or declaring fewer than 8 bytes, and unknown types.

## STmin

`0x00..0x7F` is milliseconds; `0xF1..0xF9` is 100..900 microseconds; any other value is treated as the 127 ms maximum.

## State machines

`Sender` and `Receiver` are **sans-IO**: they take the current time as an argument and return what to do, so tests
use a fake clock and never sleep.

- `Sender::new` returns the first frame to transmit (a Single Frame, or a First Frame). It then waits for Flow Control
  (`N_Bs`), honours block size and STmin between Consecutive Frames, tolerates up to `max_wait_frames` Wait frames,
  and fails on Overflow or timeout. `poll(now)` says `Send(bytes)`, `WaitUntil(t)` or `Done`.
- `Receiver` reassembles. A First Frame larger than `max_len` is answered with Overflow. A Consecutive Frame out of
  sequence is an error and resets to idle. `poll(now)` detects a stalled transfer (`N_Cr`).
- `IsoTpLink` connects both to a `Transport` using one request id and one response id; frames from other ids are ignored.

## Defaults

`block_size` 0 (no further flow control), `st_min` 0, `n_bs` and `n_cr` 1000 ms, `max_len` 4095,
`max_wait_frames` 10, no padding.

## Tested

Round trips for message lengths up to 4095 across block sizes and STmin values (including random lengths), exact
timing of STmin and the timeouts, wait and overflow handling, and 50,000 random inputs into the decoder and both
state machines with no panics.

## Not verified

Behaviour against real ECUs. Real modules differ in how they use STmin, padding and Wait frames.
