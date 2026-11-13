//! A read-only web interface for a connected module.
//!
//! This crate is only the request handler: a pure `handle(request) -> response`
//! with no sockets, so every security rule is testable. The CLI wraps it in a
//! tiny HTTP server bound to `127.0.0.1`.
//!
//! Threat model: the server talks to a vehicle's diagnostic port, so a web page
//! the user happens to visit must not be able to read from or drive it. Defences:
//!
//! - **Host check** (DNS rebinding): only `127.0.0.1:<port>` and `localhost:<port>` are served.
//! - **Per-run token** in a custom `X-OpenDiag-Token` header for every API call. A
//!   cross-origin page cannot send a custom header without a CORS preflight, and
//!   no preflight is ever answered.
//! - **Read-only:** `GET` only, and no endpoint modifies anything. Clearing codes
//!   and adaptation changes stay in the guarded CLI.
//! - **Strict CSP** and `nosniff` on every response; the page renders module text
//!   with `textContent` only, never as HTML.

use std::time::{SystemTime, UNIX_EPOCH};

use opendiag_data::DataSet;
use opendiag_services::Module;
use serde::Serialize;

const INDEX_HTML: &str = include_str!("index.html");
const APP_JS: &str = include_str!("app.js");
const APP_CSS: &str = include_str!("app.css");

/// Headers added to every response.
pub const SECURITY_HEADERS: [(&str, &str); 5] = [
    (
        "Content-Security-Policy",
        "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; \
         img-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
    ),
    ("X-Content-Type-Options", "nosniff"),
    ("Cache-Control", "no-store"),
    ("Referrer-Policy", "no-referrer"),
    ("Cross-Origin-Resource-Policy", "same-origin"),
];

