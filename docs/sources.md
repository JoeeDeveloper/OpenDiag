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
| TP2.0 | Public documentation | VAG CAN transport protocol |

Do not cite or derive from Ross-Tech label files, VCDS or ODIS.
