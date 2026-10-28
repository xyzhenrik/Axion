#!/usr/bin/env bash
# Regenerates the app icon set from assets/icons/source/axion-logo-new.webp (ImageMagick).
#   assets/icons/axion.png                      1024 px master (square, transparent)
#   assets/icons/hicolor/<n>x<n>/apps/axion.png 16 .. 512
#   assets/icons/hicolor/scalable/apps/axion.svg  the master embedded in an SVG wrapper
# Run from the repository root. The logo is already a rounded glass tile: it is only trimmed,
# centred and scaled, never masked again.
set -euo pipefail
cd "$(dirname "$0")/.."
src=assets/icons/source/axion-logo-new.webp
out=assets/icons
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Trim the transparent margin (alpha above ~5 %, so faint haze does not widen the box).
box=$(magick "$src" -alpha extract -threshold 5% -format '%@' info:)
magick "$src" -crop "$box" +repage "$tmp/trim.png"

# fit <percent of the canvas the tile's long side fills> <size> <file>
fit() {
  local pct=$1 size=$2 file=$3 inner
  inner=$((size * pct / 100))
  magick "$tmp/trim.png" -filter Lanczos -resize "${inner}x${inner}" -background none \
    -gravity center -extent "${size}x${size}" "$file"
}

fit 94 1024 "$out/axion.png"
for n in 512 256 128 64 48; do
  magick "$out/axion.png" -filter Lanczos -resize "${n}x${n}" -unsharp 0x0.6+0.5+0.02 \
    "$out/hicolor/${n}x${n}/apps/axion.png"
done
# Tiny sizes: the tile fills the canvas and gets more sharpening, so the globe stays readable.
for n in 32 24 16; do
  magick "$tmp/trim.png" -filter Lanczos -resize "${n}x${n}" -background none -gravity center \
    -extent "${n}x${n}" -unsharp 0x0.8+0.8+0.02 "$out/hicolor/${n}x${n}/apps/axion.png"
done
for f in "$out"/hicolor/*/apps/axion.png "$out/axion.png"; do
  magick "$f" -strip -define png:compression-level=9 "$f"
done

b64=$(magick "$out/axion.png" -resize 512x512 -strip -define png:compression-level=9 png:- | base64 -w0)
cat > "$out/hicolor/scalable/apps/axion.svg" <<EOF
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="512" height="512" viewBox="0 0 512 512"><image width="512" height="512" xlink:href="data:image/png;base64,$b64"/></svg>
EOF