/// The parts of an HTTP request the handler looks at.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub method: &'a str,
    /// Path plus optional query, as in the request line (`/api/live?groups=1`).
    pub target: &'a str,
    pub host: Option<&'a str>,
    /// Value of the `X-OpenDiag-Token` header.
    pub token_header: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    fn json(status: u16, value: &impl Serialize) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
        }
    }

    fn error(status: u16, message: &str) -> Self {
        #[derive(Serialize)]
        struct E<'a> {
            error: &'a str,
        }
        Self::json(status, &E { error: message })
    }

    fn asset(content_type: &'static str, body: &str) -> Self {
        Self {
            status: 200,
            content_type,
            body: body.as_bytes().to_vec(),
        }
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Constant-time comparison, so response timing does not reveal how much of the
/// token was right.
fn token_matches(given: Option<&str>, expected: &str) -> bool {
    let Some(given) = given else { return false };
    let (a, b) = (given.as_bytes(), expected.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

/// How the connection was made, shown at the top of the page.
#[derive(Debug, Clone, Serialize)]
pub struct Info {
    pub version: String,
    pub connection: String,
    /// True for the built-in virtual modules: the page shows a banner.
    pub simulated: bool,
    pub module: String,
}

pub struct App {
    module: Box<dyn Module>,
    data: Option<DataSet>,
    info: Info,
    token: String,
    port: u16,
    /// Set when keep-alive fails: the session is gone and requests are refused.
    alive: bool,
}

impl App {
    pub fn new(
        module: Box<dyn Module>,
        data: Option<DataSet>,
        info: Info,
        token: String,
        port: u16,
    ) -> Self {
        Self {
            module,
            data,
            info,
            token,
            port,
            alive: true,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive
    }

    /// Call periodically while idle. If the session cannot be kept alive, later API
    /// requests are refused instead of talking into a dead session.
    pub fn tick(&mut self) {
        if self.alive && self.module.keep_alive().is_err() {
            self.alive = false;
        }
    }

    fn host_allowed(&self, host: Option<&str>) -> bool {
        let Some(h) = host else { return false };
        let h = h.to_ascii_lowercase();
        h == format!("127.0.0.1:{}", self.port) || h == format!("localhost:{}", self.port)
    }

    pub fn handle(&mut self, req: &Request<'_>) -> Response {
        if !self.host_allowed(req.host) {
            return Response::error(403, "unexpected Host header");
        }
        if req.method != "GET" {
            return Response::error(405, "this interface is read-only; only GET is allowed");
        }
        let (path, query) = req.target.split_once('?').unwrap_or((req.target, ""));

        match path {
            "/app.js" => Response::asset("text/javascript; charset=utf-8", APP_JS),
            "/app.css" => Response::asset("text/css; charset=utf-8", APP_CSS),
            "/" => {
                if token_matches(query_param(query, "t"), &self.token) {
                    Response::asset("text/html; charset=utf-8", INDEX_HTML)
                } else {
                    Response::error(
                        401,
                        "missing or wrong token: open the URL printed when `opendiag serve` started",
                    )
                }
            }
            p if p.starts_with("/api/") => {
                if !token_matches(req.token_header, &self.token) {
                    return Response::error(401, "missing or wrong token");
                }
                self.api(p, query)
            }
            _ => Response::error(404, "not found"),
        }
    }

    fn api(&mut self, path: &str, query: &str) -> Response {
        if !self.alive {
            return Response::error(
                503,
                "the connection to the module was lost; restart `opendiag serve`",
            );
        }
        match path {
            "/api/info" => Response::json(200, &self.info),
            "/api/ident" => match self.module.identify() {
                Ok(id) => Response::json(
                    200,
                    &IdentJson {
                        part_number: id.part_number,
                        software_version: id.software_version,
                        vin: id.vin,
                        lines: id.lines,
                    },
                ),
                Err(e) => Response::error(502, &e.to_string()),
            },
            "/api/dtcs" => match self.module.read_dtcs() {
                Ok(list) => {
                    let data = self.data.as_ref();
                    let faults: Vec<FaultJson> = list
                        .into_iter()
                        .map(|d| FaultJson {
                            description: data
                                .and_then(|s| s.describe_displayed_code(&d.text))
                                .map(str::to_string),
                            flags: d.flags.iter().map(|f| (*f).to_string()).collect(),
                            text: d.text,
                            status: d.status,
                        })
                        .collect();
                    Response::json(200, &faults)
                }
                Err(e) => Response::error(502, &e.to_string()),
            },
            "/api/live" => self.live(query),
            _ => Response::error(404, "not found"),
        }
    }

    fn live(&mut self, query: &str) -> Response {
        let Some(groups) = parse_groups(query_param(query, "groups").unwrap_or("1")) else {
            return Response::error(
                400,
                "groups must be 1 to 8 comma-separated numbers from 0 to 255",
            );
        };
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let Some(live) = self.module.live() else {
            return Response::error(501, "live data is only available for KW1281 modules so far");
        };
        let mut out = Vec::new();
        for g in groups {
            match live.read_group(g) {
                Ok(cells) => out.push(GroupJson {
                    group: g,
                    cells: cells
                        .into_iter()
                        .map(|c| CellJson {
                            formula: c.formula,
                            a: c.a,
                            b: c.b,
                        })
                        .collect(),
                }),
                Err(e) => return Response::error(502, &e.to_string()),
            }
        }
        Response::json(
            200,
            &LiveJson {
                unix_ms: started,
                groups: out,
            },
        )
    }
}

/// Value of `key` in a query string (`a=1&b=2`). No percent-decoding: the only
/// values this interface accepts are plain tokens, digits and commas.
fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// `1,2,3` -> groups; 1 to 8 distinct-or-not numbers, each 0..=255.
fn parse_groups(s: &str) -> Option<Vec<u8>> {
    let groups: Option<Vec<u8>> = s.split(',').map(|p| p.trim().parse::<u8>().ok()).collect();
    groups.filter(|g| (1..=8).contains(&g.len()))
}

#[derive(Serialize)]
struct IdentJson {
    part_number: Option<String>,
    software_version: Option<String>,
    vin: Option<String>,
    lines: Vec<String>,
}

#[derive(Serialize)]
struct FaultJson {
    text: String,
    status: u8,
    flags: Vec<String>,
    description: Option<String>,
}

#[derive(Serialize)]
struct CellJson {
    formula: u8,
    a: u8,
    b: u8,
}

#[derive(Serialize)]
struct GroupJson {
    group: u8,
    cells: Vec<CellJson>,
}

#[derive(Serialize)]
struct LiveJson {
    unix_ms: u128,
    groups: Vec<GroupJson>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comparison_is_exact() {
        assert!(token_matches(Some("abc"), "abc"));
        for bad in [
            None,
            Some(""),
            Some("abd"),
            Some("ab"),
            Some("abcd"),
            Some("ABC"),
        ] {
            assert!(!token_matches(bad, "abc"), "{bad:?}");
        }
    }

    #[test]
    fn query_and_group_parsing() {
        assert_eq!(query_param("t=x&groups=1,2", "groups"), Some("1,2"));
        assert_eq!(query_param("t=x", "groups"), None);
        assert_eq!(query_param("", "t"), None);
        assert_eq!(parse_groups("1,2,3"), Some(vec![1, 2, 3]));
        assert_eq!(parse_groups("255"), Some(vec![255]));
        for bad in ["", "256", "-1", "a", "1,,2", "1,2,3,4,5,6,7,8,9", "1.5"] {
            assert_eq!(parse_groups(bad), None, "{bad:?}");
        }
    }
}
