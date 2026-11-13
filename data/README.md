# Open data

Community-authored module, DTC and measuring-block definitions, as YAML. See
[docs/data-format.md](../docs/data-format.md) for how to write and contribute them.

```bash
cargo run -p opendiag-cli -- data validate data     # check everything (also runs in CI)
cargo run -p opendiag-cli -- data schema            # print the JSON Schema
```

- `modules.yaml` - diagnostic addresses and module names
- `dtc_sae_generic.yaml` - standard SAE generic powertrain codes, descriptions in our own words
- `schema/opendiag-data-v0.schema.json` - generated from the Rust model; do not edit by hand

Every file carries provenance. **No Ross-Tech, VCDS or ODIS derived content, ever**
([docs/legal.md](../docs/legal.md)).
