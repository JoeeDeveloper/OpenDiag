//! Contributed data files are untrusted input: parsing must never panic, hang
//! or balloon in memory, whatever they contain.

use std::time::{Duration, Instant};

use opendiag_data::parse_str;

const HEADER: &str = "format: opendiag-data/0\nkind: dtcs\nprovenance:\n  author: T\n  source: original_work\n  license: CC0-1.0\nentries:\n";

fn assert_quick(name: &str, text: &str) {
    let started = Instant::now();
    let _ = parse_str("hostile.yaml", text);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{name} took {:?}",
        started.elapsed()
    );
}

#[test]
fn alias_bombs_are_cheap_to_reject_or_parse() {
    // "Billion laughs": each level references the previous one ten times.
    let mut doc = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
    for i in 1..9 {
        let refs = vec![format!("*a{}", i - 1); 10].join(", ");
        doc.push_str(&format!("a{i}: &a{i} [{refs}]\n"));
    }
    assert_quick("alias bomb at top level", &doc);
    assert_quick(
        "alias bomb inside entries",
        &format!("{HEADER}  - {{ code_space: sae_j2012, code: P0171, description: *a8 }}\n{doc}"),
    );
}

#[test]
fn deep_nesting_is_rejected_without_overflowing_the_stack() {
    for depth in [100, 1_000, 20_000] {
        assert_quick(
            "deep sequence",
            &format!("{}{}", "[".repeat(depth), "]".repeat(depth)),
        );
        assert_quick(
            "deep mapping",
            &format!("{}x{}", "{a: ".repeat(depth), "}".repeat(depth)),
        );
        let indented: String = (0..depth.min(2_000))
            .map(|i| format!("{}- a\n", " ".repeat(i % 200)))
            .collect();
        assert_quick("deep indentation", &indented);
    }
}

#[test]
fn random_text_and_mutated_valid_files_never_panic() {
    let valid = format!("{HEADER}  - {{ code_space: sae_j2012, code: P0171, description: Fuel mixture too lean }}\n");
    let mut state = 12345u64;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state >> 33
    };
    let mut parsed_ok = 0;
    for _ in 0..3_000 {
        let mut bytes = valid.clone().into_bytes();
        for _ in 0..(next() % 6) {
            if bytes.is_empty() {
                break;
            }
            let i = (next() as usize) % bytes.len();
            match next() % 3 {
                0 => bytes[i] = next() as u8,
                1 => {
                    bytes.remove(i);
                }
                _ => bytes.insert(i, next() as u8),
            }
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if parse_str("m.yaml", &text).is_ok() {
            parsed_ok += 1;
        }
        let noise: String = (0..(next() % 200))
            .map(|_| char::from_u32((next() % 0x2000) as u32).unwrap_or('?'))
            .collect();
        let _ = parse_str("n.yaml", &noise);
    }
    // Some mutations are harmless, so some must still parse; otherwise the
    // mutator is too destructive to test anything.
    assert!(parsed_ok > 50, "only {parsed_ok} mutated files parsed");
}

#[test]
fn oversized_files_are_refused_by_the_loader() {
    let dir = std::env::temp_dir().join(format!("opendiag-oversize-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut text = String::from(HEADER);
    let mut n = 0u32;
    while text.len() <= (1 << 20) + 1 {
        text.push_str(&format!(
            "  - {{ code_space: vag_5digit, code: \"{:05}\", description: a }}
",
            n % 100_000
        ));
        n += 1;
    }
    std::fs::write(dir.join("big.yaml"), &text).unwrap();
    let started = Instant::now();
    let result = opendiag_data::load_dir(&dir);
    std::fs::remove_dir_all(&dir).ok();
    let issues = result.expect_err("an oversized file must be refused");
    assert!(
        issues.iter().any(|i| i.to_string().contains("limit")),
        "{issues:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn legitimate_nesting_is_not_refused() {
    let ok = format!(
        "{HEADER}  - {{ code_space: sae_j2012, code: P0171, description: \"a [b] {{c}}\" }}
"
    );
    assert!(parse_str("ok.yaml", &ok).is_ok());
}
