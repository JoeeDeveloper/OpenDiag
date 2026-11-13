#!/usr/bin/env bash
# Run a staged release the way a user would, against the built-in virtual modules.
# Needs no hardware. Fails loudly on the first problem.
#
# Usage:
#   scripts/smoke-test.sh <staged directory> <binary name> [runner prefix]
# The optional runner prefix wraps the binary (e.g. "arch -x86_64" to run an
# Intel Mac build under Rosetta).
set -euo pipefail

if [ "$#" -lt 2 ]; then
  echo "usage: $0 <staged directory> <binary name> [runner prefix]" >&2
  exit 2
fi
dir="$1"
bin="$2"
prefix="${3:-}"

fail() {
  echo "SMOKE TEST FAILED: $*" >&2
  exit 1
}

# Run from inside the staged directory, as a user who unpacked the archive would,
# so the ./data directory shipped next to the binary is what gets found.
cd "$dir"
[ -f "$bin" ] || fail "binary $bin is not in $dir"
[ -d data ] && [ -f LICENSE ] && [ -f README.md ] && [ -d docs ] || fail "release contents are incomplete"

run() {
  if [ -n "$prefix" ]; then
    $prefix "./$bin" "$@"
  else
    "./$bin" "$@"
  fi
}

# Capture output first, then match: piping into `grep -q` with pipefail can fail
# spuriously when grep exits early.
expect() { # expect <description> <output> <substring>
  case "$2" in
    *"$3"*) echo "ok: $1" ;;
    *) echo "--- output was:" >&2; echo "$2" >&2; fail "$1 (missing: $3)" ;;
  esac
}

out=$(run --version) || fail "--version did not run"
case "$out" in
  opendiag\ [0-9]*) echo "ok: --version ($out)" ;;
  *) fail "unexpected --version output: $out" ;;
esac

out=$(run scan --simulate-kw1281 2>/dev/null) || fail "scan --simulate-kw1281"
expect "KW1281 scan" "$out" "DEMO-KW-0001"

out=$(run scan --simulate-kwp 2>/dev/null) || fail "scan --simulate-kwp"
expect "KWP2000 scan" "$out" "VIRTUAL KWP2000 ECU"

out=$(run dtc read --simulate 2>/dev/null) || fail "dtc read --simulate"
expect "UDS fault codes" "$out" "P0171-00"

out=$(run dtc read --simulate-elm327 2>/dev/null) || fail "dtc read --simulate-elm327"
expect "UDS fault codes through the software ELM327" "$out" "P0171-00"
# Descriptions come from the data files shipped in ./data.
expect "descriptions from shipped data" "$out" "Fuel mixture too lean on bank 1"

out=$(run data validate data 2>&1) || fail "data validate"
expect "shipped data validates" "$out" "OK:"

run report --simulate-kw1281 --out smoke-report.html >/dev/null 2>&1 || fail "report"
[ -s smoke-report.html ] || fail "report file is empty"
out=$(cat smoke-report.html)
expect "report is labelled simulated" "$out" "SIMULATED DATA"
expect "report forbids scripts" "$out" "default-src 'none'"
rm -f smoke-report.html

out=$(run log --simulate-kw1281 --groups 1 --samples 2 --interval-ms 10 2>/dev/null) || fail "log"
expect "live data CSV header" "$out" "time_ms,group,cell,formula,a,b"

run ports >/dev/null 2>&1 || fail "ports must work without any hardware"
echo "ok: ports"

# The write path, end to end: confirm, backup, verify, list.
rm -f smoke-backups.jsonl
out=$(printf 'YES\n' | run adaptation set --simulate-kw1281 --channel 4 --value 200 \
  --workshop-code 0 --backup-file smoke-backups.jsonl 2>/dev/null) || fail "adaptation set"
expect "guarded adaptation change verifies" "$out" "Done and verified"
[ -s smoke-backups.jsonl ] || fail "no backup file was written"
out=$(run backup list --backup-file smoke-backups.jsonl 2>/dev/null) || fail "backup list"
expect "backup is listed as verified" "$out" "Verified"

# Declining must change nothing and write no backup.
rm -f smoke-backups.jsonl
out=$(printf 'no\n' | run dtc clear --simulate-kw1281 --backup-file smoke-backups.jsonl 2>/dev/null) \
  || fail "dtc clear (declined)"
expect "declining the clear cancels" "$out" "Cancelled"
[ ! -e smoke-backups.jsonl ] || fail "a declined clear wrote a backup file"

rm -f smoke-backups.jsonl
echo "SMOKE TEST PASSED ($bin)"
