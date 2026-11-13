//! Security and behaviour tests for the pure request handler.

use opendiag_core::{Error, Result};
use opendiag_services::demo::{virtual_kw1281_module, virtual_uds_client};
use opendiag_services::{DtcReport, FaultAccess, Ident, Module, WriteToken};
use opendiag_web::{App, Info, Request, Response, SECURITY_HEADERS};

const TOKEN: &str = "s3cret-token";
const PORT: u16 = 4242;
const HOST: &str = "127.0.0.1:4242";

fn info(simulated: bool) -> Info {
    Info {
        version: "test".into(),
        connection: "test connection".into(),
        simulated,
        module: "test module".into(),
    }
}

fn app_with(module: Box<dyn Module>) -> App {
    App::new(module, None, info(true), TOKEN.into(), PORT)
}

fn kw_app() -> App {
    app_with(Box::new(virtual_kw1281_module().expect("virtual module")))
}

fn get(app: &mut App, target: &str, host: Option<&str>, token: Option<&str>) -> Response {
    app.handle(&Request {
        method: "GET",
        target,
        host,
        token_header: token,
    })
}

fn api(app: &mut App, target: &str) -> Response {
    get(app, target, Some(HOST), Some(TOKEN))
}

/// A module whose session is gone: everything fails.
struct Dead;
impl Module for Dead {
    fn identify(&mut self) -> Result<Ident> {
        Err(Error::Timeout)
    }
    fn read_dtcs(&mut self) -> Result<Vec<DtcReport>> {
        Err(Error::Timeout)
    }
    fn faults(&mut self) -> &mut dyn FaultAccess {
        self
    }
    fn keep_alive(&mut self) -> Result<()> {
        Err(Error::Timeout)
    }
}
impl FaultAccess for Dead {
    fn protocol(&self) -> &'static str {
        "dead"
    }
    fn module_address(&self) -> u8 {
        0
    }
    fn describe(&self) -> String {
        "dead".into()
    }
    fn read(&mut self) -> Result<Vec<DtcReport>> {
        Err(Error::Timeout)
    }
    fn clear(&mut self, _t: &WriteToken) -> Result<()> {
        Err(Error::Timeout)
    }
}

#[test]
fn serves_index_only_with_the_token() {
    let mut app = kw_app();
    let ok = get(&mut app, &format!("/?t={TOKEN}"), Some(HOST), None);
    assert_eq!(ok.status, 200);
    assert!(ok.content_type.starts_with("text/html"));
    for bad in ["/", "/?t=", "/?t=wrong", "/?x=1"] {
        assert_eq!(get(&mut app, bad, Some(HOST), None).status, 401, "{bad}");
    }
}

#[test]
fn api_needs_the_token_header() {
    let mut app = kw_app();
    for t in [None, Some(""), Some("wrong"), Some("s3cret-toke")] {
        for path in ["/api/info", "/api/ident", "/api/dtcs", "/api/live"] {
            assert_eq!(
                get(&mut app, path, Some(HOST), t).status,
                401,
                "{path} {t:?}"
            );
        }
    }
    // The token in the query string is not accepted for the API.
    let r = get(&mut app, &format!("/api/info?t={TOKEN}"), Some(HOST), None);
    assert_eq!(r.status, 401);
}

#[test]
fn wrong_host_is_refused_even_with_the_right_token() {
    let mut app = kw_app();
    for host in [
        None,
        Some("evil.example"),
        Some("evil.example:4242"),
        Some("127.0.0.1"),
        Some("127.0.0.1:9999"),
        Some("localhost"),
        Some("127.0.0.1:4242.evil.example"),
        Some("[::1]:4242"),
    ] {
        let r = get(&mut app, "/api/info", host, Some(TOKEN));
        assert_eq!(r.status, 403, "{host:?}");
        let r = get(&mut app, &format!("/?t={TOKEN}"), host, None);
        assert_eq!(r.status, 403, "{host:?}");
    }
    for host in ["localhost:4242", "LOCALHOST:4242", "127.0.0.1:4242"] {
        assert_eq!(
            get(&mut app, "/api/info", Some(host), Some(TOKEN)).status,
            200
        );
    }
}

