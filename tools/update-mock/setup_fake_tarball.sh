#!/bin/bash
# A throwaway tarball installation to try the tarball updater with: <target>/updater-fake-tarball/opt/axion
# is laid out like an unpacked Axion-0.3.0-linux-x86_64 (AppRun, usr/bin/axion, no marker, like the
# 0.2/0.3 tarballs), with the real release binary as usr/bin/axion. Point the mock at
# tarball.json (see make_site.py) and run <dir>/usr/bin/axion with throwaway profile/config dirs.
set -eu
WT=$(cd "$(dirname "$0")/../.." && pwd)
TARGET=${CARGO_TARGET_DIR:-$WT/target}
BASE=$TARGET/updater-fake-tarball
rm -rf "$BASE"
mkdir -p "$BASE/opt/axion/usr/bin"
cd "$TARGET/release"
for f in chrome_100_percent.pak chrome_200_percent.pak chrome-sandbox icudtl.dat libcef.so libvk_swiftshader.so libvulkan.so.1 resources.pak v8_context_snapshot.bin vk_swiftshader_icd.json; do
  ln -f "$f" "$BASE/opt/axion/usr/bin/$f"
done
cp -al locales "$BASE/opt/axion/usr/bin/locales"
cp axion "$BASE/opt/axion/usr/bin/axion"
printf '#!/bin/sh\nexec "$(dirname "$0")/usr/bin/axion" "$@"\n' > "$BASE/opt/axion/AppRun"
chmod +x "$BASE/opt/axion/AppRun"
echo "fake tarball install ready: $BASE/opt/axion"
