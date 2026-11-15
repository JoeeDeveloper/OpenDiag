# BMW F-series (ENET / HSFZ)

**Status: simulator-tested only.** None of this has run against a real car, gateway or ENET cable. Read-only by default. The only writes are `dtc clear` and `block write/restore`, both through the guarded flow below.

```bash
opendiag bmw discover                 # find gateways (UDP broadcast, port 6811)
opendiag bmw ident [--ip 169.254.x.x] # VIN, I-level, FA (raw) from the gateway
opendiag bmw did --target 0x12 0xF190 # read one data identifier from one ECU
opendiag bmw scan [--all]             # which ECUs answer, and how many fault codes each holds
opendiag bmw dtc read --target 0x12   # stored fault codes (raw hex + status flags)
opendiag bmw dtc clear --target 0x12  # guarded: snapshot, typed YES, clear, re-read
opendiag bmw block read --target 0x12 --did 0x2000            # raw block as hex
opendiag bmw block write --target 0x12 --did 0x2000 --hex ... # guarded write
opendiag bmw block restore b0003 --target 0x12                # put a backed-up block back
opendiag bmw live --target 0x12 --did 0x4002,0xF190 --samples 20   # raw CSV log
opendiag bmw ident --simulate         # no car: built-in virtual BMW (works for every command)
```

Before connecting to a real car: ignition on, a battery charger connected. The ENET cable needs the car's
activation line (OBD pin 8) wired; a plain Ethernet cable will not wake the gateway.

## How it is built

`opendiag-bmw` speaks HSFZ over TCP and plugs into the shared UDS client through the `MessageLink` trait
(`opendiag-proto`). There is no ISO-TP: the gateway carries whole UDS messages. One TCP connection serves every
ECU behind the gateway; each ECU gets an `HsfzLink` addressed by its diagnostic address.

Keep-alive is driven by the caller between requests (`tester_present`), not by a background thread, because a
second thread could read another request's reply off the shared stream. If the gateway drops the connection,
`HsfzGateway::reconnect` reopens it; ECU session state must then be re-established by the caller.

Safety: the link refuses programming sessions (`0x10 02`) and download/transfer services (`0x34`-`0x38`) at any
write mode.

### Step 2: scan, fault codes, live data

- **Scan** probes each candidate address with TesterPresent and, for ECUs that answer, reads the fault-code
  count. Default candidates are the short address table (`ecus.rs`); `--all` sweeps `0x00`-`0xFF`.
  Only TesterPresent and ReadDTCInformation are sent (tested).
- **DTCs** are shown as raw 3-byte hex (e.g. `480A12`) with decoded status bits. BMW codes are not SAE J2012
  codes, so no `P`/`B`/`C`/`U` text is invented, and no descriptions are bundled.
- **Clear** reuses the same guarded flow as the VAG modules (`plan_clear` / `apply_clear`): a confirmation bound
  to the exact codes shown, a re-read, a flushed snapshot in the backup file before anything is sent, the clear,
  and a re-read afterwards. There is no `--yes`. This path is part of
  `crates/opendiag-services/tests/write_safety.rs` (no clear reaches the ECU without a durable snapshot, under
  corrupted bytes, and none if the snapshot cannot be written).
- **Live data** logs raw DID values as hex at a fixed rate. Nothing is converted to units: we have no verified
  DID definitions. Which DIDs a given ECU offers is for you to find (`opendiag bmw did`).
- **ECU address table** (`crates/opendiag-bmw/src/ecus.rs`): six names, **NOT VERIFIED**, labels only. It does
  not affect what is sent.

### Step 3: raw block write (guarded)

`block write` replaces one data block, addressed by a data identifier **you** supply, with new bytes of the same
length. It is raw bytes only: nothing here knows what the bytes mean, and no DID for coding data is built in,
because we have no verified one. Decoding (FDL/CAFD) is design only and will load from your own local
PSdZData path; BMW data is never bundled (see [legal.md](legal.md)).

The flow (`opendiag-services/src/block.rs`, shared by any protocol):

1. the confirmation must be bound to the exact change shown (old bytes, new bytes, DID);
2. identification identifiers (`0xF100`-`0xF1FF`, VIN and part numbers), identical blocks and length changes are refused;
3. the block is re-read and must still equal what was shown;
4. the complete old block is written to the backup file and flushed. If that fails, nothing is sent;
5. the write is sent (a `WriteToken` only this flow can create);
6. the block is read back and must equal what was written. Otherwise the backup is marked `verification_failed`
   and the error names it. Nothing is rewritten automatically.

`block restore <id>` is the same guarded flow in reverse, from the stored old block, and marks the original
`restored`. It refuses a backup taken from a different ECU. There is no `--yes`. The block-write path is part of
`write_safety.rs`. Programming sessions and flash services stay refused at the link; `--extended-session` opens
`0x10 03` only, and is off by default because whether it is needed is unverified.

**Do not use this on a real car yet.** The write itself, the DID, the ECU's session and any security-access
requirement are all unverified. Keep a charger connected and the ignition on, and expect to need a capture from
your own car first.

## Verified vs unverified

Verified against public sources (see [sources.md](sources.md)):

- TCP port 6801 for diagnostics, UDP port 6811 for discovery.
- Frame: 4-byte big-endian length (payload only), 2-byte control word, payload. A diagnostic message (`0x0001`)
  payload is `source, target, UDS data`, one byte each address. No routing activation (unlike DoIP).
- Control words: `0x01` diagnostic, `0x02` acknowledge, `0x10` terminal 15, `0x11` vehicle ident,
  `0x12` alive check, `0x13` status inquiry, `0x40`-`0x45` and `0xFF` errors.
- Default tester address `0xF4`.
- VIN is UDS DID `0xF190` (ISO 14229).

**NOT verified.** These are flagged in the code and must be checked on a real car:

- The exact ASCII layout of the discovery reply (`DIAGADR..BMWMAC..BMWVIN..`). Unparseable replies are shown raw.
- That the discovery *request* is a `0x0011` frame with an empty payload (inferred from the Scapy dissector).
- The acknowledge (`0x0002`) payload beyond its two address bytes, and what an alive check (`0x0012`) expects as
  a reply. This client ignores both and relies on UDS TesterPresent.
- The I-level DID (`0x100B`) and FA DID (`0x3F06`), and how to decode them. Output is raw bytes. A wrong DID
  just yields "not available".
- Whether an F30 needs ReadDTCInformation sub-functions other than `0x02`.
- The gateway's diagnostic address (`0x10` is the usual one and is taken from discovery when available).
- Whether an F-series ECU needs an extended diagnostic session (`0x10 03`) before ReadDTCInformation or
  ClearDiagnosticInformation. Currently neither is sent in a special session.
- How a real gateway reports an address with no ECU (silence or an error frame). The scan treats both as absent.
- The names in the ECU address table.
- Whether `0x14 FFFFFF` clears every ECU's codes on a real F30, or only some groups.
- Which data identifiers hold coding blocks on an F-series ECU, whether `0x2E` on them needs an extended session
  or security access, and whether the ECU accepts a whole-block write. Not guessed anywhere: you pass the DID.
