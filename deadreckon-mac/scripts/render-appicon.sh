#!/bin/bash
# Render every AppIcon.appiconset slot from design/icon.svg (the committed
# master, SETTINGS-SCREENS-SPEC.md §I). Each size is rendered from the
# vector at its exact pixel size — never downscaled from a single raster —
# so the small sizes get real rasterizer hinting instead of resample mush.
#
# Requires rsvg-convert (brew install librsvg).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MASTER="$ROOT/design/icon.svg"
OUT="$ROOT/Assets.xcassets/AppIcon.appiconset"

command -v rsvg-convert >/dev/null 2>&1 || {
    echo "rsvg-convert not found (brew install librsvg)" >&2
    exit 1
}
[ -f "$MASTER" ] || { echo "master missing: $MASTER" >&2; exit 1; }

# filename => pixel size (Contents.json slot filenames stay unchanged).
render() {
    local name="$1" px="$2"
    rsvg-convert -w "$px" -h "$px" "$MASTER" -o "$OUT/$name"
    echo "rendered $name (${px}x${px})"
}

render icon_16.png      16
render icon_16@2x.png   32
render icon_32.png      32
render icon_32@2x.png   64
render icon_128.png     128
render icon_128@2x.png  256
render icon_256.png     256
render icon_256@2x.png  512
render icon_512.png     512
render icon_512@2x.png  1024
