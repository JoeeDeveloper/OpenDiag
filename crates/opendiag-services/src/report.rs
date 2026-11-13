//! A self-contained HTML report of a module's identification and fault codes.
//!
//! Safety properties, because the text comes from vehicles and data files:
//! every string is HTML-escaped and stripped of control characters; the page
//! contains no JavaScript and loads nothing from the network; and a
//! Content-Security-Policy forbids both anyway, so even a missed escape could
//! not run code. Simulated data is labelled prominently.

use crate::Ident;

/// One fault code as shown in the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportFault {
    /// Code as displayed (`P0171-00`, `00525`).
    pub text: String,
    /// Raw status byte. Its meaning is module-specific for older protocols.
    pub status: u8,
    pub flags: Vec<String>,
    /// Description from the open data set, when it has one.
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub generated_unix: u64,
    pub tool_version: String,
    /// How the data was obtained, e.g. `K-line COM3 at 10400 baud, KW1281, module 0x01`.
    pub connection: String,
    /// True when the data came from a built-in virtual module, not a vehicle.
    pub simulated: bool,
    /// Module description, e.g. `KW1281 module 0x01`.
    pub module: Option<String>,
    pub ident: Ident,
    pub faults: Vec<ReportFault>,
}

/// `YYYY-MM-DD HH:MM:SS` in UTC from Unix seconds (civil-from-days; no date crate needed).
pub fn format_unix_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Escape text for HTML. Control characters (including newlines and tabs) become
/// spaces, so nothing can break out of an element or fake extra lines.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

const STYLE: &str = "\
:root{--bg:#fff;--fg:#1a1a1a;--muted:#5b6470;--line:#d7dce2;--accent:#0b5cad;--warn-bg:#fff4d6;--warn-fg:#6b4a00;--head:#f3f5f8}\
@media (prefers-color-scheme:dark){:root{--bg:#14171c;--fg:#e8eaed;--muted:#9aa4b2;--line:#2c333d;--accent:#6db3f2;--warn-bg:#3a2f0e;--warn-fg:#ffd978;--head:#1c2128}}\
*{box-sizing:border-box}\
body{margin:0;padding:24px 16px;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}\
main{max-width:860px;margin:0 auto}\
h1{font-size:1.5rem;margin:0 0 4px}h2{font-size:1.1rem;margin:28px 0 8px;border-bottom:1px solid var(--line);padding-bottom:4px}\
.sub{color:var(--muted);margin:0 0 16px}\
.banner{background:var(--warn-bg);color:var(--warn-fg);border:1px solid var(--warn-fg);border-radius:6px;padding:10px 12px;margin:0 0 16px;font-weight:600}\
table{width:100%;border-collapse:collapse}\
th,td{text-align:left;padding:6px 8px;border-bottom:1px solid var(--line);vertical-align:top}\
th{background:var(--head);font-weight:600}\
caption{text-align:left;color:var(--muted);padding:0 0 6px}\
code{font-family:ui-monospace,SFMono-Regular,Consolas,monospace}\
dl{display:grid;grid-template-columns:max-content 1fr;gap:4px 16px;margin:0}dt{color:var(--muted)}dd{margin:0;overflow-wrap:anywhere}\
ul{padding-left:20px}li{margin:2px 0}.muted{color:var(--muted)}\
@media print{body{padding:0}.banner{border-width:2px}a{color:inherit}}";

fn row(label: &str, value: &str) -> String {
    format!(
        "<dt>{}</dt><dd>{}</dd>",
        escape_html(label),
        escape_html(value)
    )
}

