#!/usr/bin/env bash
# Builds CEF with proprietary codecs (H.264/AAC) — the prebuilt CEF lacks them and ffmpeg is
# linked into libcef.so, so it can't be swapped. Takes hours and ~100 GB; run in the background.
#
#   tools/build-cef.sh            # builds into ~/cef-build (override with CEF_BUILD_DIR)
#
# The CEF commit must match the version the `cef` crate binds (third_party/cef/*/archive.json).
set -euo pipefail

CEF_COMMIT=4fa4ab5          # 156.0.14+g4fa4ab5+chromium-156.0.8078.48
CEF_BRANCH=8078
BUILD_DIR=${CEF_BUILD_DIR:-$HOME/cef-build}

mkdir -p "$BUILD_DIR"
cd "$BUILD_DIR"
if [ ! -f automate-git.py ]; then
  curl -fsSLo automate-git.py \
    https://raw.githubusercontent.com/chromiumembedded/cef/master/tools/automate/automate-git.py
fi

export GN_DEFINES="is_official_build=true proprietary_codecs=true ffmpeg_branding=Chrome \
use_sysroot=true symbol_level=0 is_cfi=false chrome_pgo_phase=0 use_thin_lto=false"
export CEF_ARCHIVE_FORMAT=tar.bz2

python3 automate-git.py \
  --download-dir="$BUILD_DIR/code" \
  --branch="$CEF_BRANCH" \
  --checkout="$CEF_COMMIT" \
  --x64-build \
  --no-debug-build \
  --minimal-distrib \
  --no-distrib-docs \
  --build-target=cefsimple
