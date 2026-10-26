#!/usr/bin/env bash
# Builds Axion's release artifacts: an AppDir, an AppImage, a relocatable tarball and SHA256SUMS.
# Run it from anywhere; it works in the repository root. Safe to run again (it rebuilds the
# AppDir and the artifacts from scratch; cargo only recompiles what changed).
#
#   tools/package.sh [--version X.Y.Z] [--cef-path DIR] [--out DIR] [options]
#   tools/package.sh --notes                 # release notes skeleton from `git log`, then exit
#
#   --version X.Y.Z    version in the file names (default: the workspace version in Cargo.toml;
#                      a different value is refused: bump Cargo.toml first, the app shows it)
#   --cef-path DIR     the CEF distribution to ship: a folder with libcef.so, or a folder that
#                      holds <version>/cef_linux_x86_64 (the layout of third_party/cef).
#                      Default: $CEF_PATH, else third_party/cef. This is also what cargo builds
#                      against, so use the same value you build with (a custom H.264 build!).
#   --out DIR          where the artifacts go (default: dist/)
#   --skip-build       use the existing target/release/axion
#   --debug-symbols    keep the symbols in out/axion-<version>.debug (the shipped binary is stripped)
#   --no-appimage      only the tarball
#   --no-tarball       only the AppImage
#   --comp xz|zstd     AppImage compression (default zstd)
#   --runtime FILE     AppImage runtime to use when `appimagetool` is not installed (also
#                      $APPIMAGE_RUNTIME, or ~/.cache/axion/appimage-runtime-x86_64)
#   --cargo-args ARGS  extra arguments for cargo, e.g. --cargo-args "--no-default-features" to
#                      build without the TPM backend
#
# AppImage tooling, in this order: `appimagetool` (or $APPIMAGETOOL), else `mksquashfs` plus an
# AppImage runtime file. With neither, the AppDir and the tarball are still produced and the
# script says what to install. Nothing is downloaded.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

version=""
cef_path=${CEF_PATH:-}
out=dist
skip_build=0
debug_symbols=0
do_appimage=1
do_tarball=1
comp=zstd
runtime=${APPIMAGE_RUNTIME:-}
cargo_extra=()
notes=0

die() { echo "package.sh: $*" >&2; exit 1; }
say() { echo "==> $*"; }

while [ $# -gt 0 ]; do
  case $1 in
    --version) version=${2:?}; shift 2 ;;
    --cef-path) cef_path=${2:?}; shift 2 ;;
    --out) out=${2:?}; shift 2 ;;
    --skip-build) skip_build=1; shift ;;
    --debug-symbols) debug_symbols=1; shift ;;
    --no-appimage) do_appimage=0; shift ;;
    --no-tarball) do_tarball=0; shift ;;
    --comp) comp=${2:?}; shift 2 ;;
    --runtime) runtime=${2:?}; shift 2 ;;
    --cargo-args) read -r -a more <<<"${2:?}"; cargo_extra+=("${more[@]}"); shift 2 ;;
    --notes) notes=1; shift ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
done

# ---- release notes skeleton ---------------------------------------------------------------
if [ "$notes" = 1 ]; then
  last=$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)
  # On the release commit itself ("v0.2.0" is HEAD) the notes cover the span since the tag before.
  if [ -n "$last" ] && [ "$(git rev-list -n1 "$last")" = "$(git rev-parse HEAD)" ]; then
    last=$(git describe --tags --abbrev=0 --match 'v[0-9]*' "$last^" 2>/dev/null || true)
  fi
  range=${last:+$last..}HEAD
  echo "## What's new"
  echo
  if [ -n "$last" ]; then echo "<!-- changes since $last -->"; else echo "<!-- first release: all commits -->"; fi
  git log --no-merges --pretty='- %s' "$range"
  echo
  echo "## Install"
  echo
  echo "Download \`Axion-<version>-x86_64.AppImage\`, run \`chmod +x\` on it and start it, or unpack"
  echo "\`Axion-<version>-linux-x86_64.tar.gz\` and run \`./install.sh\`. Check the download against"
  echo "\`SHA256SUMS\`. Details: docs/install.md."
  exit 0
fi

