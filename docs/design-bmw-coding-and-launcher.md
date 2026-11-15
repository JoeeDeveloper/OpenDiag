# Design: BMW coding decode, `VehicleBackend`, and the launcher

**Status: design only. No code in this document exists yet.** It covers the last three items of the BMW brief.
Everything about BMW file formats below is *unknown to us* and is stated as such; the design is shaped so that
it does not depend on guesses.

## 1. Decoding coding blocks from the user's own local files (FDL / CAFD)

### Goal
Turn the raw bytes `opendiag bmw block read` returns into named options ("this bit enables X"), and let a user
change an option by name instead of by hand-editing hex, through the **same guarded block flow** (backup,
typed confirmation of the exact diff, read-back verify). Decoding never bypasses that flow.

### Hard rules (from CLAUDE.md and `docs/legal.md`)
- **Never bundle, commit, copy or redistribute BMW proprietary data** (PSdZData, CAFD files, FDL tables). Not in
  `data/`, not in test fixtures, not in the repo history, not in release artifacts.
- The loader reads **only a path the user supplies** (`--bmw-data <dir>` or `OPENDIAG_BMW_DATA`). No
  default search path, no download, no network fetch.
- BMW data is **not** CC0 and does not enter `opendiag-data`. `data validate` and `load_data()` never see it.
- Nothing decoded from it is written to disk by OpenDiag except in the user's own backup/log files. No cache
  directory of extracted definitions.
- Tests use **synthetic fixtures we author ourselves** in our own format. No test may need real BMW files.

### What we do not know (and will not guess)
The binary layout of CAFD/FDL files, how a CAFD is matched to an ECU and I-level, and which DIDs carry the
coding blocks. We have no verified public specification for any of these. So the design has two stages:

1. **Format investigation, done by the user's own files, before any decoder is written.** A read-only
   `opendiag bmw data inspect <file>` that prints only structural facts (size, magic bytes, section offsets)
   so the format can be worked out from the user's own files. Anything learned goes into `docs/sources.md`
   with how it was derived. If a format cannot be established from public sources or the user's own
   observation, the decoder is not built. That is an acceptable outcome.
2. **Decoder behind a trait** (below), so the rest of the system is written and tested without any real file.

