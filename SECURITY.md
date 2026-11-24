# Security policy

## Reporting a vulnerability

Please report security issues privately through GitHub's
[private vulnerability reporting](https://github.com/JoeeDeveloper/OpenDiag/security/advisories/new)
rather than a public issue. Include the version, platform and steps to reproduce.

This is a hobby project with no fixed response time; reports are handled on a best-effort basis.

## Scope

In scope: memory-safety or parsing bugs in the protocol decoders, flaws in the guarded write flow
(a write that skips the durable backup, confirmation or read-back), and the `serve` HTTP interface.

Out of scope: requests to bypass immobiliser, odometer or tuning protection, or to add flashing support.
These are refused by design (see [docs/safety.md](docs/safety.md)).

## Status

OpenDiag has not been verified against real vehicles ([docs/status.md](docs/status.md)). Do not rely
on it for safety-critical decisions, and use write functions only on a bench module you can afford to lose.