#[test]
fn only_get_is_allowed() {
    let mut app = kw_app();
    for method in ["POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD", "get"] {
        for target in ["/", "/api/info", "/api/dtcs", "/api/clear", "/api/live"] {
            let r = app.handle(&Request {
                method,
                target,
                host: Some(HOST),
                token_header: Some(TOKEN),
            });
            assert_eq!(r.status, 405, "{method} {target}");
        }
    }
}

#[test]
fn unknown_paths_are_404_and_there_is_no_write_endpoint() {
    let mut app = kw_app();
    for p in [
        "/nope",
        "/api/",
        "/api/clear",
        "/api/dtcs/clear",
        "/api/adaptation",
        "/api/write",
        "/../etc/passwd",
        "//",
    ] {
        let r = api(&mut app, p);
        assert!(r.status == 404 || r.status == 401, "{p} -> {}", r.status);
        assert_ne!(r.status, 200, "{p}");
    }
}

#[test]
fn every_response_carries_the_security_headers_contract() {
    // The handler returns bodies; the CLI adds SECURITY_HEADERS to all of them.
    // Pin the contract here so it cannot be weakened unnoticed.
    let h: std::collections::HashMap<_, _> = SECURITY_HEADERS.iter().copied().collect();
    let csp = h["Content-Security-Policy"];
    assert!(csp.contains("default-src 'none'"));
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(!csp.contains("unsafe-inline"));
    assert!(!csp.contains("unsafe-eval"));
    assert!(!csp.contains("http"), "no external origins");
    assert_eq!(h["X-Content-Type-Options"], "nosniff");
    assert_eq!(h["Cache-Control"], "no-store");
    assert_eq!(h["Referrer-Policy"], "no-referrer");
}

#[test]
fn page_assets_contain_no_inline_script_and_no_html_injection() {
    let mut app = kw_app();
    let html = get(&mut app, &format!("/?t={TOKEN}"), Some(HOST), None).body_text();
    assert!(!html.contains("onclick") && !html.contains("javascript:"));
    // Only external script tags with src.
    for part in html.split("<script").skip(1) {
        assert!(part.trim_start().starts_with("src="), "inline script found");
    }
    let js = get(&mut app, "/app.js", Some(HOST), None).body_text();
    for banned in [
        "innerHTML",
        "outerHTML",
        "insertAdjacentHTML",
        "document.write",
        "eval(",
        "new Function",
    ] {
        assert!(!js.contains(banned), "app.js uses {banned}");
    }
    assert_eq!(get(&mut app, "/app.css", Some(HOST), None).status, 200);
}

#[test]
fn reads_identification_and_codes_from_the_virtual_module() {
    let mut app = kw_app();
    let info = api(&mut app, "/api/info");
    assert_eq!(info.status, 200);
    assert!(info.body_text().contains("\"simulated\":true"));
    let ident = api(&mut app, "/api/ident");
    assert_eq!(ident.status, 200);
    assert!(
        ident.body_text().contains("VIRTUAL ENGINE ECU"),
        "{}",
        ident.body_text()
    );
    let dtcs = api(&mut app, "/api/dtcs");
    assert_eq!(dtcs.status, 200);
    assert!(dtcs.body_text().starts_with('['));
}

#[test]
fn live_groups_are_validated() {
    let mut app = kw_app();
    assert_eq!(api(&mut app, "/api/live?groups=1").status, 200);
    assert_eq!(api(&mut app, "/api/live").status, 200);
    for bad in ["", "256", "a", "1,,2", "1,2,3,4,5,6,7,8,9", "-1"] {
        let r = api(&mut app, &format!("/api/live?groups={bad}"));
        assert_eq!(r.status, 400, "{bad:?}");
    }
}

