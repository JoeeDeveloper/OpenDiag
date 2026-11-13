# Protocols

Implemented in `crates/opendiag-proto`. Sources for every protocol are in [../sources.md](../sources.md); none of them has been verified against a real vehicle yet ([../status.md](../status.md)).

| Protocol | Module | Notes |
|---|---|---|
| ISO-TP (ISO 15765-2) | `isotp` | Classic CAN, normal addressing, up to 4095 bytes. Sans-IO state machines plus `IsoTpLink`. |
| UDS (ISO 14229) | `uds` | Sessions, read DID/DTC, gated clear/write/routine/security access. |
| KWP2000 (ISO 14230) | `kwp` | K-line framing and services, fast init. |
| KW1281 | `kw1281` | 5-baud init, byte-wise lock-step blocks, ident/faults/groups. |
| TP2.0 | `tp20` | VW CAN transport carrying KWP2000. Community-documented, reverse-engineered. |

Detailed per-protocol pages (framing, timing, state machines) are still to be written.