### Proposed structure
New crate `opendiag-bmw-coding` (keeps BMW-file knowledge out of `opendiag-bmw`'s protocol code):

```rust
/// One named option inside a block, described in OUR schema.
pub struct CodingField { name: String, bit_offset: u32, bit_len: u32, values: Vec<(u64, String)> }

pub trait CodingSource {
    /// Field layout for one block of one ECU, if this source knows it.
    fn layout(&self, ecu: u8, i_level: &str, block: u16) -> Result<Option<Vec<CodingField>>>;
}

pub fn decode(layout: &[CodingField], block: &[u8]) -> Vec<DecodedField>;
pub fn encode_change(layout: &[CodingField], block: &[u8], field: &str, value: &str) -> Result<Vec<u8>>;
```

- `encode_change` returns a **new full block** (same length). The caller hands that to
  `plan_block_change`, so everything after is the existing guarded flow. The decoder cannot write.
- Two `CodingSource` implementations:
  - `SyntheticSource`: loads our own open JSON format (`opendiag-bmw-coding/0`, with a provenance field,
    separate from `opendiag-data`'s schema). Used by tests and by anyone who writes definitions by hand.
  - `LocalFilesSource`: reads the user's own files from the supplied path. Built only after stage 1 succeeds.
- Unknown field or value: error, never a guess. A block whose length does not match the layout is not decoded.
- Decoded values are display aids. The authority on what gets written is the byte diff shown to the user.

### CLI sketch
```
opendiag bmw coding show   --target 0x12 --did <hex> --bmw-data <dir>
opendiag bmw coding set    --target 0x12 --did <hex> --bmw-data <dir> --field NAME --value VALUE
```
`set` prints the named change *and* the byte diff, then goes through the typed confirmation as `block write`.

### Safety additions specific to decoding
- Refuse `set` when the source cannot match the ECU's current I-level exactly. A layout for the wrong software
  level could flip the wrong bit.
- Refuse to run decode on a block whose read-back differs between two consecutive reads.
- Show raw hex before and after in every confirmation, not just the names.

## 2. Extracting `VehicleBackend`

### Principle
Extract the trait from the two working implementations; do not design it ahead of them. We now have both.

### What the two implementations actually share
| Need | VAG today | BMW today |
|---|---|---|
| Identify | `Module::identify` -> `Ident` | `read_ident` -> `BmwIdent` (VIN, raw I-level, raw FA) |
| Fault codes | `Module::read_dtcs` / `FaultAccess` | same trait, hex text |
| Guarded clear | `FaultAccess` + `apply_clear` | same |
| Keep-alive | `Module::keep_alive` | same |
| Multi-ECU scan | none (one module at a time) | `scan` through the gateway |
| Live data | `LiveData::read_group` (KW1281 groups) | `log_dids_csv` (DIDs) |
| Block writes | none | `BlockAccess` |
| Adaptation | `AdaptationAccess` (KW1281) | none |

Shared and already abstracted: `Module`, `FaultAccess`, the backup store, the clear flow. Genuinely different:
connection setup, how ECUs are enumerated, live data shape, and coding.

### Proposed trait (to be written only when a launcher needs it)
```rust
pub trait Vehicle {
    fn brand(&self) -> Brand;                       // Vag | Bmw
    fn connect(&mut self, opts: &ConnectOptions) -> Result<()>;
    fn identify(&mut self) -> Result<VehicleIdent>; // vin + brand-specific extras as labelled lines
    fn scan(&mut self) -> Result<Vec<EcuEntry>>;    // address/name/fault count
    fn ecu(&mut self, id: EcuId) -> Result<&mut dyn Module>;
    fn keep_alive(&mut self) -> Result<()>;
}
```
- `ecu()` returns the existing `Module`, so every front end keeps using `faults()`, `plan_clear`, etc.
- Optional capabilities stay **optional traits reached through accessors** (`block()`, `adaptation()`,
  `live()`), exactly as `Module::adaptation()` already works. No brand-wide "does everything" interface, and
  no default method that pretends a capability exists.
- `VehicleIdent` carries VIN plus a list of labelled `(label, value, verified: bool)` lines, so unverified BMW
  items (I-level, FA) stay visibly marked in every front end.
- Live data is **not** unified in v1: KW1281 groups and DID logging stay separate. Forcing them together would
  invent an abstraction neither implementation justifies.
- `opendiag-session` (a VAG whole-vehicle scan) was never built. The trait must not assume it; VAG's `scan`
  can return a single entry until that exists.

### Where it lives
A small `opendiag-vehicle` crate depending on `opendiag-services` and re-used by CLI and `serve`. VAG and BMW
backends are adapters in their own crates or a thin module each, so neither crate depends on the other.

## 3. Launcher

### Selection
1. `--brand vag|bmw` on the CLI. Explicit always wins.
2. `serve --brand vag|bmw`: **chosen at launch**, not changed from inside a page. The web handler is GET-only
   and holds one module; a brand switch mid-session would mean tearing down a live connection from a GET.
   The page shows which brand it is serving.
3. Optional auto-detect, **off unless asked** (`--brand auto`): an ENET discovery reply (UDP 6811) on the
   local link means BMW; a K-line/CAN adapter selected means VAG. This is a hint, not a decision: the chosen
   brand is printed before anything is sent, and ambiguity (both present) is an error asking for `--brand`.

### Migration path (no behaviour change to VAG)
- Existing commands (`scan`, `dtc`, `clear`, `kw1281 ...`) stay as they are. `opendiag bmw ...` stays.
- `--brand` is added first as a pure router to the existing code paths. Only after that does anything move
  onto `Vehicle`.
- Each step is a separate commit with the full suite green. The VAG tests are the safety net, as with the
  `MessageLink` refactor.

### Out of scope
Any GUI launcher (that waits for the Tauri app), an HTTP API, and cross-brand features.

## 4. Order of work and what blocks it

1. `--brand` router and `serve --brand` (no new abstraction; low risk, can start now).
2. `Vehicle` extraction, driven by step 1's actual call sites.
3. Synthetic coding format and `encode_change`/`decode` with synthetic fixtures (can start now, no BMW data).
4. `bmw data inspect` and the real-file decoder: **blocked on the user's own files and on a car or capture**
   to confirm coding DIDs. Not started until both exist.

## 5. Open questions for the user
- Are you willing to share *structural* facts from your own PSdZData (sizes, headers) so stage 1 can start?
  Never the files themselves.
- Is `--brand auto` wanted at all, or is an explicit flag enough?
- Should `serve` stay one-brand-per-process, or is running two instances acceptable?
