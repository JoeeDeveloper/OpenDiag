# Protocols

Implemented in `crates/opendiag-proto`. Sources for every protocol are in [../sources.md](../sources.md). None of them
has been verified against a real vehicle yet ([../status.md](../status.md)); each page says what it assumes.

| Protocol | Page | Module | In one line |
|---|---|---|---|
| ISO-TP (ISO 15765-2) | [isotp.md](isotp.md) | `isotp` | Segmentation and flow control for UDS on classic CAN |
| UDS (ISO 14229) | [uds.md](uds.md) | `uds` | Sessions, read DID and DTCs, gated clear/write/routine/security access |
| KWP2000 (ISO 14230) | [kwp2000.md](kwp2000.md) | `kwp` | K-line framing and fast init, and the services shared with TP2.0 |
| KW1281 | [kw1281.md](kw1281.md) | `kw1281` | 5-baud init and byte-by-byte lock-step blocks of older modules |
| TP2.0 | [tp20.md](tp20.md) | `tp20` | VW's CAN transport carrying KWP2000, community-documented |

Which protocol a command uses is chosen on the command line: `--slow-init` is KW1281, `--fast-init` is KWP2000 on
K-line, `--can <iface>` is KWP2000 over TP2.0 (Linux), and the `--simulate*` flags use the built-in virtual modules.
