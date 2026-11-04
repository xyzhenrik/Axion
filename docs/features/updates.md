# Updates

Axion can tell you when a new version is out and, depending on how you installed it, update itself.
There are four kinds of installation, and each one is updated differently (see
[What each kind does](#what-each-kind-does-and-does-not-touch)).
It never sends anything except a plain request for the newest release: no telemetry, no
identifiers.

Everything is on **Settings > About & Updates**.

## How Axion finds updates

Axion asks the project's releases on GitHub (`xyzhenrik/Axion` by default) for the newest version.
That version is compared with the one you run.

- A check happens a few seconds after Axion starts, at most once a day. Turn it off with
  **Check for updates automatically**.
- **Check for Updates** checks right now and always shows the result.
- You can also run `axion --check-update` in a terminal. It asks once, prints the answer and exits.
- When a newer version is found, a message **Axion X is available** appears with an **Update**
  button, and a red dot shows on the account button and on the **About & Updates** entry. Axion tells
  you once per version.
- When Axion's window is **not** in front, the same news also arrives as a desktop notification
  (freedesktop notifications, so any desktop with a notification daemon). It has **Update** (opens
  **About & Updates** and the confirmation) and **Later** (silences reminders like **Remind Me
  Later**); clicking the notification itself means **Update**. It shows the first line of the release
  notes. Turn it off with **Notify me about updates**. While the window is in front only the in-app
  message shows (`updates.desktop_notification_when_focused` changes that). Without a notification
  service nothing happens and the in-app message is all there is.
- Pre-releases (alpha, beta, release candidates) are hidden unless you turn on
  **Include pre-releases**.

The page shows the running version (and the Git commit for builds from source), how Axion was
installed, when it last checked, and the release notes of a new version.

## Choose what happens with a new version

| Button | What it does |
|---|---|
| **Update** / **Update from Git** / **Open Release Page** | Installs the update, or opens its page, depending on how you installed Axion (see below). |
| **Skip This Version** | Axion does not announce this version again. A later version is announced as usual. |
| **Remind Me Later** | Silences the notice for a day. |
| **Release Page** | Opens the release in a tab. |

Anything that changes your installation first shows a confirmation that says what will happen.
When you confirm, the sheet closes at once and the update reports itself on the **About & Updates**
page (Axion opens it for you):

- **While it runs**: a progress bar (a real percentage for downloads and unpacking, a moving bar for
  `git` and `cargo`), the current step ("Fetching changes…", "Building Axion…") and the newest line of
  the command's output. **Cancel** stops it: a download or unpacking is dropped, `git` and `cargo` are
  sent a termination signal (a stopped build leaves your previous program in place). The other update
  buttons are hidden meanwhile, and only one update runs at a time.
- **When it worked**: "Updated to X" with **Restart Axion** and **Later**. The window keeps running the
  old version until you restart, so the version line says "Axion X is installed. Restart Axion to
  finish the update." instead of showing stale information. Restarting restores your session.
- **When it failed**: the reason, what was (not) changed, the last lines of output, and **Try again**,
  **Close** and, for downloads, **Open Release Page**.

## Where Axion looks: the Repository field

**Repository** in **Settings > About & Updates** holds the project's name as `owner/name`. The
field is empty and shows the default (`xyzhenrik/Axion`) as a placeholder; clear it to go back to
the default. If your copy points to a different project,
or to a self-hosted site that offers a GitHub-compatible API, change it here or in `config.toml`
(`updates.repository`, `updates.api_url`).

For sites without such an API, set `updates.manifest_url` to the address of a `latest.json` file
(see [For maintainers](#for-maintainers)). It is used instead of the repository.

## What each kind does and does not touch

| Installation | How Axion recognises it | **Update** does | Never touches |
|---|---|---|---|
| **AppImage** | the `APPIMAGE` variable | downloads the new image next to the old one, verifies it, swaps the file | anything but that one file (and its `.old` backup) |
| **Tarball / portable folder** (`tools/install.sh`, `~/.local/opt/axion`, any extracted `Axion-X-linux-x86_64` folder) | the folder with `AppRun` and `usr/bin/axion` (and `axion-install.json` in newer tarballs) | downloads and unpacks the new tarball next to the folder, swaps the folders when you restart | settings, profile, anything outside the folder and its `.new-*` / `.old-*` neighbours |
| **Source checkout** | `target/<profile>/axion` inside a Git working tree | `git fetch`, `git pull --ff-only`, `cargo build -p axion-app --release` | your commits, branches and untracked files (never forces, merges, rebases or resets) |
| **Package or other** | none of the above | shows the notes and opens the release page | everything |

### Updating an AppImage

**Update** downloads the new AppImage next to the old one, and then:

1. checks it against the release's `SHA256SUMS` file (a mismatch stops the update),
2. checks that it really is a program file,
3. keeps the old file as `<name>.old`,
4. replaces the file in one step, so there is never a moment without a working Axion.

Then press **Restart Axion**. Very large downloads (over `updates.max_download_mb`, 1 GB) are refused.

### Updating a tarball or portable install

If you installed from `Axion-X.Y.Z-linux-x86_64.tar.gz` (with `tools/install.sh` or by extracting it
somewhere), **Update**:

1. downloads the release's `Axion-*-linux-x86_64.tar.gz` and checks it against `SHA256SUMS`
   (a mismatch stops the update; the size limit is `updates.max_download_mb`),
2. unpacks it into `<folder>.new-<version>` next to your installation, on the same disk. Entries
   that would leave the folder (absolute paths, `..`, links pointing outside, writing through a link,
   device files, hard links) reject the whole archive; permissions are kept; unpacking is capped at
   `updates.max_extracted_mb`, so a "zip bomb" cannot fill your disk,
3. checks that the new tree has `AppRun` and an executable `usr/bin/axion` of the announced version.

The running Axion is not touched while this happens. The swap is the last thing Axion does when it
**restarts** (or when you quit, if you chose **Later**): the folder becomes `<folder>.old-<oldversion>`
and the new one takes its place; if the second step fails the first is undone. Only the newest backup
is kept, and you can delete it any time. The menu entry and icons repair themselves at the next start.

If Axion cannot write to the folder or the folder around it (installed by root in `/opt`, or on a
read-only mount), it says "managed elsewhere or needs permissions" and **Open Release Page** takes you to
the download instead of failing half way.

### Updating a source checkout

If you built Axion yourself from a Git checkout, **Update from Git** runs `git fetch`, then
`git pull --ff-only` when there is something new, and rebuilds with
`cargo build -p axion-app --release`, showing the output as it goes. It only runs when:

- no **tracked** file has uncommitted changes (`git status --porcelain --untracked-files=no`).
  Untracked files such as a `todo.txt` do not matter and are left alone. The `-dirty` suffix in the
  version line (`git describe --dirty`) means exactly this: a tracked file differs.
- you are on a branch with an upstream whose remote matches the **Repository** setting.

If it refuses, the page lists the changed files (`M Cargo.toml`) and says what to do. A checkout that
is **ahead** of the remote (local commits, nothing new upstream) or has **diverged** (both) is
reported with the numbers ("1 local and 3 new commits"); Axion never merges, rebases or resets for
you. A checkout you already pulled by hand is only built. If the build fails, your previous program
stays in place. Press **Restart Axion** when it is done.

### Other installs

If a package manager installed Axion (or you copied it by hand), **Open Release Page** shows the
notes and downloads. Update it the same way you installed it.

## For maintainers

A release is what the updater reads. Axion looks at the repository's newest **published** release
(drafts are invisible), takes its tag `vX.Y.Z` as the version, picks the asset whose name ends in
`.AppImage` and contains `x86_64` (or `amd64`) (for tarball installs the `.tar.gz` named for the
architecture), downloads it, and checks it against the entry for that
file name in the release's `SHA256SUMS`. So a release needs exactly:

- the tag `vX.Y.Z` (SemVer; a pre-release such as `v0.4.0-beta.1` counts as older than `v0.4.0` and
  must be marked as a pre-release),
- `Axion-X.Y.Z-x86_64.AppImage` and `Axion-X.Y.Z-linux-x86_64.tar.gz` (the tarball contains
  `axion-install.json`, which `tools/package.sh` writes),
- `SHA256SUMS` with a line per file (`sha256sum`'s format). Without it the update still installs, but
  the confirmation says it could not be verified,
- the release notes in the description (shown in the update message).

`tools/package.sh` makes the files; the tarball `Axion-X.Y.Z-linux-x86_64.tar.gz` is for people who
install by hand.

**1. Build the release on your machine** (with the custom CEF, so H.264/AAC play):

```bash
# bump [workspace.package] version in Cargo.toml (and Cargo.lock), commit
tools/package.sh --version 0.1.0 --cef-path ~/cef-dist --out dist
tools/package.sh --notes > dist/notes.md       # skeleton from `git log`; edit it
```

`--cef-path` takes the folder that holds `libcef.so` or a folder in the layout of `third_party/cef`
(default: `$CEF_PATH`, else `third_party/cef`). The result in `dist/` is the AppImage, the tarball
and `SHA256SUMS`. The AppImage needs `appimagetool` in `PATH` (or `APPIMAGETOOL=...`), or
`mksquashfs` plus an AppImage runtime file in `~/.cache/axion/appimage-runtime-x86_64`; without
either the script still makes the tarball and says what is missing. Add `--debug-symbols` to keep the
symbols in a separate `.debug` file.

**2. Publish it** (a draft first, so you can look before the updater can see it):

```bash
git tag v0.1.0 && git push origin main v0.1.0
gh release create v0.1.0 --draft --title "Axion 0.1.0" --notes-file dist/notes.md \
  dist/Axion-0.1.0-x86_64.AppImage dist/Axion-0.1.0-linux-x86_64.tar.gz dist/SHA256SUMS
# check the draft on GitHub, then publish it:
gh release edit v0.1.0 --draft=false
```

**Without the custom CEF (GitHub does the work).** Pushing the tag is enough: the workflow
`.github/workflows/release.yml` builds on GitHub with the **stock** engine (no H.264/AAC), runs
`tools/package.sh` and creates a **draft** release with the three files and a notes skeleton.
Edit the notes and publish the draft. To ship the codec-enabled build instead, let the workflow
create the draft, then replace the AppImage and checksums with your local ones:

```bash
gh release upload v0.1.0 dist/Axion-0.1.0-x86_64.AppImage dist/Axion-0.1.0-linux-x86_64.tar.gz dist/SHA256SUMS --clobber
```

Make `SHA256SUMS` from the files you actually upload (`tools/package.sh` does): a stale checksum makes every update fail
("has no entry" or a mismatch), which is the intended safety net.

Details of the files and what a system needs: [Install](../install.md).

**Without GitHub-style releases**, host a `latest.json` on any web server and give its address in
`updates.manifest_url`:

```json
{
  "version": "0.3.1",
  "notes": "What is new in this version.",
  "url": "https://example.org/axion/releases/0.3.1",
  "prerelease": false,
  "assets": [
    {
      "name": "Axion-0.3.1-x86_64.AppImage",
      "url": "Axion-0.3.1-x86_64.AppImage",
      "size": 123456789,
      "sha256": "..."
    }
  ]
}
```

Relative asset addresses are resolved against the manifest's address.

## Config keys

All in the `[updates]` section of `config.toml`:

| Key | Meaning | Default |
|---|---|---|
| `enabled` | Master switch for update checks. | `true` |
| `repository` | `owner/name` of the releases; empty uses the default `xyzhenrik/Axion`. | empty |
| `manifest_url` | Address of a `latest.json`; wins over `repository`. | empty |
| `api_url` | The hosting site's GitHub-compatible API. | `https://api.github.com` |
| `channel` | `stable` or `prerelease`. | `stable` |
| `check_interval_hours` | Automatic checks at most this often. | `24` |
| `auto_check_on_start` | Check shortly after Axion starts. | `true` |
| `startup_delay_seconds` | How long after the start the check runs. | `8` |
| `remind_later_hours` | How long **Remind Me Later** stays quiet. | `24` |
| `timeout_seconds` | Give up on a check after this long. | `10` |
| `max_download_mb` | Larger downloads are refused. | `1024` |
| `max_extracted_mb` | A downloaded tarball may unpack to at most this much. | `4096` |
| `desktop_notification` | Also announce a new version with a desktop notification. | `true` |
| `desktop_notification_when_focused` | Send it even while Axion's window is in front. | `false` |
| `progress_interval_ms` | How often a running update refreshes its lines and bar. | `250` |

Axion also writes `last_checked`, `skipped_version`, `notified_version` and `remind_until` there.
Leave those alone.
