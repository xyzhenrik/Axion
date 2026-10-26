#!/usr/bin/env bash
# Installs (or removes) Axion's desktop entry and icons so Wayland compositors (KDE, GNOME)
# can show the app icon: on Wayland the window icon comes from `<app_id>.desktop` + the icon
# theme, not from the window itself (winit ignores window icons there). Idempotent.
#
#   tools/install-desktop.sh [--bin PATH] [--uninstall]
#
#   --bin PATH   the binary the entry launches (default: target/release/axion of this checkout)
#   --uninstall  remove the entry and icons again
#
# Installs per user into ${XDG_DATA_HOME:-~/.local/share}.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
bin=$root/target/release/axion
uninstall=0

while [ $# -gt 0 ]; do
  case $1 in
    --bin) bin=$(realpath -m "$2"); shift 2 ;;
    --uninstall) uninstall=1; shift ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

apps=$data/applications
icons=$data/icons/hicolor

refresh() {
  if command -v update-desktop-database >/dev/null; then update-desktop-database "$apps" 2>/dev/null || true; fi
  if command -v gtk-update-icon-cache >/dev/null; then gtk-update-icon-cache -q -f -t "$icons" 2>/dev/null || true; fi
  if command -v kbuildsycoca6 >/dev/null; then kbuildsycoca6 >/dev/null 2>&1 || true; fi
}

if [ "$uninstall" = 1 ]; then
  rm -f "$apps/axion.desktop" "$icons"/*/apps/axion.png "$icons/scalable/apps/axion.svg"
  refresh
  echo "Axion desktop entry and icons removed from $data"
  exit 0
fi

[ -f "$bin" ] || echo "note: $bin does not exist yet (build it first); the entry will point there" >&2

mkdir -p "$apps"
# Exec= gets the absolute binary path (quoted, as the spec requires for spaces).
esc=${bin//\\/\\\\}
esc=${esc//\"/\\\"}
sed -e "s|^Exec=.*|Exec=\"$esc\" %U|" "$root/assets/axion.desktop" >"$apps/axion.desktop"
for src in "$root"/assets/icons/hicolor/*/apps/axion.*; do
  rel=${src#"$root"/assets/icons/hicolor/}
  mkdir -p "$icons/$(dirname "$rel")"
  install -m 644 "$src" "$icons/$rel"
done
refresh

echo "Installed $apps/axion.desktop (Exec: $bin) and the icons in $icons"
echo "Restart Axion; if the icon still looks generic, log out/in or run: kbuildsycoca6 --noincremental"
