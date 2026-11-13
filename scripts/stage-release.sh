#!/usr/bin/env bash
# Assemble a release directory: the binary, licence, README, docs and open data.
#
# Usage (from the repository root):
#   scripts/stage-release.sh <path to built binary> <output directory>
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <path to built binary> <output directory>" >&2
  exit 2
fi
bin="$1"
out="$2"

[ -f "$bin" ] || { echo "binary not found: $bin" >&2; exit 1; }
for f in LICENSE README.md; do
  [ -f "$f" ] || { echo "run this from the repository root ($f not found)" >&2; exit 1; }
done
[ -d docs ] && [ -d data ] || { echo "docs/ and data/ must exist" >&2; exit 1; }

rm -rf "$out"
mkdir -p "$out"
cp "$bin" "$out/"
chmod +x "$out/$(basename "$bin")"
cp LICENSE README.md "$out/"
cp -R docs "$out/docs"
# The data files are what turn trouble codes into descriptions; the CLI finds
# them in ./data, so they ship next to the binary.
cp -R data "$out/data"

echo "staged $(basename "$out"):"
( cd "$out" && find . -maxdepth 1 | sort )
