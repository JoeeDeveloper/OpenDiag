# Sources

Every piece of protocol knowledge must be traceable to a public source listed here. Add an entry before (or in the same PR as) the code that relies on it.

| Topic | Source | Notes |
|---|---|---|
| ISO-TP | ISO 15765-2 | Segmentation, flow control, STmin/BS |
| UDS | ISO 14229 | Services and negative response codes |
| KWP2000 | ISO 14230 | K-line framing and services |
| K-line init | ISO 9141 | 5-baud slow init |
| KW1281 | KW1281Test, https://github.com/gmenounos/kw1281test (MIT licence), read 2026-11-13 | Protocol facts only, no code copied: block layout `[len, counter, title, data, 0x03]` with `len` counting everything after itself; per-byte complement acknowledgement for every byte except the end byte; title numbers (`BlockTitle.cs`); ident blocks follow the keyword exchange directly; `0xFF 0xFF 0x88` fault record means "no fault"; 3-byte fault and measuring-cell records; "more data" flag is the top bit of the first ident byte. Not verified against a real module. |
| KW1281 5-baud init | Same source (`KwpCommon.cs`) and https://www.blafusel.de/obd/obd2_kw1281.html as cited there | Address sent as 1 start, 7 data bits LSB first, odd parity, 1 stop; sync byte 0x55 then keyword bytes; tester answers with the complement of the second keyword (0x8A for KW1281). KKL cables are driven with DTR high, RTS low. |
| KW1281 timing (R6 ack delay, 25 ms block gap) | Same source | Values in `Kw1281Config` are conservative placeholders, not measured. |
| TP2.0 | "VW Transport Protocol 2.0 (TP 2.0) for CAN bus", https://jazdw.net/tp20 (read 2026-11-13) | Community documentation, reverse-engineered from captures; the author says they had no access to official VW documents. Used for: setup request/reply on CAN id 0x200 / 0x200+address and the id/validity encoding; parameters messages (0xA0/0xA1), channel test 0xA3, break 0xA4, disconnect 0xA8; the timing byte (2-bit unit, 6-bit scale, 0xFF unused); data opcodes 0-3 and ACK 0xB/0x9; the 2-byte message length in the first packet; per-direction 4-bit sequence counters. The worked capture on that page is replayed byte for byte in `crates/opendiag-proto/tests/tp20.rs`. Unverified against a real module. Our assumptions beyond the page: the module's T3 is the minimum gap we leave between our packets, and its block size is how many packets we send before waiting for an ACK. |

Do not cite or derive from Ross-Tech label files, VCDS or ODIS.
