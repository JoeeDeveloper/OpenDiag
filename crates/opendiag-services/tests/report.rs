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

#[test]
fn the_report_warns_about_privacy_and_verification() {
    let h = render_html(&base());
    assert!(h.contains("VIN") && h.contains("before sharing"));
    assert!(h.contains("not yet been verified against real vehicles"));
}
