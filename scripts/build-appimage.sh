#!/usr/bin/env bash
# Packs a built booto-writer binary into an AppImage and writes <out>.sha256 next to it.
# Usage: scripts/build-appimage.sh <booto-writer binary> <out.AppImage>
set -euo pipefail
bin="${1:?usage: $0 <binary> <out.AppImage>}"
out="${2:?usage: $0 <binary> <out.AppImage>}"
here="$(cd "$(dirname "$0")/.." && pwd)"

tool_url="https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage"
tool_sha="ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
curl -fsSLo "$work/appimagetool" "$tool_url"
echo "$tool_sha  $work/appimagetool" | sha256sum -c --quiet
chmod +x "$work/appimagetool"

app="$work/AppDir"
install -Dm755 "$bin" "$app/usr/bin/booto-writer"
ln -s usr/bin/booto-writer "$app/AppRun"
cp "$here/packaging/booto-writer.desktop" "$here/packaging/booto-writer.svg" "$app/"

# CI runners have no FUSE, so run appimagetool extracted.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$work/appimagetool" --no-appstream "$app" "$out"
(cd "$(dirname "$out")" && sha256sum "$(basename "$out")" > "$(basename "$out").sha256")
echo "Built $out"
