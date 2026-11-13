# Releasing OpenDiag

Releases are built by GitHub Actions (`.github/workflows/release.yml`). Nothing is built or uploaded by hand.

## What a release contains

One archive per platform, each holding the `opendiag` binary, the licence, the README, `docs/` and `data/` (the
open data files the CLI reads to turn trouble codes into descriptions):

| Archive | Platform |
|---|---|
| `opendiag-<version>-x86_64-unknown-linux-musl.tar.gz` | Linux, any distribution (static binary) |
| `opendiag-<version>-x86_64-pc-windows-msvc.zip` | Windows 10/11 |
| `opendiag-<version>-aarch64-apple-darwin.tar.gz` | macOS on Apple Silicon |
| `opendiag-<version>-x86_64-apple-darwin.tar.gz` | macOS on Intel (cross-compiled; smoke-tested only best-effort) |

plus a `SHA256SUMS` file. Unpack an archive and run `opendiag` from inside the unpacked folder (it looks for
`./data` there). Releases whose version starts with `0.` are marked as pre-releases.

## Cutting a release

1. Make sure `main` is green and `docs/status.md` is accurate. Say honestly what is and is not verified.
2. Bump `version` under `[workspace.package]` in `Cargo.toml`, commit, and push.
3. Tag and push the tag. The tag must be `v` plus exactly that version, or the workflow stops at the first step:
   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```
4. The workflow builds all four targets, **smoke-tests each package** against the built-in virtual modules
   (`scripts/smoke-test.sh`), checksums them, and publishes the release with generated notes.
5. Download one archive and try it before announcing anything.

## Trying the pipeline without publishing

Run the workflow by hand (Actions tab, "Release", "Run workflow", or `gh workflow run release.yml`). It does
everything except publish: you get the packages as downloadable artifacts of that run.

The same scripts run locally (from the repository root, in Git Bash on Windows):

```bash
cargo build --release -p opendiag-cli
scripts/stage-release.sh target/release/opendiag dist/opendiag-test        # opendiag.exe on Windows
scripts/smoke-test.sh dist/opendiag-test opendiag                          # opendiag.exe on Windows
```

## What the smoke test checks

It unpacks nothing clever: it runs the staged binary from inside its folder, like a user would, and checks scans
on the virtual UDS, KW1281 and KWP2000 modules, fault codes with descriptions from the **shipped** data files,
data validation, the HTML report (including its "SIMULATED DATA" label and script-blocking policy), live-data
CSV, `ports`, a complete guarded adaptation change (confirm, backup, verify, list), and that declining a clear
changes nothing and writes no backup. It needs no hardware. It cannot say anything about real vehicles.

## Known limitations

- **The binaries are not code-signed.** Windows SmartScreen and macOS Gatekeeper will warn. On macOS you may need
  to clear the quarantine flag (`xattr -d com.apple.quarantine opendiag`) or right-click, Open. Signing needs a
  certificate and an Apple developer account, which this project does not have yet. Verify downloads against
  `SHA256SUMS` instead.
- **No installer, package manager entry or auto-update.** Archives only.
- **The Intel macOS build is cross-compiled** on an Apple Silicon runner. Its smoke test runs under Rosetta on a
  best-effort basis (a failure there does not fail the release). It passed in the first dry run.
- **The publish step has never run.** The build, smoke-test, archive and checksum steps were verified by a dry
  run, but publishing only happens for a pushed tag, which has not been done yet. Expect to fix small things
  the first time (for example release-note formatting) and check the first real release before announcing it.
- GitHub warns that some actions (`checkout`, `upload-artifact`) target Node.js 20, which is deprecated. They
  still work; bump their versions when newer majors are available.
- **Linux builds are x86_64 only**, and serial port discovery uses the kernel's device list because the build has
  no libudev dependency. ARM Linux (for example a Raspberry Pi) is not built yet.
- **Release notes cannot promise the software works on a vehicle.** It has only ever run against simulators.
