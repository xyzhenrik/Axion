# Updates

Axion can tell you when a new version is out and, depending on how you installed it, update itself.
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

## Where Axion looks: the Repository field

**Repository** in **Settings > About & Updates** holds the project's name as `owner/name`. Empty
means updates are off and Axion never contacts a server. If your copy points to a different project,
or to a self-hosted site that offers a GitHub-compatible API, change it here or in `config.toml`
(`updates.repository`, `updates.api_url`).

For sites without such an API, set `updates.manifest_url` to the address of a `latest.json` file
(see [For maintainers](#for-maintainers)). It is used instead of the repository.

## Updating an AppImage

If you run Axion as an AppImage, **Update** downloads the new AppImage next to the old one, and then:

1. checks it against the release's `SHA256SUMS` file (a mismatch stops the update),
2. checks that it really is a program file,
3. keeps the old file as `<name>.old`,
4. replaces the file in one step, so there is never a moment without a working Axion.

Then press **Relaunch Now**. Very large downloads (over 1 GB) are refused.

## Updating a source checkout

If you built Axion yourself from a Git checkout, **Update from Git** runs `git pull --ff-only` and
rebuilds with `cargo build -p axion-app --release`, showing the output as it goes. It only runs when:

- you have no uncommitted changes,
- you are on a branch whose remote matches the **Repository** setting.

It never forces anything, never resets or merges. If the build fails, your previous program stays in
place. Press **Relaunch Now** when it is done.

## Other installs

If a package manager installed Axion (or you copied it by hand), **Open Release Page** shows the
notes and downloads. Update it the same way you installed it.

## For maintainers

Publish each version as a release on the project's hosting:

1. Tag it `vX.Y.Z` (for example `v0.3.1`). Versions use SemVer; a pre-release such as `v0.4.0-beta.1`
   counts as older than the final `v0.4.0`. Mark pre-releases as such.
2. Attach an AppImage named `Axion-X.Y.Z-x86_64.AppImage`.
3. Attach a `SHA256SUMS` file with the checksum of every file, for example from
   `sha256sum Axion-*.AppImage > SHA256SUMS`. A release without checksums still installs, but the
   confirmation says that it could not be verified.
4. Put the release notes in the release description.

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
| `repository` | `owner/name` of the releases. | `xyzhenrik/Axion` |
| `manifest_url` | Address of a `latest.json`; wins over `repository`. | empty |
| `api_url` | The hosting site's GitHub-compatible API. | `https://api.github.com` |
| `channel` | `stable` or `prerelease`. | `stable` |
| `check_interval_hours` | Automatic checks at most this often. | `24` |
| `auto_check_on_start` | Check shortly after Axion starts. | `true` |
| `startup_delay_seconds` | How long after the start the check runs. | `8` |
| `remind_later_hours` | How long **Remind Me Later** stays quiet. | `24` |
| `timeout_seconds` | Give up on a check after this long. | `10` |
| `max_download_mb` | Larger downloads are refused. | `1024` |

Axion also writes `last_checked`, `skipped_version`, `notified_version` and `remind_until` there.
Leave those alone.
