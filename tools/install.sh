#!/usr/bin/env bash
# Installs Axion as an app for your user, without root. Safe to run again (it replaces the
# installed copy atomically; your settings and browsing data are never touched).
#
#   ./install.sh [SOURCE] [--prefix DIR] [--bin-dir DIR] [--uninstall]
#
#   SOURCE        what to install, one of
#                   - nothing: the unpacked release folder this script sits in
#                   - a folder: an unpacked Axion-X.Y.Z-linux-x86_64 (or dist/Axion.AppDir)
#                   - a Axion-X.Y.Z-linux-x86_64.tar.gz
#                   - an Axion-X.Y.Z-x86_64.AppImage (kept as one file, so Axion can update
#                     itself in place, see docs/features/updates.md)
#   --prefix DIR  where the program goes (default: ~/.local/opt/axion)
#   --bin-dir DIR where the `axion` command goes (default: ~/.local/bin)
#   --uninstall   remove the program, the command, the menu entry and the icons
#
# The menu entry and icons go to ${XDG_DATA_HOME:-~/.local/share}.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
prefix=${HOME:?}/.local/opt/axion
bindir=$HOME/.local/bin
data=${XDG_DATA_HOME:-$HOME/.local/share}
source_arg=""
uninstall=0

die() { echo "install.sh: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case $1 in
    --prefix) prefix=$(realpath -m -- "${2:?}"); shift 2 ;;
    --bin-dir) bindir=$(realpath -m -- "${2:?}"); shift 2 ;;
    --uninstall) uninstall=1; shift ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) die "unknown option: $1 (try --help)" ;;
    *) [ -z "$source_arg" ] || die "only one SOURCE, please"; source_arg=$1; shift ;;
  esac
done

apps=$data/applications
icons=$data/icons/hicolor

refresh() {
  if command -v update-desktop-database >/dev/null; then update-desktop-database "$apps" 2>/dev/null || true; fi
  if command -v gtk-update-icon-cache >/dev/null; then gtk-update-icon-cache -q -f -t "$icons" 2>/dev/null || true; fi
  if command -v kbuildsycoca6 >/dev/null; then kbuildsycoca6 >/dev/null 2>&1 || true; fi
  # KDE keeps icons it once could not find empty until it is told the icons changed (what
  # Axion itself sends after installing them, see desktop_integration.rs).
  if command -v dbus-send >/dev/null; then
    dbus-send --session --type=signal /KIconLoader org.kde.KIconLoader.iconChanged int32:0 2>/dev/null || true
  fi
}

# ---- uninstall ---------------------------------------------------------------------------
if [ "$uninstall" = 1 ]; then
  # The command, only when it is ours (a link into the prefix).
  if [ -L "$bindir/axion" ]; then
    target=$(readlink -f -- "$bindir/axion" 2>/dev/null || true)
    case $target in "$prefix"/*|"$prefix") rm -f -- "$bindir/axion" ;; *) echo "kept $bindir/axion (it does not point into $prefix)" ;; esac
  fi
  rm -rf -- "$prefix"
  rm -f -- "$apps/axion.desktop" "$icons"/*/apps/axion.png "$icons/scalable/apps/axion.svg"
  refresh
  echo "Axion removed (program, command, menu entry, icons)."
  echo "Your settings and data are still in ~/.config/axion and ~/.local/share/axion; delete them yourself to start fresh."
  exit 0
fi

# ---- find the source ---------------------------------------------------------------------
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT

src=${source_arg:-$here}
mode=tree
if [ -f "$src" ]; then
  case $src in
    *.AppImage|*.appimage) mode=appimage ;;
    *.tar.gz|*.tgz) mode=tree; tar -xzf "$src" -C "$tmp"; src=$(find "$tmp" -mindepth 1 -maxdepth 1 -type d | head -n1) ;;
    *) die "$src is neither a folder, a .tar.gz nor an .AppImage" ;;
  esac
fi
if [ "$mode" = tree ]; then
  [ -x "$src/AppRun" ] && [ -x "$src/usr/bin/axion" ] || die "$src does not look like an Axion release folder (no AppRun / usr/bin/axion)"
fi

