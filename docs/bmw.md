# BMW F-series (ENET / HSFZ)

**Status: simulator-tested only.** None of this has run against a real car, gateway or ENET cable. Read-only.

```bash
opendiag bmw discover                 # find gateways (UDP broadcast, port 6811)
opendiag bmw ident [--ip 169.254.x.x] # VIN, I-level, FA (raw) from the gateway
opendiag bmw did --target 0x12 0xF190 # read one data identifier from one ECU
opendiag bmw ident --simulate         # no car: built-in virtual BMW
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
write mode. Clearing DTCs and writes are not wired to the CLI yet.

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
