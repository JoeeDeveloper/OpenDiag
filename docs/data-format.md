# Open data format v0

All module names, trouble code descriptions and measuring-block labels live in YAML files
under `data/`. The format is defined by the Rust types in `crates/opendiag-data/src/model.rs`;
the JSON Schema in `data/schema/` is generated from them, so the two cannot disagree.

## File layout

```yaml
format: opendiag-data/0
kind: modules            # modules | dtcs | measuring_blocks
provenance:
  author: Your Name
  source: own_vehicle_capture   # original_work | own_vehicle_capture | public_specification
  license: CC0-1.0              # CC0-1.0 | CC-BY-4.0 | GPL-3.0-or-later
  captured_on: 2026-11-13       # required for own_vehicle_capture
  verified: false               # true only once checked on a real vehicle
  notes: Optional free text.
entries:
  - ...
```

Unknown keys are errors, so typos are caught rather than silently ignored.

## Entry kinds

**modules**: `address` (e.g. `0x01`), `name`, optional `description`, `protocols`, `platforms`.

**dtcs**: `code_space` (`sae_j2012` for `P0171`-style codes, `vag_5digit` for `00525`-style),
`code`, `description` (max 200 characters, your own words).

**measuring_blocks**: `protocol`, `module_address`, `group`, and `cells` (a `label` and optional
`unit` each). KW1281 groups have at most 4 cells.

## Rules

- **Clean-room.** Write descriptions in your own words from your own vehicle or from public
  standards. Never copy from, convert or paraphrase Ross-Tech label files, VCDS output or ODIS
  data. The validator rejects provenance text that names those sources; if it trips on an
  honest note, reword it. See [legal.md](legal.md).
- **Provenance is mandatory** and `verified` stays `false` until you have confirmed the entry on
  a real vehicle.
- A key (module address, code, block) may be defined **once** across all files.
- Short and factual beats long and clever.

## Checking your work

```bash
cargo run -p opendiag-cli -- data validate data
```

This reports every problem with its file name, not just the first. CI runs it on every PR.
If you change the model, regenerate the schema (a test fails until you do):

```bash
cargo run -p opendiag-cli -- data schema > data/schema/opendiag-data-v0.schema.json
```

## Using the data

`opendiag dtc read` shows descriptions automatically when it finds `./data` or the directory in
`$OPENDIAG_DATA`.

## Open questions

The data licence default (`CC0-1.0`) is a project decision that has not been made final.
