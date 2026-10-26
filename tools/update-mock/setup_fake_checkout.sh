#!/bin/bash
# Run after `cargo build -p axion-app --release`.
# A throwaway source checkout for the "Update from Git" flow: bare remote + clone + developer clone,
# the release binary (and CEF files) hard-linked into <checkout>/target/release, and a stub cargo.
set -eu
WT=$(cd "$(dirname "$0")/../.." && pwd)
BASE=$WT/target/updater-fake
export GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=t@example.org GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=t@example.org
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
rm -rf "$BASE"
mkdir -p "$BASE/remote/mock" "$BASE/bin"
git init -q --bare -b main "$BASE/remote/mock/axion.git"
git clone -q "$BASE/remote/mock/axion.git" "$BASE/checkout" 2>/dev/null
cd "$BASE/checkout"
printf '[workspace]\n' > Cargo.toml
printf 'target/\n' > .gitignore
git add . && git commit -q -m first && git push -q -u origin main
git clone -q "$BASE/remote/mock/axion.git" "$BASE/developer" 2>/dev/null
cd "$BASE/developer"
printf 'a new feature\n' > feature.txt
git add . && git commit -q -m "Add feature" && git push -q
# The runtime tree of the release build, hard-linked (same file system).
mkdir -p "$BASE/checkout/target/release"
cd "$WT/target/release"
for f in chrome_100_percent.pak chrome_200_percent.pak chrome-sandbox icudtl.dat libcef.so libvk_swiftshader.so libvulkan.so.1 resources.pak v8_context_snapshot.bin vk_swiftshader_icd.json; do
  ln -f "$f" "$BASE/checkout/target/release/$f"
done
cp -al locales "$BASE/checkout/target/release/locales"
cp axion "$BASE/checkout/target/release/axion"
# Stub cargo: prints a few lines like a build, takes a few seconds, succeeds (or fails with STUB_FAIL=1).
cat > "$BASE/bin/cargo" <<'EOS'
#!/bin/sh
echo "$@" >> "$(dirname "$0")/cargo.log"
for crate in axion-glass axion-ui axion-compositor axion-engine axion-update axion-app; do
  echo "   Compiling $crate v0.0.1 ($(pwd))"
  sleep 0.6
done
echo "warning: stub build, nothing was compiled" >&2
if [ -n "$STUB_FAIL" ]; then echo "error: could not compile \`axion-app\` (bin \"axion\")" >&2; exit 101; fi
echo "    Finished \`release\` profile [optimized] target(s) in 4.1s"
EOS
chmod +x "$BASE/bin/cargo"
echo "fake checkout ready: $BASE/checkout"
git -C "$BASE/checkout" status --short | head -3
