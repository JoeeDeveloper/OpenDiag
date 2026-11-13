//! Checks the JSON Schema cannot express: formats, uniqueness, provenance rules.

use std::collections::HashSet;

use crate::model::{
    BlockDef, Body, CodeSpace, DataFile, DtcDef, ModuleDef, Protocol, Provenance, Source, FORMAT,
};

/// Contributed data is dedicated to the public domain (project decision, 2026-11-13).
const LICENSES: [&str; 1] = ["CC0-1.0"];
const MAX_DESCRIPTION: usize = 200;
const MAX_KW1281_CELLS: usize = 4;

/// Return every problem found in `file` (empty means valid).
pub fn validate(file: &DataFile) -> Vec<String> {
    let mut errs = Vec::new();
    if file.format != FORMAT {
        errs.push(format!("format is `{}`, expected `{FORMAT}`", file.format));
    }
    provenance(&file.provenance, &mut errs);
    match &file.body {
        Body::Modules(m) => modules(m, &mut errs),
        Body::Dtcs(d) => dtcs(d, &mut errs),
        Body::MeasuringBlocks(b) => blocks(b, &mut errs),
    }
    errs
}

/// `YYYY-MM-DD` with plausible month and day.
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| s.get(r).and_then(|t| t.parse::<u32>().ok());
    matches!(
        (digits(0..4), digits(5..7), digits(8..10)),
        (Some(y), Some(m), Some(d)) if y >= 1980 && (1..=12).contains(&m) && (1..=31).contains(&d)
    )
}

/// Clean-room tripwire: refuse provenance text that names proprietary sources.
/// It is a safety net, not a legal test; contributors who trip it should
/// reword and, if the data really came from such a source, not submit it.
fn mentions_proprietary_source(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    for needle in ["ross-tech", "rosstech", ".lbl"] {
        if lower.contains(needle) {
            return Some(needle);
        }
    }
    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    ["vcds", "odis", "vagcom"]
        .into_iter()
        .find(|bad| tokens.contains(bad))
}

fn provenance(p: &Provenance, errs: &mut Vec<String>) {
    if p.author.trim().is_empty() {
        errs.push("provenance.author must not be empty".into());
    }
    if !LICENSES.contains(&p.license.as_str()) {
        errs.push(format!(
            "provenance.license `{}` is not one of {}",
            p.license,
            LICENSES.join(", ")
        ));
    }
    match (&p.captured_on, p.source) {
        (None, Source::OwnVehicleCapture) => {
            errs.push("provenance.captured_on is required for own_vehicle_capture".into());
        }
        (Some(d), _) if !is_iso_date(d) => {
            errs.push(format!(
                "provenance.captured_on `{d}` is not a YYYY-MM-DD date"
            ));
        }
        _ => {}
    }
    check_plain("provenance.author", &p.author, false, errs);
    if let Some(n) = &p.notes {
        check_plain("provenance.notes", n, true, errs);
    }
    for (field, text) in [("author", Some(&p.author)), ("notes", p.notes.as_ref())] {
        if let Some(bad) = text.and_then(|t| mentions_proprietary_source(t)) {
            errs.push(format!(
                "provenance.{field} mentions `{bad}`: data derived from proprietary tools or \
                 label files must not be contributed (see docs/legal.md)"
            ));
        }
    }
}

/// Reject control characters. Data strings are printed to terminals, so an
/// escape sequence in a description could rewrite the screen or fake output.
/// `allow_newline` lets free-form notes span lines.
fn check_plain(what: &str, text: &str, allow_newline: bool, errs: &mut Vec<String>) {
    if text
        .chars()
        .any(|c| c.is_control() && !(allow_newline && c == '\n'))
    {
        errs.push(format!("{what} contains control characters"));
    }
}

fn check_text(what: &str, text: &str, errs: &mut Vec<String>) {
    check_plain(what, text, false, errs);
    if text.trim().is_empty() {
        errs.push(format!("{what} must not be empty"));
    } else if text.chars().count() > MAX_DESCRIPTION {
        errs.push(format!(
            "{what} is longer than {MAX_DESCRIPTION} characters"
        ));
    }
}

