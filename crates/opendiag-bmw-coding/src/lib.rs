//! A name-based view of raw coding blocks, from **our own** open definition
//! format (`opendiag-bmw-coding/0`).
//!
//! Pure functions over bytes: nothing here talks to an ECU or writes anything.
//! [`encode_change`] returns a complete new block; handing that to the guarded
//! block flow (`opendiag-services::block`: backup, typed confirmation of the
//! byte diff, read-back verify) is the caller's job, and the only way it gets
//! written.
//!
//! The definitions are community-authored (or written by hand) and carry a
//! provenance field. This crate never reads BMW files: their format is not
//! known to us (see `docs/design-bmw-coding-and-launcher.md`).
//!
//! Bit numbering is our own choice and is part of the format: bit `n` of a
//! block is bit `n % 8` (0 = least significant) of byte `n / 8`, and a field's
//! value is its bits read least-significant first starting at `bit_offset`.

use std::collections::{BTreeMap, BTreeSet};

use opendiag_core::{Error, Result};
use serde::Deserialize;

pub const FORMAT_ID: &str = "opendiag-bmw-coding/0";

fn err(msg: impl Into<String>) -> Error {
    Error::Protocol(format!("coding: {}", msg.into()))
}

/// One named option inside a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodingField {
    pub name: String,
    pub bit_offset: u32,
    pub bit_len: u32,
    /// Known values and their labels, sorted by value. Empty: any value is allowed.
    pub values: Vec<(u64, String)>,
}

impl CodingField {
    fn mask(&self) -> u64 {
        if self.bit_len >= 64 {
            u64::MAX
        } else {
            (1u64 << self.bit_len) - 1
        }
    }
}

/// The fields of one block of one ECU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockLayout {
    pub ecu: u8,
    pub block: u16,
    /// Exact block length in bytes.
    pub length: usize,
    /// The software level this layout is for, if the definition says.
    pub i_level: Option<String>,
    pub fields: Vec<CodingField>,
}

/// A field as it reads in a particular block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedField {
    pub name: String,
    pub raw: u64,
    /// The label for `raw`, if the definition has one.
    pub label: Option<String>,
}

/// Where layouts come from.
pub trait CodingSource {
    /// The layout for one block. An error says why there is none (unknown block,
    /// or the definition is for a different software level).
    fn layout(&self, ecu: u8, i_level: Option<&str>, block: u16) -> Result<&BlockLayout>;
}

