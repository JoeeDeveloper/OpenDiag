use opendiag_services::{render_html, Ident, Report, ReportFault};

fn base() -> Report {
    Report {
        generated_unix: 1_794_589_583, // 2026-11-13 17:06:23 UTC
        tool_version: "0.0.1".into(),
        connection: "K-line COM3 at 10400 baud, KW1281 (5-baud init), module 0x01".into(),
        simulated: false,
        module: Some("KW1281 module 0x01".into()),
        ident: Ident {
            part_number: Some("1J0 906 018 AB".into()),
            lines: vec!["1J0 906 018 AB".into(), "1.8L R4/5VT".into()],
            ..Ident::default()
        },
        faults: vec![
            ReportFault {
                text: "P0171-00".into(),
                status: 0x09,
                flags: vec!["testFailed".into(), "confirmed".into()],
                description: Some("Fuel mixture too lean on bank 1".into()),
            },
            ReportFault {
                text: "00525".into(),
                status: 0x20,
                flags: vec![],
                description: None,
            },
        ],
    }
}

#[test]
fn a_report_is_a_complete_standalone_document_with_the_expected_content() {
    let h = render_html(&base());
    assert!(h.starts_with("<!doctype html>") && h.trim_end().ends_with("</html>"));
    assert!(h.contains("lang=\"en\"") && h.contains("<title>OpenDiag diagnostic report</title>"));
    assert!(h.contains("Generated 2026-11-13 17:06:23 UTC"));
    assert!(h.contains("K-line COM3 at 10400 baud, KW1281 (5-baud init), module 0x01"));
    assert!(h.contains("KW1281 module 0x01") && h.contains("0.0.1"));
    // Identification: the part number once, then the remaining free-form lines.
    assert_eq!(h.matches("1J0 906 018 AB").count(), 1, "not repeated");
    assert!(h.contains("1.8L R4/5VT"));
    // Fault table.
    assert!(h.contains("2 stored code(s)"));
    assert!(h.contains("<code>P0171-00</code>") && h.contains("Fuel mixture too lean on bank 1"));
    assert!(h.contains("<code>0x09</code>") && h.contains("testFailed, confirmed"));
    assert!(
        h.contains("<code>00525</code>")
            && h.contains("not in the data set")
            && h.contains("raw byte only")
    );
    assert!(
        h.contains("scope=\"col\"") && h.contains("<caption>"),
        "accessible table"
    );
}

#[test]
fn simulated_data_is_labelled_and_real_data_is_not() {
    let mut r = base();
    assert!(!render_html(&r).contains("SIMULATED DATA"));
    r.simulated = true;
    let h = render_html(&r);
    assert!(h.contains("SIMULATED DATA") && h.contains("not from a vehicle"));
    assert!(
        h.find("SIMULATED DATA").unwrap() < h.find("<h2>Summary</h2>").unwrap(),
        "banner comes first"
    );
}