fn modules(entries: &[ModuleDef], errs: &mut Vec<String>) {
    let mut seen = HashSet::new();
    for m in entries {
        if !seen.insert(m.address) {
            errs.push(format!("duplicate module address {:#04X}", m.address));
        }
        check_text(&format!("module {:#04X} name", m.address), &m.name, errs);
        if let Some(d) = &m.description {
            check_text(&format!("module {:#04X} description", m.address), d, errs);
        }
        for p in &m.platforms {
            check_text(&format!("module {:#04X} platform", m.address), p, errs);
        }
    }
}

/// `P0171`: letter P/C/B/U, then 0-3, then three hex digits.
fn is_sae_code(code: &str) -> bool {
    let b = code.as_bytes();
    b.len() == 5
        && matches!(b[0], b'P' | b'C' | b'B' | b'U')
        && matches!(b[1], b'0'..=b'3')
        && b[2..]
            .iter()
            .all(|c| matches!(c, b'0'..=b'9' | b'A'..=b'F'))
}

fn dtcs(entries: &[DtcDef], errs: &mut Vec<String>) {
    let mut seen = HashSet::new();
    for d in entries {
        let ok = match d.code_space {
            CodeSpace::SaeJ2012 => is_sae_code(&d.code),
            CodeSpace::Vag5Digit => d.code.len() == 5 && d.code.bytes().all(|c| c.is_ascii_digit()),
        };
        if !ok {
            errs.push(format!(
                "code `{}` is not a valid {:?} code",
                d.code, d.code_space
            ));
        }
        if !seen.insert((d.code_space, d.code.clone())) {
            errs.push(format!("duplicate code `{}`", d.code));
        }
        check_text(
            &format!("code `{}` description", d.code),
            &d.description,
            errs,
        );
    }
}

fn blocks(entries: &[BlockDef], errs: &mut Vec<String>) {
    let mut seen = HashSet::new();
    for b in entries {
        let id = format!("{:#04X} group {}", b.module_address, b.group);
        if !seen.insert((b.protocol, b.module_address, b.group)) {
            errs.push(format!("duplicate measuring block {id}"));
        }
        if b.cells.is_empty() {
            errs.push(format!("measuring block {id} has no cells"));
        }
        if b.protocol == Protocol::Kw1281 && b.cells.len() > MAX_KW1281_CELLS {
            errs.push(format!(
                "measuring block {id} has {} cells; KW1281 groups have at most {MAX_KW1281_CELLS}",
                b.cells.len()
            ));
        }
        for (i, c) in b.cells.iter().enumerate() {
            check_text(
                &format!("measuring block {id} cell {} label", i + 1),
                &c.label,
                errs,
            );
            if let Some(u) = &c.unit {
                check_text(
                    &format!("measuring block {id} cell {} unit", i + 1),
                    u,
                    errs,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert!(is_iso_date("2026-11-13"));
        for bad in [
            "2026-13-01",
            "2026-00-10",
            "26-11-13",
            "2026/11/13",
            "2026-11-1x",
            "",
        ] {
            assert!(!is_iso_date(bad), "{bad}");
        }
    }

    #[test]
    fn sae_codes() {
        assert!(is_sae_code("P0171") && is_sae_code("U3FFF") && is_sae_code("B1A2B"));
        for bad in ["P4171", "X0171", "P017", "p0171", "P0G71", "P01711"] {
            assert!(!is_sae_code(bad), "{bad}");
        }
    }

    #[test]
    fn tripwire_matches_names_but_not_innocent_words() {
        assert!(mentions_proprietary_source("from a Ross-Tech label file").is_some());
        assert!(mentions_proprietary_source("copied from VCDS").is_some());
        assert!(mentions_proprietary_source("an ODIS export").is_some());
        assert!(mentions_proprietary_source("engine.LBL").is_some());
        assert!(mentions_proprietary_source("melodis odisha tardis").is_none());
        assert!(mentions_proprietary_source("my own Polo 9N3").is_none());
    }
}