# ---- install -----------------------------------------------------------------------------
mkdir -p -- "$(dirname -- "$prefix")" "$bindir" "$apps"
new=$prefix.new.$$
old=$prefix.old.$$
rm -rf -- "$new"
mkdir -p -- "$new"

if [ "$mode" = appimage ]; then
  image=$(realpath -- "$src")
  install -m 755 "$image" "$new/Axion.AppImage"
  launcher=$prefix/Axion.AppImage
  # Menu entry and icons, taken from the image (extraction works without FUSE).
  ( cd "$tmp" && "$image" --appimage-extract usr/share >/dev/null 2>&1 ) || true
  icon_src=$tmp/squashfs-root/usr/share/icons/hicolor
  entry_src=$tmp/squashfs-root/usr/share/applications/axion.desktop
  if [ ! -f "$entry_src" ]; then
    # The image could not be unpacked here: a plain entry (Axion adds the icons itself at start).
    entry_src=$tmp/axion.desktop
    printf '%s\n' '[Desktop Entry]' 'Type=Application' 'Version=1.0' 'Name=Axion' 'GenericName=Web Browser' \
      'Comment=Browse the web with Liquid Glass' 'Exec=axion %U' 'Icon=axion' 'Terminal=false' 'StartupWMClass=axion' \
      'StartupNotify=true' 'Categories=Network;WebBrowser;' \
      'MimeType=text/html;application/xhtml+xml;x-scheme-handler/http;x-scheme-handler/https;' >"$entry_src"
  fi
else
  # A copy that keeps hard links apart from the source (cp -a copies the data).
  cp -a -- "$src/." "$new/"
  launcher=$prefix/AppRun
  icon_src=$new/usr/share/icons/hicolor
  entry_src=$new/usr/share/applications/axion.desktop
fi
[ -f "$entry_src" ] || die "the release has no usr/share/applications/axion.desktop"

# Swap the new copy in; the old one is removed only after the new one is in place.
if [ -e "$prefix" ] || [ -L "$prefix" ]; then mv -- "$prefix" "$old"; fi
mv -- "$new" "$prefix"
rm -rf -- "$old"
if [ "$mode" = tree ]; then
  icon_src=$prefix/usr/share/icons/hicolor
  entry_src=$prefix/usr/share/applications/axion.desktop
fi

# The `axion` command.
if [ -e "$bindir/axion" ] && [ ! -L "$bindir/axion" ]; then
  die "$bindir/axion exists and is not a link; move it away or use --bin-dir"
fi
ln -sfn -- "$launcher" "$bindir/axion"

# The icons first: a desktop that sees the entry before its icon keeps the icon empty.
if [ -d "$icon_src" ]; then
  for f in "$icon_src"/*/apps/axion.*; do
    [ -f "$f" ] || continue
    rel=${f#"$icon_src"/}
    mkdir -p -- "$icons/$(dirname -- "$rel")"
    install -m 644 -- "$f" "$icons/$rel"
  done
fi
# Menu entry: Exec points at the installed launcher, quoted and escaped as the Desktop Entry
# Specification wants (the same as Axion's own `exec_command`, so Axion does not rewrite it for
# that): `"`, `` ` ``, `$` and `\` get a backslash, which the string syntax doubles again, and
# `%` doubles.
esc=${launcher//\\/\\\\\\\\}
esc=${esc//\"/\\\\\"}
esc=${esc//\`/\\\\\`}
esc=${esc//\$/\\\\\$}
esc=${esc//%/%%}
EXEC_LINE="Exec=\"$esc\" %U" awk '/^Exec=/ {print ENVIRON["EXEC_LINE"]; next} /^X-AppImage-/ {next} {print}' \
  "$entry_src" >"$apps/axion.desktop"
refresh

echo "Installed Axion in $prefix"
echo "  command:    $bindir/axion"
echo "  menu entry: $apps/axion.desktop"
case ":$PATH:" in
  *":$bindir:"*) ;;
  *) echo
     echo "Note: $bindir is not in your PATH. Add it (for example in ~/.profile):"
     echo "  export PATH=\"$bindir:\$PATH\"" ;;
esac
echo
echo "Start it from the application menu or with: axion"
