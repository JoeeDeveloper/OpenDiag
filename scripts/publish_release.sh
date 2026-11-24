#!/usr/bin/env bash
# Download a tagged GitHub release's Windows build (CI cross-compiles it; this box can't) and publish it to
# /home/jga/builds/OpenDiag/<short-hash-of-the-tag>/ and latest/. Other platforms are kept as archives
# alongside it. Keeps the last 10 non-"latest" folders.
#
# Usage: scripts/publish_release.sh [tag]   (default: the latest release)
set -euo pipefail
cd "$(dirname "$0")/.."

tag="${1:-$(gh release list --limit 1 --json tagName --jq '.[0].tagName')}"
hash=$(git rev-parse --short "$tag")
dest=/home/jga/builds/OpenDiag
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

gh release download "$tag" --dir "$tmp" --clobber
zip_name=$(cd "$tmp" && ls opendiag-*-x86_64-pc-windows-msvc.zip)
( cd "$tmp" && grep "$zip_name" SHA256SUMS | sha256sum -c - )

for d in "$dest/$hash" "$dest/latest"; do
  rm -rf "$d"
  mkdir -p "$d"
  python3 -c "
import zipfile, pathlib, sys
with zipfile.ZipFile('$tmp/$zip_name') as z:
    for info in z.infolist():
        rel = pathlib.Path(*pathlib.Path(info.filename).parts[1:])  # drop the zip's top-level folder
        if not str(rel) or str(rel) == '.':
            continue
        out = pathlib.Path('$d') / rel
        if info.is_dir():
            out.mkdir(parents=True, exist_ok=True)
        else:
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_bytes(z.read(info))
"
  chmod +x "$d/opendiag.exe"
  mkdir -p "$d/other-platforms"
  for f in "$tmp"/*.tar.gz "$tmp/SHA256SUMS"; do
    cp "$f" "$d/other-platforms/"
  done
done

ls -1dt "$dest"/*/ | grep -v '/latest/$' | tail -n +11 | xargs -r rm -rf
echo "published $tag ($hash) -> N:\\OpenDiag\\latest\\"