// ---- the open JSON format ----

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDef {
    format: String,
    provenance: String,
    blocks: Vec<BlockDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockDef {
    ecu: u8,
    block: u16,
    length: usize,
    #[serde(default)]
    i_level: Option<String>,
    fields: Vec<FieldDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldDef {
    name: String,
    bit_offset: u32,
    bit_len: u32,
    #[serde(default)]
    values: BTreeMap<String, String>,
}

/// Definitions loaded from our own JSON format.
#[derive(Debug, Clone, Default)]
pub struct SyntheticSource {
    layouts: Vec<BlockLayout>,
    pub provenance: String,
}

fn parse_number(s: &str) -> Option<u64> {
    let s = s.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

impl SyntheticSource {
    /// Parse and validate a definition file. Strict: unknown keys, an empty
    /// provenance, fields outside the block, overlapping fields, values that do
    /// not fit their field and duplicate names are all errors.
    pub fn from_json(text: &str) -> Result<Self> {
        let def: FileDef = serde_json::from_str(text).map_err(|e| err(format!("bad JSON: {e}")))?;
        if def.format != FORMAT_ID {
            return Err(err(format!(
                "format is `{}`, expected `{FORMAT_ID}`",
                def.format
            )));
        }
        if def.provenance.trim().is_empty() {
            return Err(err("`provenance` must say where the definitions come from"));
        }
        let mut layouts: Vec<BlockLayout> = Vec::new();
        for b in def.blocks {
            let at = format!("ECU {:#04X} block {:#06X}", b.ecu, b.block);
            if layouts
                .iter()
                .any(|l| l.ecu == b.ecu && l.block == b.block && l.i_level == b.i_level)
            {
                return Err(err(format!("{at}: defined twice")));
            }
            if b.length == 0 {
                return Err(err(format!("{at}: length must be at least 1")));
            }
            let total_bits = b.length as u64 * 8;
            let mut used = BTreeSet::new();
            let mut names = BTreeSet::new();
            let mut fields = Vec::new();
            for f in b.fields {
                if f.name.trim().is_empty() || !names.insert(f.name.clone()) {
                    return Err(err(format!(
                        "{at}: empty or duplicate field name `{}`",
                        f.name
                    )));
                }
                if f.bit_len == 0 || f.bit_len > 64 {
                    return Err(err(format!(
                        "{at}: field `{}` bit_len must be 1..=64",
                        f.name
                    )));
                }
                if u64::from(f.bit_offset) + u64::from(f.bit_len) > total_bits {
                    return Err(err(format!(
                        "{at}: field `{}` runs past the end of the block",
                        f.name
                    )));
                }
                for bit in f.bit_offset..f.bit_offset + f.bit_len {
                    if !used.insert(bit) {
                        return Err(err(format!(
                            "{at}: field `{}` overlaps another field",
                            f.name
                        )));
                    }
                }
                let mut field = CodingField {
                    name: f.name,
                    bit_offset: f.bit_offset,
                    bit_len: f.bit_len,
                    values: Vec::new(),
                };
                let mut labels = BTreeSet::new();
                for (k, label) in f.values {
                    let v = parse_number(&k).ok_or_else(|| {
                        err(format!(
                            "{at}: value key `{k}` of `{}` is not a number",
                            field.name
                        ))
                    })?;
                    if v > field.mask() {
                        return Err(err(format!(
                            "{at}: value {v} does not fit in {} bit(s) of `{}`",
                            field.bit_len, field.name
                        )));
                    }
                    if !labels.insert(label.clone()) || parse_number(&label).is_some() {
                        return Err(err(format!(
                            "{at}: label `{label}` of `{}` is duplicated or looks like a number",
                            field.name
                        )));
                    }
                    if field.values.iter().any(|(x, _)| *x == v) {
                        return Err(err(format!(
                            "{at}: value {v} of `{}` is listed twice",
                            field.name
                        )));
                    }
                    field.values.push((v, label));
                }
                field.values.sort();
                fields.push(field);
            }
            layouts.push(BlockLayout {
                ecu: b.ecu,
                block: b.block,
                length: b.length,
                i_level: b.i_level,
                fields,
            });
        }
        Ok(Self {
            layouts,
            provenance: def.provenance,
        })
    }
}

impl CodingSource for SyntheticSource {
    fn layout(&self, ecu: u8, i_level: Option<&str>, block: u16) -> Result<&BlockLayout> {
        let candidates: Vec<&BlockLayout> = self
            .layouts
            .iter()
            .filter(|l| l.ecu == ecu && l.block == block)
            .collect();
        if candidates.is_empty() {
            return Err(err(format!(
                "no definition for ECU {ecu:#04X} block {block:#06X}"
            )));
        }
        // A definition that names a software level is used only for exactly that level.
        if let Some(l) = candidates.iter().find(|l| match (&l.i_level, i_level) {
            (None, _) => true,
            (Some(want), Some(have)) => want == have,
            (Some(_), None) => false,
        }) {
            return Ok(l);
        }
        let wanted: Vec<&str> = candidates
            .iter()
            .filter_map(|l| l.i_level.as_deref())
            .collect();
        Err(err(format!(
            "the definitions for ECU {ecu:#04X} block {block:#06X} are for software level(s) {wanted:?}, but {} was given; refusing to guess",
            i_level.map_or("none".to_string(), |s| format!("`{s}`"))
        )))
    }
}

// ---- decode / encode ----

fn check_len(layout: &BlockLayout, block: &[u8]) -> Result<()> {
    if block.len() != layout.length {
        return Err(err(format!(
            "the block has {} bytes but the definition says {}",
            block.len(),
            layout.length
        )));
    }
    Ok(())
}

fn get_bits(block: &[u8], offset: u32, len: u32) -> u64 {
    (0..len).fold(0u64, |acc, i| {
        let bit = offset + i;
        let b = (block[(bit / 8) as usize] >> (bit % 8)) & 1;
        acc | (u64::from(b) << i)
    })
}

fn set_bits(block: &mut [u8], offset: u32, len: u32, value: u64) {
    for i in 0..len {
        let bit = offset + i;
        let byte = &mut block[(bit / 8) as usize];
        let mask = 1u8 << (bit % 8);
        if (value >> i) & 1 == 1 {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
    }
}

/// Read every field of `block`. Refuses a block of the wrong length.
pub fn decode(layout: &BlockLayout, block: &[u8]) -> Result<Vec<DecodedField>> {
    check_len(layout, block)?;
    Ok(layout
        .fields
        .iter()
        .map(|f| {
            let raw = get_bits(block, f.bit_offset, f.bit_len);
            DecodedField {
                name: f.name.clone(),
                raw,
                label: f
                    .values
                    .iter()
                    .find(|(v, _)| *v == raw)
                    .map(|(_, l)| l.clone()),
            }
        })
        .collect())
}

/// A complete new block with one field changed and every other bit untouched
/// (including bits that no field describes). `value` is a label or a number
/// (decimal or `0x` hex). When the field lists its values, anything else is refused.
pub fn encode_change(
    layout: &BlockLayout,
    block: &[u8],
    field: &str,
    value: &str,
) -> Result<Vec<u8>> {
    check_len(layout, block)?;
    let f = layout
        .fields
        .iter()
        .find(|f| f.name == field)
        .ok_or_else(|| err(format!("no field named `{field}`")))?;
    let v = match f.values.iter().find(|(_, l)| l == value) {
        Some((v, _)) => *v,
        None => parse_number(value).ok_or_else(|| {
            let known: Vec<&str> = f.values.iter().map(|(_, l)| l.as_str()).collect();
            err(format!(
                "`{value}` is neither a number nor one of {known:?}"
            ))
        })?,
    };
    if v > f.mask() {
        return Err(err(format!(
            "{v} does not fit in the {} bit(s) of `{field}`",
            f.bit_len
        )));
    }
    if !f.values.is_empty() && !f.values.iter().any(|(x, _)| *x == v) {
        return Err(err(format!(
            "{v} is not a known value of `{field}`; known: {:?}",
            f.values
        )));
    }
    let mut out = block.to_vec();
    set_bits(&mut out, f.bit_offset, f.bit_len, v);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEF: &str = r#"{
      "format": "opendiag-bmw-coding/0",
      "provenance": "synthetic test data written for OpenDiag; not from any car",
      "blocks": [{
        "ecu": 18, "block": 8192, "length": 4, "i_level": "TEST-00",
        "fields": [
          {"name": "mode", "bit_offset": 0, "bit_len": 2, "values": {"0": "off", "1": "eco", "2": "sport"}},
          {"name": "flag", "bit_offset": 7, "bit_len": 1, "values": {"0": "no", "1": "yes"}},
          {"name": "limit", "bit_offset": 12, "bit_len": 8}
        ]
      }]
    }"#;

    fn src() -> SyntheticSource {
        SyntheticSource::from_json(DEF).unwrap()
    }

    fn layout() -> BlockLayout {
        src().layout(0x12, Some("TEST-00"), 0x2000).unwrap().clone()
    }

    #[test]
    fn decodes_fields_across_byte_boundaries() {
        // mode=2 (bits0-1), flag=1 (bit7), limit=0xAB at bits 12..20.
        let mut b = vec![0u8; 4];
        set_bits(&mut b, 0, 2, 2);
        set_bits(&mut b, 7, 1, 1);
        set_bits(&mut b, 12, 8, 0xAB);
        assert_eq!(b, vec![0x82, 0xB0, 0x0A, 0x00]);
        let d = decode(&layout(), &b).unwrap();
        assert_eq!((d[0].raw, d[0].label.as_deref()), (2, Some("sport")));
        assert_eq!((d[1].raw, d[1].label.as_deref()), (1, Some("yes")));
        assert_eq!((d[2].raw, d[2].label.as_deref()), (0xAB, None));
    }

    #[test]
    fn a_change_touches_only_its_own_bits() {
        let block = vec![0xFF, 0xFF, 0xFF, 0xFF];
        let out = encode_change(&layout(), &block, "mode", "eco").unwrap();
        assert_eq!(out, vec![0xFD, 0xFF, 0xFF, 0xFF]);
        let out = encode_change(&layout(), &block, "limit", "0x10").unwrap();
        // bits 12..20 now 0x10; every other bit, including undescribed ones, stays 1.
        let d = decode(&layout(), &out).unwrap();
        assert_eq!(d[2].raw, 0x10);
        let diff: u32 = block
            .iter()
            .zip(&out)
            .map(|(a, b)| (a ^ b).count_ones())
            .sum();
        assert_eq!(diff, 7, "0xFF -> 0x10 clears seven of the eight bits");
        // Round trip: decode, change to the same value, nothing differs.
        assert_eq!(encode_change(&layout(), &out, "limit", "16").unwrap(), out);
    }

    #[test]
    fn bad_changes_are_refused() {
        let b = vec![0u8; 4];
        let l = layout();
        assert!(encode_change(&l, &b, "nope", "1").is_err());
        assert!(encode_change(&l, &b, "mode", "turbo").is_err());
        assert!(
            encode_change(&l, &b, "mode", "3").is_err(),
            "3 is not a listed value"
        );
        assert!(
            encode_change(&l, &b, "limit", "256").is_err(),
            "does not fit 8 bits"
        );
        assert!(
            encode_change(&l, &b[..3], "mode", "eco").is_err(),
            "wrong length"
        );
        assert!(decode(&l, &b[..3]).is_err());
    }

    #[test]
    fn the_software_level_must_match_exactly() {
        let s = src();
        assert!(s.layout(0x12, Some("TEST-00"), 0x2000).is_ok());
        let e = s
            .layout(0x12, Some("TEST-01"), 0x2000)
            .unwrap_err()
            .to_string();
        assert!(e.contains("refusing to guess"), "{e}");
        assert!(s.layout(0x12, None, 0x2000).is_err());
        assert!(s.layout(0x12, Some("TEST-00"), 0x9999).is_err());
        assert!(s.layout(0x60, Some("TEST-00"), 0x2000).is_err());
    }

    #[test]
    fn invalid_definitions_are_rejected() {
        let bad = |from: &str, to: &str| {
            SyntheticSource::from_json(&DEF.replace(from, to))
                .unwrap_err()
                .to_string()
        };
        assert!(bad("opendiag-bmw-coding/0", "other/1").contains("format"));
        assert!(bad(
            "synthetic test data written for OpenDiag; not from any car",
            " "
        )
        .contains("provenance"));
        assert!(bad("\"bit_offset\": 12", "\"bit_offset\": 28").contains("past the end"));
        assert!(bad("\"bit_offset\": 7", "\"bit_offset\": 1").contains("overlaps"));
        assert!(bad("\"2\": \"sport\"", "\"4\": \"sport\"").contains("does not fit"));
        assert!(bad("\"name\": \"flag\"", "\"name\": \"mode\"").contains("duplicate"));
        assert!(bad("\"ecu\": 18,", "\"ecu\": 18, \"extra\": 1,").contains("bad JSON"));
        assert!(bad("\"yes\"", "\"1\"").contains("looks like a number"));
    }
}
