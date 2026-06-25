#!/usr/bin/env bash
# Regenerate platform icon assets from the master emblem art.
#
# Source of truth : build/branding/logo_master_bore.png  (square emblem, transparent bg)
# Generated outputs: build/icons/{window_icon.png, icon.icns, icon.ico}  (committed)
#
# We derive icons from the EMBLEM, not the full wordmark: text-in-icon is
# illegible at 16x16/32x32. Re-run this whenever the master art changes.
#
# Requires: sips + iconutil (macOS, built-in) and ImageMagick (`brew install imagemagick`).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/build/branding/logo_master_bore.png"
OUT="$ROOT/build/icons"
mkdir -p "$OUT"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# resize <px> <dest> — square resample of the master into a PNG.
resize() { sips -s format png -z "$1" "$1" "$SRC" --out "$2" >/dev/null; }

echo ">> window_icon.png (256, winit runtime icon for Windows/X11)"
resize 256 "$OUT/window_icon.png"

echo ">> icon.icns (macOS app bundle)"
iconset="$tmp/icon.iconset"; mkdir -p "$iconset"
resize 16   "$iconset/icon_16x16.png"
resize 32   "$iconset/icon_16x16@2x.png"
resize 32   "$iconset/icon_32x32.png"
resize 64   "$iconset/icon_32x32@2x.png"
resize 128  "$iconset/icon_128x128.png"
resize 256  "$iconset/icon_128x128@2x.png"
resize 256  "$iconset/icon_256x256.png"
resize 512  "$iconset/icon_256x256@2x.png"
resize 512  "$iconset/icon_512x512.png"
resize 1024 "$iconset/icon_512x512@2x.png"
iconutil -c icns "$iconset" -o "$OUT/icon.icns"

echo ">> icon.ico (Windows exe resource, multi-resolution)"
sizes=(16 32 48 64 128 256)
pngs=()
for s in "${sizes[@]}"; do resize "$s" "$tmp/ico_$s.png"; pngs+=("$tmp/ico_$s.png"); done
magick "${pngs[@]}" "$OUT/icon.ico"

echo ">> Done. Generated in $OUT:"
ls -1 "$OUT"
