#!/usr/bin/env bash
# Generate all platform icon formats from the canonical LunCoSim SVGs.
#
# SVG files under assets/icons/svg are the source of truth. Raster files,
# ICO, and ICNS are build outputs and are intentionally ignored by Git.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SVG_DIR="$ROOT/assets/icons/svg"
OUT="$ROOT/assets/icons"

command -v convert >/dev/null || {
    echo "generate_icons.sh needs ImageMagick's convert" >&2
    exit 1
}

render() {
    local source="$1" size="$2" destination="$3"
    mkdir -p "$(dirname "$destination")"
    convert -background none "$source" -resize "${size}x${size}" "$destination"
}

linux="$SVG_DIR/lcs-night-linux.svg"
windows="$SVG_DIR/lcs-night-win.svg"
macos="$SVG_DIR/lcs-night-mac.svg"

for size in 16 24 32 48 64 128 256; do
    render "$linux" "$size" "$OUT/linux/hicolor/${size}x${size}/apps/luncosim.png"
    render "$windows" "$size" "$OUT/windows/png/luncosim-${size}.png"
done
for size in 16 32 64 128 256 512 1024; do
    render "$macos" "$size" "$OUT/macos/png/luncosim-${size}.png"
done

convert "$windows" -define icon:auto-resize=16,24,32,48,64,128,256 "$OUT/windows/luncosim.ico"

if command -v iconutil >/dev/null; then
    iconset="$OUT/macos/luncosim.iconset"
    rm -rf "$iconset"
    mkdir -p "$iconset"
    for size in 16 32 128 256 512; do
        render "$macos" "$size" "$iconset/icon_${size}x${size}.png"
        render "$macos" "$((size * 2))" "$iconset/icon_${size}x${size}@2x.png"
    done
    iconutil -c icns "$iconset" -o "$OUT/macos/luncosim.icns"
fi

echo "Generated LunCoSim icons from assets/icons/svg"