/// Render the report as a complete, standalone HTML document.
pub fn render_html(r: &Report) -> String {
    let mut h = String::with_capacity(4096);
    h.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    // Forbid scripts and every external load, whatever the content turns out to be.
    h.push_str(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\">\n",
    );
    h.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n");
    h.push_str("<title>OpenDiag diagnostic report</title>\n");
    h.push_str(&format!(
        "<style>{STYLE}</style>\n</head>\n<body>\n<main>\n"
    ));

    h.push_str("<h1>OpenDiag diagnostic report</h1>\n");
    h.push_str(&format!(
        "<p class=\"sub\">Generated {} UTC</p>\n",
        escape_html(&format_unix_utc(r.generated_unix))
    ));
    if r.simulated {
        h.push_str(
            "<p class=\"banner\" role=\"note\">SIMULATED DATA: this report comes from a built-in virtual module, not from a vehicle.</p>\n",
        );
    }

    h.push_str("<h2>Summary</h2>\n<dl>\n");
    h.push_str(&row("Connection", &r.connection));
    if let Some(m) = &r.module {
        h.push_str(&row("Module", m));
    }
    h.push_str(&row("OpenDiag version", &r.tool_version));
    h.push_str("\n</dl>\n");

    h.push_str("<h2>Identification</h2>\n");
    let id = &r.ident;
    let mut any = false;
    let mut dl = String::new();
    for (label, value) in [
        ("Part number", &id.part_number),
        ("Software version", &id.software_version),
        ("VIN", &id.vin),
    ] {
        if let Some(v) = value {
            dl.push_str(&row(label, v));
            any = true;
        }
    }
    // When a part number was picked out it is also the first free-form line.
    let skip = usize::from(id.part_number.is_some());
    for line in id.lines.iter().skip(skip) {
        dl.push_str(&row("Identification", line));
        any = true;
    }
    if any {
        h.push_str(&format!("<dl>\n{dl}\n</dl>\n"));
    } else {
        h.push_str("<p class=\"muted\">The module reported no identification.</p>\n");
    }

    h.push_str("<h2>Fault codes</h2>\n");
    if r.faults.is_empty() {
        h.push_str("<p>No trouble codes stored.</p>\n");
    } else {
        h.push_str(&format!(
            "<table>\n<caption>{} stored code(s)</caption>\n<thead><tr><th scope=\"col\">Code</th><th scope=\"col\">Description</th><th scope=\"col\">Status</th><th scope=\"col\">Flags</th></tr></thead>\n<tbody>\n",
            r.faults.len()
        ));
        for f in &r.faults {
            let desc = f.description.as_deref().map_or_else(
                || "<span class=\"muted\">not in the data set</span>".to_string(),
                escape_html,
            );
            let flags = if f.flags.is_empty() {
                "<span class=\"muted\">raw byte only</span>".to_string()
            } else {
                escape_html(&f.flags.join(", "))
            };
            h.push_str(&format!(
                "<tr><td><code>{}</code></td><td>{desc}</td><td><code>{:#04X}</code></td><td>{flags}</td></tr>\n",
                escape_html(&f.text),
                f.status
            ));
        }
        h.push_str("</tbody>\n</table>\n");
    }

    h.push_str("<h2>About this report</h2>\n<ul>\n");
    h.push_str("<li>Descriptions come from the OpenDiag open data set (public domain) and are only shown when it has an entry.</li>\n");
    h.push_str("<li>Status bytes of older modules (KW1281, KWP2000) are shown raw; their meaning is module-specific.</li>\n");
    h.push_str("<li>OpenDiag has not yet been verified against real vehicles. Treat this as a reading aid, not a diagnosis.</li>\n");
    h.push_str("<li>This report may contain a VIN and other identifying data. Review it before sharing.</li>\n");
    h.push_str("</ul>\n</main>\n</body>\n</html>\n");
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_times_format_as_utc_including_leap_edges() {
        for (secs, want) in [
            (0, "1970-01-01 00:00:00"),
            (86_399, "1970-01-01 23:59:59"),
            (951_782_400, "2000-02-29 00:00:00"), // leap day (divisible by 400)
            (1_709_164_800, "2024-02-29 00:00:00"), // leap day
            (4_107_542_400, "2100-03-01 00:00:00"), // 2100 is not a leap year
            (1_794_589_583, "2026-11-13 17:06:23"),
        ] {
            assert_eq!(format_unix_utc(secs), want, "{secs}");
        }
    }

    #[test]
    fn escaping_neutralises_markup_quotes_and_control_characters() {
        assert_eq!(
            escape_html("<script>alert(\"x\" & 'y')</script>"),
            "&lt;script&gt;alert(&quot;x&quot; &amp; &#39;y&#39;)&lt;/script&gt;"
        );
        assert_eq!(escape_html("a\nb\tc\u{1b}[31md\r"), "a b c [31md ");
        assert_eq!(escape_html("Ünïcode ✓ stays"), "Ünïcode ✓ stays");
    }
}
