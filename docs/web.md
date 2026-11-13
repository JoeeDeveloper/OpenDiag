# Local web interface

`opendiag serve <same connection options as scan>` connects to the module, then serves a page on
`127.0.0.1` (this computer only). The first line printed is the URL to open; it contains a random token that
changes every run. Stop with Ctrl-C (the session is closed on exit).

It is **read-only**: identification, fault codes (with descriptions from the open data files when present) and
raw live data. Clearing codes and adaptation changes are not offered, because they must go through the guarded
backup-and-confirm flow in the command line (see [safety.md](safety.md)).

## Why it is built this way

The server is attached to a vehicle's diagnostic port, so a web page you happen to visit must not be able to read
from it. Every request goes through one pure function (`opendiag-web`), which enforces:

- **Host check:** only `127.0.0.1:<port>` and `localhost:<port>`. Defeats DNS rebinding.
- **Token:** required in an `X-OpenDiag-Token` header for `/api/*` (and as `?t=` for the first page load). A
  foreign page cannot send a custom header without a CORS preflight, and none is ever answered.
- **GET only**, no endpoint that changes anything.
- **Strict CSP** (no inline script, no external origins), `nosniff`, `no-store`, no referrer. The page code puts
  module text into the page with `textContent` only; a test bans `innerHTML` and friends.

Requests are handled one at a time, so the diagnostic session is never used concurrently. When idle, the server
sends keep-alive messages; if one fails, the API answers 503 until you restart.

## Limits

- Live data is KW1281 only, as raw cells (formula, A, B). Values are not converted to units, and the chart plots
  A×256+B.
- Anyone who can run programs as you on this computer can read the token from the process or the URL. It is not a
  defence against local malware.
- Simulator-tested only; see [status.md](status.md).