# ---- version -----------------------------------------------------------------------------
cargo_version=$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p}' Cargo.toml | head -n1)
[ -n "$cargo_version" ] || die "cannot read the workspace version from Cargo.toml"
version=${version:-$cargo_version}
version=${version#v}
[ "$version" = "$cargo_version" ] || die "--version $version differs from Cargo.toml ($cargo_version): bump [workspace.package] version (and Cargo.lock) first"
[ "$(uname -m)" = x86_64 ] || die "only x86_64 is packaged today (this machine: $(uname -m))"
arch=x86_64

# ---- the CEF runtime to ship ---------------------------------------------------------------
resolve_cef() {
  local base=$1 d
  [ -n "$base" ] || base=third_party/cef
  if [ -f "$base/libcef.so" ]; then echo "$base"; return; fi
  # <base>/<version>/cef_linux_x86_64 (newest version first; never the *.stock copies)
  for d in $(find "$base" -mindepth 2 -maxdepth 2 -type d -name 'cef_linux_x86_64' 2>/dev/null | sort -rV); do
    if [ -f "$d/libcef.so" ]; then echo "$d"; return; fi
  done
  return 1
}

if [ "$skip_build" = 0 ]; then
  say "cargo build -p axion-app --release ${cargo_extra[*]:-}"
  if [ -n "$cef_path" ]; then export CEF_PATH=$cef_path; fi
  cargo build -p axion-app --release "${cargo_extra[@]}"
fi

target_dir=${CARGO_TARGET_DIR:-target}
exe=$target_dir/release/axion
[ -x "$exe" ] || die "$exe does not exist (build failed or --skip-build without a build)"
cef_dir=$(resolve_cef "$cef_path") || die "no CEF distribution found in '${cef_path:-third_party/cef}' (pass --cef-path DIR)"
cef_dir=$(cd "$cef_dir" && pwd)
say "CEF runtime from $cef_dir"
# The build copied the files it linked against next to the binary; a different libcef here
# means the wrong --cef-path.
if [ -f "$target_dir/release/libcef.so" ] \
  && [ "$(stat -c %s "$cef_dir/libcef.so")" != "$(stat -c %s "$target_dir/release/libcef.so")" ]; then
  echo "warning: $cef_dir/libcef.so differs in size from $target_dir/release/libcef.so (what the binary was built against);" >&2
  echo "         rebuild with the same CEF_PATH or pass the right --cef-path" >&2
fi

mkdir -p "$out"
out=$(cd "$out" && pwd)
appdir=$out/Axion.AppDir
name=Axion-$version
rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" "$appdir/usr/share/doc/axion"

# ---- AppDir ------------------------------------------------------------------------------
say "assembling $appdir"
install -m 755 "$exe" "$appdir/usr/bin/axion"
if [ "$debug_symbols" = 1 ]; then
  objcopy --only-keep-debug "$appdir/usr/bin/axion" "$out/axion-$version.debug"
  objcopy --add-gnu-debuglink="$out/axion-$version.debug" "$appdir/usr/bin/axion"
fi
strip --strip-unneeded "$appdir/usr/bin/axion"

# CEF: libraries, resources and locales. Not shipped: headers, cmake files, the SUID helper
# (it cannot be root-owned in an AppImage; Axion uses the user-namespace sandbox).
for f in "$cef_dir"/*; do
  [ -f "$f" ] || continue
  case $(basename "$f") in
    libcef.so|libvk_swiftshader.so|libvulkan.so.1|libEGL.so|libGLESv2.so|libvulkan.so) install -m 755 "$f" "$appdir/usr/bin/" ;;
    *.pak|*.dat|*.bin) install -m 644 "$f" "$appdir/usr/bin/" ;;
    vk_swiftshader_icd.json) install -m 644 "$f" "$appdir/usr/bin/" ;;
    CREDITS.html) install -m 644 "$f" "$appdir/usr/share/doc/axion/CREDITS-chromium.html" ;;
  esac
done
mkdir -p "$appdir/usr/bin/locales"
install -m 644 "$cef_dir"/locales/*.pak "$appdir/usr/bin/locales/"

# TPM libraries: the binary links libtss2 (the `tpm` feature), which many systems lack.
# Bundled next to it, they are found through the RUNPATH. libcrypto stays a system library.
while read -r lib path; do
  case $lib in
    libtss2-*) install -m 755 "$(readlink -f "$path")" "$appdir/usr/bin/$lib" ;;
  esac
done < <(ldd "$exe" | awk '$2 == "=>" && $3 ~ /^\// {print $1, $3}')

# Desktop entry and icons (the icon set Axion also writes itself, see desktop_integration.rs).
sed -e '/^Version=/a X-AppImage-Version='"$version" assets/axion.desktop >"$appdir/usr/share/applications/axion.desktop"
mkdir -p "$appdir/usr/share/icons"
cp -a assets/icons/hicolor "$appdir/usr/share/icons/"
cp "$appdir/usr/share/applications/axion.desktop" "$appdir/axion.desktop"
cp assets/icons/hicolor/256x256/apps/axion.png "$appdir/axion.png"
ln -s axion.png "$appdir/.DirIcon"
install -m 644 LICENSE "$appdir/usr/share/doc/axion/LICENSE"
install -m 755 tools/package/AppRun "$appdir/AppRun"
command -v desktop-file-validate >/dev/null && desktop-file-validate "$appdir/axion.desktop" || true

# What the binary asks of the target system.
need_glibc=$(objdump -T "$appdir/usr/bin/axion" "$appdir/usr/bin/libcef.so" 2>/dev/null \
  | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -n1)
say "needs glibc >= ${need_glibc:-?}"
if LD_LIBRARY_PATH= ldd "$appdir/usr/bin/axion" | grep -q 'not found'; then
  LD_LIBRARY_PATH= ldd "$appdir/usr/bin/axion" | grep 'not found' >&2
  die "the AppDir binary has unresolved libraries (see above)"
fi
say "AppDir: $(du -sh "$appdir" | cut -f1)"

# ---- AppImage ----------------------------------------------------------------------------
appimage=$out/$name-$arch.AppImage
rm -f "$appimage"
if [ "$do_appimage" = 1 ]; then
  tool=${APPIMAGETOOL:-$(command -v appimagetool || true)}
  if [ -z "$runtime" ] && [ -f "$HOME/.cache/axion/appimage-runtime-$arch" ]; then
    runtime=$HOME/.cache/axion/appimage-runtime-$arch
  fi
  if [ -n "$tool" ]; then
    say "AppImage with $tool ($comp)"
    ARCH=$arch "$tool" --no-appstream --comp "$comp" "$appdir" "$appimage"
  elif command -v mksquashfs >/dev/null && [ -n "$runtime" ] && [ -f "$runtime" ]; then
    say "AppImage with mksquashfs ($comp) and the runtime $runtime"
    sqfs=$out/.axion-$version.squashfs
    rm -f "$sqfs"
    case $comp in
      zstd) comp_args=(-comp zstd -Xcompression-level 19) ;;
      xz) comp_args=(-comp xz -Xdict-size 100%) ;;
      *) die "--comp must be xz or zstd" ;;
    esac
    mksquashfs "$appdir" "$sqfs" -root-owned -noappend -b 1M "${comp_args[@]}" -quiet
    cat "$runtime" "$sqfs" >"$appimage"
    rm -f "$sqfs"
    chmod 755 "$appimage"
  else
    cat >&2 <<EOF
note: no AppImage tooling found, so no AppImage was built (the AppDir and the tarball are fine).
      Install ONE of these and run this script again:
        - appimagetool   https://github.com/AppImage/appimagetool/releases (a single file; put it in PATH,
                         or set APPIMAGETOOL=/path/to/appimagetool-x86_64.AppImage)
        - squashfs-tools (mksquashfs) plus an AppImage runtime file:
                         https://github.com/AppImage/type2-runtime/releases  ->  runtime-x86_64
                         saved as ~/.cache/axion/appimage-runtime-x86_64 (or --runtime FILE)
EOF
    do_appimage=0
  fi
  if [ -f "$appimage" ]; then say "AppImage: $appimage ($(du -h "$appimage" | cut -f1))"; fi
fi

# ---- tarball -----------------------------------------------------------------------------
tarball=$out/$name-linux-$arch.tar.gz
rm -f "$tarball"
if [ "$do_tarball" = 1 ]; then
  say "tarball"
  stage=$out/.stage
  rm -rf "$stage"
  mkdir -p "$stage"
  # Hard links where possible, so the 500 MB engine is not copied a second time.
  cp -al "$appdir" "$stage/$name-linux-$arch" 2>/dev/null || cp -a "$appdir" "$stage/$name-linux-$arch"
  ( cd "$stage/$name-linux-$arch"
    ln -s AppRun axion
    # The tarball and the AppDir are the same tree; the AppImage-only files are harmless.
  )
  install -m 755 tools/install.sh "$stage/$name-linux-$arch/install.sh"
  gz=(gzip -6)
  command -v pigz >/dev/null && gz=(pigz -6)
  tar --owner=0 --group=0 --numeric-owner --sort=name -C "$stage" -cf - "$name-linux-$arch" | "${gz[@]}" >"$tarball"
  rm -rf "$stage"
  say "tarball: $tarball ($(du -h "$tarball" | cut -f1))"
fi

# ---- checksums ---------------------------------------------------------------------------
( cd "$out"
  files=()
  [ -f "$name-$arch.AppImage" ] && files+=("$name-$arch.AppImage")
  [ -f "$name-linux-$arch.tar.gz" ] && files+=("$name-linux-$arch.tar.gz")
  if [ ${#files[@]} -gt 0 ]; then
    sha256sum "${files[@]}" >SHA256SUMS
    say "SHA256SUMS:"; sed 's/^/    /' SHA256SUMS
  fi
)
say "done: $out"
