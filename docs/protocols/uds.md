# UDS (ISO 14229)

Diagnostic services on CAN, over ISO-TP. Code: `crates/opendiag-proto/src/uds.rs`.

## Services

| Service | Name | Status |
|---|---|---|
| `0x10` | DiagnosticSessionControl | implemented; returns the P2 / P2* timings the ECU reports |
| `0x3E` | TesterPresent | implemented |
| `0x22` | ReadDataByIdentifier | implemented; the DID echo is checked |
| `0x19 02` | ReadDTCInformation by status mask | implemented; 4-byte records (3-byte code, status) |
| `0x14` | ClearDiagnosticInformation | implemented, **gated** |
| `0x2E` | WriteDataByIdentifier | implemented, **gated** (not used by any command yet) |
| `0x31` | RoutineControl | implemented, **gated** (not used by any command yet) |
| `0x27` | SecurityAccess | implemented, **gated**; seed-key algorithm is a trait, **none are bundled** |

Not implemented, and out of scope for v1: ECU reset, memory read/write, data transfer, any flashing or reprogramming.

"Gated" means the client refuses unless it is in unlocked write mode. The only caller that unlocks it is the guarded
flow in `opendiag-services` ([../safety.md](../safety.md)); the CLI uses nothing else.

## Responses

A positive response has `SID + 0x40` first. A negative response is `7F SID NRC`.

- NRC `0x78` (response pending) is waited out: the client waits `P2*` (default 5000 ms) up to `max_pending` times
  (default 20), then gives up. The first wait is `P2` (default 1000 ms).
- Any other NRC becomes `Error::NegativeResponse { service, nrc }`; `nrc_name()` gives the standard names.
- A negative response to a different service (a stale reply) is skipped.

## What `scan` and `dtc` use

Identification reads DIDs `0xF187` (part number), `0xF189` (software version) and `0xF190` (VIN). A negative response
`0x31` (request out of range) means "this ECU does not have it" and shows as not supported; a silent ECU is an error.

Fault codes are shown as SAE-style `P0171-00` (generic format, not a manufacturer code): the first byte's top two bits
pick `P/C/B/U`, followed by the remaining digits and the failure-type byte. The status byte is decoded into the eight
ISO 14229 flags (`testFailed`, `confirmed`, `pending`, and so on).

## Not verified

Everything here follows the ISO structure, not captures of VAG modules. VAG-specific session numbers, DIDs and
negative-response behaviour will differ. No UDS ECU has been tested.