#[test]
fn hostile_text_from_a_module_or_data_file_cannot_inject_markup() {
    let evil = "<script>alert(1)</script><img src=x onerror=alert(2)>\"'&";
    let mut r = base();
    r.connection = evil.into();
    r.module = Some(evil.into());
    r.tool_version = evil.into();
    r.ident = Ident {
        part_number: Some(evil.into()),
        software_version: Some(evil.into()),
        vin: Some(evil.into()),
        lines: vec![evil.into(), evil.into()],
    };
    r.faults = vec![ReportFault {
        text: evil.into(),
        status: 1,
        flags: vec![evil.into()],
        description: Some(evil.into()),
    }];
    let h = render_html(&r);
    assert!(!h.contains("<img"), "no tag survives");
    assert!(
        h.matches("<script").count() == 0,
        "no script element anywhere: {h}"
    );
    assert!(h.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(
        h.contains("&lt;img src=x onerror=alert(2)&gt;"),
        "defanged text stays visible"
    );
    assert!(
        !h.contains("onerror=alert(2)>"),
        "the tag is not closed as markup"
    );
}

#[test]
fn control_characters_and_terminal_escapes_are_removed() {
    let mut r = base();
    // No part number picked out, so every free-form line is shown.
    r.ident.part_number = None;
    r.ident.lines = vec!["line\u{1b}[2Jone\ntwo\u{7}".into(), "x".into()];
    r.faults[0].description = Some("a\u{0}b\r\nc".into());
    let h = render_html(&r);
    for bad in ['\u{1b}', '\u{7}', '\u{0}', '\r'] {
        assert!(!h.contains(bad), "{bad:?} must not appear");
    }
    assert!(
        h.contains("line [2Jone two ") && h.contains("a b  c"),
        "{h}"
    );
}

#[test]
fn the_page_cannot_run_script_or_load_anything_from_the_network() {
    let h = render_html(&base());
    // A policy that forbids everything but inline styling...
    assert!(h.contains("Content-Security-Policy"));
    assert!(h.contains("default-src 'none'; style-src 'unsafe-inline'"));
    // ...and no markup that would try.
    for needle in [
        "<script", "<link", "<iframe", "<object", "<embed", "<img", "src=", "href=", "http://",
        "https://", "@import", "url(",
    ] {
        assert!(!h.contains(needle), "unexpected `{needle}`");
    }
    assert!(!h.contains("javascript:"));
}

#[test]
fn empty_cases_say_so_instead_of_rendering_empty_tables() {
    let mut r = base();
    r.faults.clear();
    r.ident = Ident::default();
    let h = render_html(&r);
    assert!(h.contains("No trouble codes stored."));
    assert!(!h.contains("<table"), "no empty table");
    assert!(h.contains("The module reported no identification."));
}

#[test]
fn standard_identification_items_are_shown_when_present() {
    let mut r = base();
    r.ident = Ident {
        part_number: Some("P-1".into()),
        software_version: Some("SW 2".into()),
        vin: Some("WVWZZZ9NZ7Y123456".into()),
        lines: vec![],
    };
    let h = render_html(&r);
    assert!(h.contains("<dt>VIN</dt><dd>WVWZZZ9NZ7Y123456</dd>"));
    assert!(h.contains("<dt>Software version</dt><dd>SW 2</dd>"));
}

/// Every tag name the renderer itself emits.
const ALLOWED_TAGS: [&str; 26] = [
    "!doctype", "html", "head", "meta", "title", "style", "body", "main", "h1", "h2", "p", "dl",
    "dt", "dd", "table", "caption", "thead", "tbody", "tr", "th", "td", "code", "span", "ul", "li",
    "/",
];
/// Attribute names the renderer itself emits (always with fixed values).
const ALLOWED_ATTRS: [&str; 8] = [
    "lang",
    "charset",
    "http-equiv",
    "content",
    "name",
    "class",
    "scope",
    "role",
];

/// Parse every `<...>` in `html`; return a complaint for anything not emitted by the renderer.
fn find_foreign_markup(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else {
            return Some("an unterminated tag".into());
        };
        let tag = &after[..end];
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        let name = name.to_ascii_lowercase();
        if !ALLOWED_TAGS.contains(&name.as_str()) && !name.is_empty() {
            return Some(format!("unexpected tag <{tag}>"));
        }
        // Attribute names (skip the tags whose fixed attribute values contain spaces).
        if !matches!(name.as_str(), "meta" | "html" | "!doctype") {
            let mut attrs = tag[name.len().min(tag.len())..].trim();
            while let Some(eq) = attrs.find('=') {
                let attr = attrs[..eq]
                    .trim()
                    .rsplit(char::is_whitespace)
                    .next()
                    .unwrap_or("");
                if !ALLOWED_ATTRS.contains(&attr) {
                    return Some(format!("unexpected attribute `{attr}` in <{tag}>"));
                }
                let value_part = attrs[eq + 1..].trim_start();
                let Some(stripped) = value_part.strip_prefix('"') else {
                    return Some(format!("unquoted attribute value in <{tag}>"));
                };
                let Some(close) = stripped.find('"') else {
                    return Some(format!("unterminated attribute value in <{tag}>"));
                };
                attrs = stripped[close + 1..].trim_start();
            }
        }
        rest = &after[end + 1..];
    }
    None
}

#[test]
fn the_checker_itself_catches_injected_markup() {
    assert!(
        find_foreign_markup(&render_html(&base())).is_none(),
        "a clean report passes"
    );
    for bad in [
        "<b>x</b>",
        "<script>1</script>",
        "<td onclick=\"x\">",
        "<p class=muted>",
        "<img src=x>",
        "<td",
    ] {
        assert!(find_foreign_markup(bad).is_some(), "{bad}");
    }
}

#[test]
fn random_hostile_strings_never_produce_foreign_markup() {
    let alphabet: Vec<char> =
        "<>\"'&/=;:()abcdefOnonerror=ScRiPt \t\r\n\u{1b}\u{0}\u{7}%#!?javascript\u{e9}\u{2603}"
            .chars()
            .collect();
    let mut state: u64 = 0xDEC0DE;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    let text = move |next: &mut dyn FnMut() -> usize| {
        let len = next() % 40;
        (0..len)
            .map(|_| alphabet[next() % alphabet.len()])
            .collect::<String>()
    };
    for round in 0..3000 {
        let mut r = base();
        r.connection = text(&mut next);
        r.module = Some(text(&mut next));
        r.tool_version = text(&mut next);
        r.ident = Ident {
            part_number: Some(text(&mut next)),
            software_version: Some(text(&mut next)),
            vin: Some(text(&mut next)),
            lines: vec![text(&mut next), text(&mut next), text(&mut next)],
        };
        r.faults = (0..3)
            .map(|_| ReportFault {
                text: text(&mut next),
                status: next() as u8,
                flags: vec![text(&mut next)],
                description: Some(text(&mut next)),
            })
            .collect();
        r.simulated = round % 2 == 0;
        let html = render_html(&r);
        if let Some(problem) = find_foreign_markup(&html) {
            panic!("round {round}: {problem}");
        }
        for c in ['\u{1b}', '\u{0}', '\u{7}', '\r'] {
            assert!(
                !html.contains(c),
                "round {round}: control character {c:?} survived"
            );
        }
    }
}

#[test]
fn the_report_warns_about_privacy_and_verification() {
    let h = render_html(&base());
    assert!(h.contains("VIN") && h.contains("before sharing"));
    assert!(h.contains("not yet been verified against real vehicles"));
}