#[test]
fn live_data_is_501_for_modules_without_it() {
    let mut app = app_with(Box::new(virtual_uds_client()));
    assert_eq!(api(&mut app, "/api/live?groups=1").status, 501);
}

#[test]
fn a_lost_session_answers_503_instead_of_talking_into_it() {
    let mut app = app_with(Box::new(Dead));
    assert!(app.is_alive());
    app.tick();
    assert!(!app.is_alive());
    assert_eq!(api(&mut app, "/api/dtcs").status, 503);
    // The page itself and the token rules still apply.
    assert_eq!(get(&mut app, "/api/dtcs", Some(HOST), None).status, 401);
}

#[test]
fn module_errors_become_502() {
    let mut app = app_with(Box::new(Dead));
    assert_eq!(api(&mut app, "/api/ident").status, 502);
    assert_eq!(api(&mut app, "/api/dtcs").status, 502);
}

#[test]
fn a_healthy_module_stays_alive_across_ticks() {
    let mut app = kw_app();
    for _ in 0..5 {
        app.tick();
    }
    assert!(app.is_alive());
    assert_eq!(api(&mut app, "/api/dtcs").status, 200);
}

// ---- fuzzing: no request that breaks a rule may ever be served ----

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> usize {
        (self.next() % n) as usize
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len() as u64)]
    }
    fn junk(&mut self) -> String {
        (0..self.below(30))
            .map(|_| char::from_u32(self.next() as u32 % 0x800).unwrap_or('?'))
            .collect()
    }
}

#[test]
fn random_requests_are_never_served_unless_every_rule_holds() {
    let mut app = kw_app();
    let mut rng = Rng(99);
    let paths = [
        "/",
        "/api/info",
        "/api/ident",
        "/api/dtcs",
        "/api/live",
        "/api/clear",
        "/app.js",
        "/app.css",
        "/api/",
        "//api/info",
        "/api/info/",
        "/API/INFO",
        "/api/../",
        "/%2e%2e/",
    ];
    let queries = [
        "",
        "?t=",
        "?t=s3cret-token",
        "?t=s3cret-token&x=1",
        "?groups=1",
        "?groups=%31",
        "?t=S3CRET-TOKEN",
    ];
    let methods = [
        "GET", "POST", "PUT", "DELETE", "HEAD", "OPTIONS", "get", "", "CONNECT",
    ];
    let hosts = [
        None,
        Some("127.0.0.1:4242"),
        Some("localhost:4242"),
        Some("127.0.0.1"),
        Some("evil.example"),
        Some("127.0.0.1:4242@evil.example"),
        Some("127.0.0.1:4242, evil.example"),
    ];
    let mut served = 0;
    for _ in 0..20_000 {
        let target = if rng.below(4) == 0 {
            format!("{}{}", rng.junk(), rng.junk())
        } else {
            format!("{}{}", rng.pick(&paths), rng.pick(&queries))
        };
        let method = rng.pick(&methods);
        let host = hosts[rng.below(hosts.len() as u64)];
        let junk_token = rng.junk();
        let token = match rng.below(4) {
            0 => None,
            1 => Some(TOKEN),
            2 => Some(junk_token.as_str()),
            _ => Some(""),
        };
        let r = app.handle(&Request {
            method,
            target: &target,
            host,
            token_header: token,
        });
        if r.status == 200 {
            served += 1;
            let host_ok = matches!(host, Some("127.0.0.1:4242" | "localhost:4242"));
            assert!(host_ok, "served with host {host:?}: {target:?}");
            assert_eq!(method, "GET", "served a {method} request: {target:?}");
            let path = target.split('?').next().unwrap();
            if path == "/" {
                assert!(target.contains("t=s3cret-token"), "{target:?}");
            } else if path.starts_with("/api/") {
                assert_eq!(token, Some(TOKEN), "{target:?}");
            } else {
                assert!(path == "/app.js" || path == "/app.css", "{target:?}");
            }
        }
    }
    // The test must actually exercise the success path.
    assert!(served > 50, "only {served} requests were served");
}
