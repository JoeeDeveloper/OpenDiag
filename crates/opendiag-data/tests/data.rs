use std::fs;
use std::path::{Path, PathBuf};

use opendiag_data::{json_schema, load_dir, parse_str, Body, CodeSpace, Protocol};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const HEADER: &str = "format: opendiag-data/0\nprovenance:\n  author: Tester\n  source: original_work\n  license: CC0-1.0\n";

fn modules(entries: &str) -> String {
    format!("{HEADER}kind: modules\nentries:\n{entries}")
}

fn dtcs(entries: &str) -> String {
    format!("{HEADER}kind: dtcs\nentries:\n{entries}")
}

fn blocks(entries: &str) -> String {
    format!("{HEADER}kind: measuring_blocks\nentries:\n{entries}")
}

fn first_error(text: &str) -> String {
    match parse_str("t.yaml", text) {
        Ok(_) => panic!("expected an error for:\n{text}"),
        Err(e) => e
            .iter()
            .map(|i| i.message.clone())
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

#[test]
fn parses_each_kind() {
    let m = parse_str("m", &modules("  - { address: 0x01, name: Engine }\n")).unwrap();
    assert!(matches!(m.body, Body::Modules(ref v) if v[0].address == 1));
    let d = parse_str(
        "d",
        &dtcs("  - { code_space: sae_j2012, code: P0171, description: Too lean }\n"),
    )
    .unwrap();
    assert!(matches!(d.body, Body::Dtcs(ref v) if v[0].code == "P0171"));
    let b = parse_str(
        "b",
        &blocks(
            "  - protocol: kw1281\n    module_address: 0x01\n    group: 1\n    cells:\n      - { label: Engine speed, unit: rpm }\n      - { label: Coolant temperature, unit: C }\n",
        ),
    )
    .unwrap();
    assert!(matches!(b.body, Body::MeasuringBlocks(ref v) if v[0].cells.len() == 2));
}

#[test]
fn rejects_bad_headers_and_provenance() {
    let ok = "  - { address: 1, name: X }\n";
    assert!(
        first_error(&modules(ok).replace("opendiag-data/0", "opendiag-data/9"))
            .contains("expected")
    );
    assert!(first_error(&modules(ok).replace("CC0-1.0", "proprietary")).contains("license"));
    assert!(first_error(&modules(ok).replace("Tester", "  ")).contains("author"));
    assert!(
        first_error(&modules(ok).replace("original_work", "own_vehicle_capture"))
            .contains("captured_on is required")
    );
    let bad_date = modules(ok).replace(
        "license: CC0-1.0",
        "license: CC0-1.0\n  captured_on: 13/11/2026",
    );
    assert!(first_error(&bad_date).contains("YYYY-MM-DD"));
    let good_capture = modules(ok)
        .replace("original_work", "own_vehicle_capture")
        .replace(
            "license: CC0-1.0",
            "license: CC0-1.0\n  captured_on: 2026-11-13",
        );
    assert!(parse_str("t", &good_capture).is_ok());
}

#[test]
fn clean_room_tripwire_rejects_proprietary_sources() {
    let ok = "  - { address: 1, name: X }\n";
    for note in ["exported from VCDS", "Ross-Tech label file", "ODIS dump"] {
        let text = modules(ok).replace(
            "license: CC0-1.0",
            &format!("license: CC0-1.0\n  notes: {note}"),
        );
        assert!(first_error(&text).contains("legal.md"), "{note}");
    }
}

#[test]
fn rejects_unknown_keys_anywhere() {
    let ok = "  - { address: 1, name: X }\n";
    assert!(first_error(&format!("{}extra: 1\n", modules(ok))).contains("unknown top-level key"));
    assert!(first_error(&modules("  - { address: 1, name: X, colour: red }\n")).contains("colour"));
    let bad_prov = modules(ok).replace("license: CC0-1.0", "license: CC0-1.0\n  nots: typo");
    assert!(first_error(&bad_prov).contains("nots"));
}

#[test]
fn rejects_structural_problems() {
    assert!(first_error("- just\n- a list\n").contains("mapping"));
    assert!(first_error("a: [unclosed").contains("invalid YAML"));
    assert!(first_error(&format!("{HEADER}kind: gadgets\nentries: []\n")).contains("gadgets"));
    assert!(first_error(&modules("  - { address: 300, name: X }\n")).contains("300"));
}

#[test]
fn control_characters_are_rejected_everywhere_text_is_shown() {
    // YAML "\e" is the ESC character; "\n" a newline inside a quoted scalar.
    let esc = "\"red \\e[31m\"";
    assert!(
        first_error(&modules(&format!("  - {{ address: 1, name: {esc} }}\n")))
            .contains("control characters")
    );
    assert!(first_error(&modules(&format!(
        "  - {{ address: 1, name: X, description: {esc} }}\n"
    )))
    .contains("control characters"));
    assert!(first_error(&modules(&format!(
        "  - {{ address: 1, name: X, platforms: [{esc}] }}\n"
    )))
    .contains("control characters"));
    assert!(first_error(&dtcs(&format!(
        "  - {{ code_space: sae_j2012, code: P0171, description: {esc} }}\n"
    )))
    .contains("control characters"));
    let cell_unit = blocks(&format!(
        "  - protocol: kw1281\n    module_address: 1\n    group: 1\n    cells:\n      - {{ label: x, unit: {esc} }}\n"
    ));
    assert!(first_error(&cell_unit).contains("control characters"));
    let bad_author = modules("  - { address: 1, name: X }\n").replace("Tester", "\"A\\u001b[2J\"");
    assert!(first_error(&bad_author).contains("control characters"));
    let bad_notes = modules("  - { address: 1, name: X }\n")
        .replace("license: CC0-1.0", "license: CC0-1.0\n  notes: \"x\\ty\"");
    assert!(
        first_error(&bad_notes).contains("control characters"),
        "tabs are control chars too"
    );
    // A newline in notes is fine; in a name it is not.
    let ok_notes = modules("  - { address: 1, name: X }\n").replace(
        "license: CC0-1.0",
        "license: CC0-1.0\n  notes: \"line one\\nline two\"",
    );
    assert!(parse_str("t", &ok_notes).is_ok());
    assert!(first_error(&modules("  - { address: 1, name: \"a\\nb\" }\n")).contains("control"));
}

#[test]
fn oversized_files_are_refused_without_being_read() {
    let dir = std::env::temp_dir().join(format!("opendiag-data-big-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("huge.yaml"), vec![b'#'; 1024 * 1024 + 1]).unwrap();
    let errs = load_dir(&dir).unwrap_err();
    fs::remove_dir_all(&dir).unwrap();
    assert!(
        errs.iter()
            .any(|e| e.file == "huge.yaml" && e.message.contains("limit")),
        "{errs:?}"
    );
}

#[test]
fn module_rules() {
    assert!(first_error(&modules(
        "  - { address: 1, name: A }\n  - { address: 1, name: B }\n"
    ))
    .contains("duplicate module address 0x01"));
    assert!(first_error(&modules("  - { address: 1, name: '' }\n")).contains("must not be empty"));
}

#[test]
fn dtc_rules() {
    let bad = |code: &str, space: &str| {
        first_error(&dtcs(&format!(
            "  - {{ code_space: {space}, code: {code}, description: d }}\n"
        )))
    };
    assert!(bad("P4171", "sae_j2012").contains("not a valid"));
    assert!(bad("p0171", "sae_j2012").contains("not a valid"));
    assert!(bad("0052", "vag_5digit").contains("not a valid"));
    assert!(bad("'0052A'", "vag_5digit").contains("not a valid"));
    assert!(parse_str(
        "t",
        &dtcs("  - { code_space: vag_5digit, code: '00525', description: d }\n")
    )
    .is_ok());
    assert!(first_error(&dtcs(
        "  - { code_space: sae_j2012, code: P0171, description: a }\n  - { code_space: sae_j2012, code: P0171, description: b }\n"
    ))
    .contains("duplicate code"));
    let long = "x".repeat(201);
    assert!(first_error(&dtcs(&format!(
        "  - {{ code_space: sae_j2012, code: P0171, description: {long} }}\n"
    )))
    .contains("longer than"));
}

#[test]
fn block_rules() {
    let block = |cells: usize| {
        let cells: String = if cells == 0 {
            "      []\n".to_string()
        } else {
            (0..cells)
                .map(|i| format!("      - {{ label: c{i} }}\n"))
                .collect()
        };
        blocks(&format!(
            "  - protocol: kw1281\n    module_address: 1\n    group: 3\n    cells:\n{cells}"
        ))
    };
    assert!(parse_str("t", &block(4)).is_ok());
    assert!(first_error(&block(5)).contains("at most 4"));
    assert!(first_error(&block(0)).contains("no cells"));
    let dup = format!("{}  - protocol: kw1281\n    module_address: 1\n    group: 3\n    cells: [{{ label: x }}]\n", block(1));
    assert!(first_error(&dup).contains("duplicate measuring block"));
}

#[test]
fn reports_all_problems_at_once() {
    let text = modules("  - { address: 1, name: '' }\n  - { address: 1, name: B }\n");
    let errs = parse_str("t", &text).unwrap_err();
    assert!(errs.len() >= 2, "{errs:?}");
}

#[test]
fn load_dir_merges_files_and_flags_cross_file_duplicates() {
    let dir = std::env::temp_dir().join(format!("opendiag-data-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(
        dir.join("a.yaml"),
        modules("  - { address: 1, name: Engine }\n"),
    )
    .unwrap();
    fs::write(
        dir.join("sub").join("b.yml"),
        dtcs("  - { code_space: sae_j2012, code: P0300, description: Misfire }\n"),
    )
    .unwrap();
    fs::write(dir.join("notes.txt"), "ignored").unwrap();

    let set = load_dir(&dir).unwrap();
    assert_eq!(set.file_count(), 2);
    assert_eq!(set.module(1).unwrap().name, "Engine");
    assert_eq!(
        set.dtc(CodeSpace::SaeJ2012, "P0300").unwrap().description,
        "Misfire"
    );
    assert!(set.dtc(CodeSpace::Vag5Digit, "P0300").is_none());
    assert!(set.block(Protocol::Kw1281, 1, 1).is_none());

    fs::write(
        dir.join("c.yaml"),
        modules("  - { address: 1, name: Other }\n"),
    )
    .unwrap();
    let errs = load_dir(&dir).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.file == "c.yaml" && e.message.contains("already defined in a.yaml")),
        "{errs:?}"
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_dir_reports_bad_files_with_their_name() {
    let dir = std::env::temp_dir().join(format!("opendiag-data-bad-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("broken.yaml"), "format: nope\n").unwrap();
    let errs = load_dir(&dir).unwrap_err();
    assert!(errs.iter().all(|e| e.file == "broken.yaml"));
    fs::remove_dir_all(&dir).unwrap();
    assert!(load_dir(&dir).is_err(), "missing directory is an error");
}

#[test]
fn shipped_data_validates_and_has_expected_entries() {
    let set = load_dir(&repo_root().join("data")).unwrap_or_else(|e| {
        panic!(
            "shipped data is invalid:\n{}",
            e.iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        )
    });
    assert!(set.module_count() >= 10);
    assert_eq!(set.module(0x01).unwrap().name, "Engine");
    assert!(set.dtc(CodeSpace::SaeJ2012, "P0171").is_some());
}

#[test]
fn committed_schema_matches_the_model() {
    let path = repo_root().join("data/schema/opendiag-data-v0.schema.json");
    let committed: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("schema file exists")).unwrap();
    assert_eq!(
        committed,
        json_schema(),
        "schema is stale; regenerate with: cargo run -p opendiag-cli -- data schema > data/schema/opendiag-data-v0.schema.json"
    );
}
