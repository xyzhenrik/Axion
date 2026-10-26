#!/usr/bin/env python3
"""Builds ./site for mock_server.py (usage: make_site.py [PORT]): manifests, a GitHub-like API tree
(repos/mock/axion/releases/latest), a fake AppImage with SHA256SUMS, a big throttled one, and
broken variants (bad checksum, no checksum, invalid JSON) for trying the updater by hand:

    python3 tools/update-mock/make_site.py 18941 && python3 tools/update-mock/mock_server.py 18941
    # then, in a throwaway profile: [updates] manifest_url = "http://127.0.0.1:18941/latest.json"
    axion --check-update
"""
import hashlib
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SITE = os.path.join(HERE, "site")
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18941
BASE = f"http://127.0.0.1:{PORT}"
NOTES = """## What's new in 9.9.0

- **Faster** tab switching, see [the changelog](https://example.org/changes)
- Shields: `$popup` rules are applied to redirects
- New *Settings > About & Updates* page

### Fixes

- Playlist downloads resume after a restart
- The sidebar no longer flickers on resize
- Find on Page keeps the match visible
- Wayland: pasted images reach the page again
- Passwords: the export dialog remembers its folder

### Known issues

- Native Wayland file drops are still missing
"""


def write(path, data, binary=False):
    full = os.path.join(SITE, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "wb" if binary else "w") as f:
        f.write(data)


image = b"\x7fELF" + b"fake axion appimage 9.9.0\n" * 4000
write("dl/Axion-9.9.0-x86_64.AppImage", image, True)
digest = hashlib.sha256(image).hexdigest()
write("dl/SHA256SUMS", f"{digest}  Axion-9.9.0-x86_64.AppImage\n")

assets = [
    {"name": "Axion-9.9.0-x86_64.AppImage", "url": "dl/Axion-9.9.0-x86_64.AppImage", "size": len(image)},
    {"name": "SHA256SUMS", "url": "dl/SHA256SUMS"},
]
manifest = {"version": "9.9.0", "notes": NOTES, "url": f"{BASE}/release/9.9.0", "assets": assets}
write("latest.json", json.dumps(manifest))
write("uptodate.json", json.dumps({"version": "0.0.1", "notes": "Nothing new.", "assets": []}))
write("broken.json", "this is not json")
write("beta.json", json.dumps({**manifest, "version": "10.0.0-beta.1", "prerelease": True}))

gh_assets = [
    {"name": a["name"], "browser_download_url": f"{BASE}/{a['url']}", "size": a.get("size", 99)} for a in assets
]
release = {
    "tag_name": "v9.9.0",
    "name": "Axion 9.9.0",
    "body": NOTES,
    "html_url": f"{BASE}/release/9.9.0",
    "prerelease": False,
    "draft": False,
    "assets": gh_assets,
}
write("repos/mock/axion/releases/latest", json.dumps(release))
write("release/9.9.0", "<html><body><h1>Axion 9.9.0</h1></body></html>")
big = b"\x7fELF" + os.urandom(1024) * 24000
write("dl/big/Axion-9.9.0-x86_64.AppImage", big, True)
bigsum = hashlib.sha256(big).hexdigest()
write("dl/big/SHA256SUMS", f"{bigsum}  Axion-9.9.0-x86_64.AppImage\n")
big_assets = [
    {"name": "Axion-9.9.0-x86_64.AppImage", "url": "throttle/dl/big/Axion-9.9.0-x86_64.AppImage", "size": len(big)},
    {"name": "SHA256SUMS", "url": "dl/big/SHA256SUMS"},
]
write("throttle.json", json.dumps({**manifest, "assets": big_assets}))
bad_assets = [dict(a) for a in assets]
bad_assets[1] = {"name": "SHA256SUMS", "url": "dl/bad/SHA256SUMS"}
write("dl/bad/SHA256SUMS", f"{'0' * 64}  Axion-9.9.0-x86_64.AppImage\n")
write("badsum.json", json.dumps({**manifest, "assets": bad_assets}))
write("nosum.json", json.dumps({**manifest, "assets": assets[:1]}))
# For the "slow" state, the same documents below /slow/.
print("site written to", SITE, "digest", digest)
