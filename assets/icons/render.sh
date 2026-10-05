#!/usr/bin/env bash
# Rasterize the two SVGs into every size the platforms consume.
#
# Requires rsvg-convert and, on macOS, iconutil. Regenerates:
#   assets/AppIcon.icns
#   desktop/icon.png
#   assets/icons/menubar-template.pdf
#   assets/icons/hicolor/<size>/apps/animesh.png
set -euo pipefail

dir=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$dir/../.." && pwd)
mark="$dir/mark.svg"
app="$dir/app-icon.svg"

need() {
  command -v "$1" >/dev/null || {
    echo "render-icons: $1 is required" >&2
    exit 1
  }
}

need rsvg-convert

rsvg-convert -f pdf -o "$dir/menubar-template.pdf" "$mark"

# Companion raster preview at menu-bar @2x resolution; the app embeds the PDF.
rsvg-convert -w 36 -h 36 -o "$dir/menubar-template.png" "$mark"

hicolor="$dir/hicolor"
for size in 16 24 32 48 256 512; do
  dest="$hicolor/${size}x${size}/apps"
  mkdir -p "$dest"
  rsvg-convert -w "$size" -h "$size" -o "$dest/animesh.png" "$app"
done

# Tauri requires RGBA. The shared SVG's transparent margin supplies the alpha.
cp "$hicolor/256x256/apps/animesh.png" "$root/desktop/icon.png"

# macOS iconset → icns. Linux CI never runs this branch; the icns is committed.
if command -v iconutil >/dev/null; then
  iconset=$(mktemp -d)
  trap 'rm -rf "$iconset"' EXIT
  mkdir -p "$iconset/AppIcon.iconset"
  set="$iconset/AppIcon.iconset"
  # name                    pixels
  rsvg-convert -w 16 -h 16 -o "$set/icon_16x16.png" "$app"
  rsvg-convert -w 32 -h 32 -o "$set/icon_16x16@2x.png" "$app"
  rsvg-convert -w 32 -h 32 -o "$set/icon_32x32.png" "$app"
  rsvg-convert -w 64 -h 64 -o "$set/icon_32x32@2x.png" "$app"
  rsvg-convert -w 128 -h 128 -o "$set/icon_128x128.png" "$app"
  rsvg-convert -w 256 -h 256 -o "$set/icon_128x128@2x.png" "$app"
  rsvg-convert -w 256 -h 256 -o "$set/icon_256x256.png" "$app"
  rsvg-convert -w 512 -h 512 -o "$set/icon_256x256@2x.png" "$app"
  rsvg-convert -w 512 -h 512 -o "$set/icon_512x512.png" "$app"
  rsvg-convert -w 1024 -h 1024 -o "$set/icon_512x512@2x.png" "$app"
  iconutil -c icns "$set" -o "$root/assets/AppIcon.icns"
else
  echo "render-icons: iconutil missing; left assets/AppIcon.icns unchanged" >&2
fi

echo "rendered icons under $dir and $root/assets/AppIcon.icns"
