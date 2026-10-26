# Troubleshooting

## Some videos do not play (H.264 / AAC)

Axion's Chromium engine in its standard build cannot play **H.264 video and AAC audio**, because
those codecs are not part of the freely distributed engine. Many video sites and hosters use them.
**VP8, VP9, AV1, Opus and Vorbis** (what YouTube and most big players offer) work out of the box.

Symptoms: a black player, a message that the format is not supported, or a video that never starts.
MP4 start page wallpapers also need H.264; WebM wallpapers always play.

To fix it, build the engine with the codecs yourself. From a source checkout:

```bash
tools/build-cef.sh
```

This takes **several hours and about 100 GB of disk space**, so start it and let it run. Set
`CEF_BUILD_DIR` to choose where it works (default `~/cef-build`). When it is done, point `CEF_PATH`
at the result before you build or run Axion.

## The graphics driver crashes on AMD

On AMD graphics with the RADV driver, setting **Page frames** to **Shared GPU memory** can crash the
driver (a GPU fault, followed by a short freeze). Fix:

1. Set **Settings > Hardware > Page frames** to **Automatic** or **Software**. Axion asks before
   it lets you pick **Shared GPU memory** on AMD.
2. Make sure no `AXION_OSR=gpu` is set in your environment or launcher.
3. Restart Axion (**Restart Axion to Apply**).

If Axion will not even open, start it once with `AXION_OSR=software axion`. The variable wins over
the setting. See [Hardware acceleration](features/hardware-acceleration.md).

## Everything is slow or choppy

- Switch **Settings > Appearance > Glass** to **Flat**. It skips all glass drawing and helps a lot on
  integrated graphics.
- On **Settings > Hardware**, check that **Graphics card** and **GPU status** show the card you expect.
  Pick **Dedicated** if you have one.
- Turn on **Reduce transparency** and **Reduce motion** on the **Liquid Glass** page.
- Lower the memory budget or the freeze time (see [Tabs and memory](features/tabs-and-memory.md)).

## The window has no icon on Wayland

On Wayland, the desktop takes the window and taskbar icon from a desktop entry (`axion.desktop`), not
from the window. Axion adds one for your user by itself: at start it writes
`~/.local/share/applications/axion.desktop` (pointing at the Axion you started) and the icons in
`~/.local/share/icons/hicolor`, then refreshes the desktop caches in the background. This is
**Settings > General > Add Axion to the application menu** (`general.desktop_integration`, on by
default).

- The very first start after this can still show the generic icon, because the desktop only learns
  about a new entry a moment after the window opened. Restart Axion once; the icon is there from then
  on. If it is still missing, sign out and in, or run `kbuildsycoca6 --noincremental` on KDE.
- Axion rewrites the entry when you start it from another place (for example a new build) or after an
  update. It never removes the entry: delete `axion.desktop` and the `axion.png` / `axion.svg` icons
  yourself, or run `tools/install-desktop.sh --uninstall`, and turn the setting off first.
- Only a switch you turned off, or an unusual setup (no writable `~/.local/share`), needs the manual
  route. The script in the source folder does the same by hand:

```bash
tools/install-desktop.sh                   # for target/release/axion in this checkout
tools/install-desktop.sh --bin /path/to/axion
tools/install-desktop.sh --uninstall       # remove it again
```

It is safe to run more than once.

## I cannot open my private tabs

Private tabs are locked with your account password (**Settings > Privacy > Lock private tabs with
the account password**). Open the **Private** section and enter the password. If you have no password
yet, the lock does nothing until you set one in **Settings > Account**.

- **Ctrl+L** or **Lock** in the account menu locks again.
- To stop locking, turn the setting off in **Settings > Privacy**.

## I forgot my account password

Axion cannot recover it, on purpose. Your saved passwords are encrypted with it. What you can do:

- If you have an **encrypted export** (see [Passwords](features/passwords.md)), it needs the
  password it was made with.
- Otherwise, the saved passwords cannot be read. You can add a new account for a fresh start.

To avoid this in the future, make an encrypted export now and keep the password somewhere safe.

## "These passwords belong to another device"

The vault opens only with the **device secret** of the computer it was created on. This message means
the secret is missing or different: you copied the profile to another computer, reinstalled the
system, deleted `~/.local/share/axion/device.key` or `device.tpm`, or the TPM was reset.

- Put the original `device.key` / `device.tpm` back if you have it.
- Otherwise restore from an export: **Settings > Passwords > Restore Axion Export**.

See [Accounts and privacy](features/accounts-and-privacy.md#how-your-passwords-are-protected).

## The TPM does not work

Axion uses the TPM when `/dev/tpmrm0` is available to your user (your user must be in the `tss`
group). Without it, Axion falls back to a protected key file, which works as well for most
purposes. If a TPM becomes usable later, the secret moves there by itself. A build without TPM
support is made with `--no-default-features`.

## The first build cannot download the engine

The first build downloads about 1 GB of Chromium Embedded Framework into `third_party/cef`. If it
stops with a download or unpacking error:

- Check your internet connection and that you have several GB of free disk space.
- Run the build again.
- If it still fails, delete the `third_party/cef` folder and build again to get a fresh download.

## Settings do not apply or look wrong after editing `config.toml`

An unknown key, a wrong type or a typo makes the file invalid. Axion keeps the last working settings
and writes the problem to its log, including the file and line. Fix the line, or delete it. See
[Configuration files](configuration.md).

## A website is broken

Try lowering Shields for that one site: click the shield button and choose **Off** for the site. If
that fixes it, leave it that way for the site. See [Shields](features/shields.md). Do not turn
Shields off globally.

## Report a bug

When you report a problem, a few facts help a lot.

1. Open **Settings > Hardware** and press **Copy Diagnostics**. Paste the text into your report. It
   lists your graphics card, driver, the page frames mode, whether video decoding is requested and the
   Chromium version.
2. Open **Settings > About & Updates** and note the **Version** and how Axion was installed.
3. Say what you did, what you expected and what happened. A screenshot helps.
4. If Axion crashed or misbehaved, start it from a terminal (`axion`) and copy the last lines it
   prints.

Do not include passwords, vault files or the contents of `device.key`.
