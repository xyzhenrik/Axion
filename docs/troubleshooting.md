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

## Microphone, camera or screen sharing do not work

First check the site's permission: click the address field's recording icon or open **Settings >
Privacy > Remembered for Sites**. A site you once answered with **Don't Allow** is refused without
asking; remove it there and reload the page. Also check that **Settings > Privacy > Site
Permissions** is not set to **Block** for it.

**No microphone or camera, or the wrong one.** Axion uses the devices your desktop has, through
PipeWire (or PulseAudio) for sound and V4L2 for cameras. Check that the device works in another app
and is the default input (KDE: **System Settings > Sound**; `wpctl status` in a terminal). Pick
another device in the site's own settings (in Discord: **User Settings > Voice & Video**). Only one
app can use some cameras at a time: close other video apps.

**Screen sharing shows no picker, or fails at once.** On Wayland it needs PipeWire and the screen cast
portal of your desktop. Install, for example on Arch:

```bash
sudo pacman -S pipewire wireplumber xdg-desktop-portal xdg-desktop-portal-kde   # KDE Plasma
sudo pacman -S pipewire wireplumber xdg-desktop-portal xdg-desktop-portal-gnome # GNOME
```

(Debian and Ubuntu use the same package names with `apt`, Fedora with `dnf`.) Then log out and in
again. `systemctl --user status xdg-desktop-portal pipewire` should show both running. Cancelling
the desktop's picker counts as refusing: the site shows an error, try again.

**Screen sharing shares everything.** Under X11 or XWayland there is no picker; the whole screen is
shared. Run Axion in a Wayland session to choose a window.

**Others cannot hear my shared screen's sound.** The sound comes from the monitor of your default
output. Check that the sound you want plays on the default output device.

**The location is never found.** Axion's engine has no location service, so sites get "position
unavailable" even when allowed. Enter your location on the site instead.

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
- On a high refresh rate monitor, pages draw at most 120 frames per second (`tabs.max_frame_rate_cap`
  in [Configuration files](configuration.md)). Heavy pages such as chat apps get smoother with a lower
  cap or a fixed `tabs.max_frame_rate`, for example 60.

If it stays slow, record a performance log (next section) and send it with your report.

## Performance logging

Axion can write one line per second about how it draws: how many frames the window showed, how
many frames the page delivered, how long each step took on the CPU and on the graphics card, the
window size, the monitor and its refresh rate, fullscreen, and how busy Axion and Chromium are.

1. Close Axion.
2. Start it from a terminal with the log turned on, and keep the output in a file:

   ```bash
   AXION_PERF_LOG=1 axion 2>&1 | tee axion-perf.log
   ```

3. Do what is slow: open the page, go fullscreen (**F11**), open the menu that lags. Keep at it for
   about 20 seconds, then a few seconds more after leaving fullscreen.
4. Quit Axion and attach `axion-perf.log` to your report, with what you did and when (for example
   "0-10 s windowed, then F11, at 20 s left fullscreen").

The lines to look at are marked `perf`. The most telling values:

| Value | Meaning |
|---|---|
| `presented_fps` | Frames the window showed per second. |
| `page_fps` / `chromium_paints_per_s` | Page frames shown / frames the page drew. Far apart means frames are dropped on the way. |
| `page_rate` | The frame rate Chromium was asked for. |
| `redraw_requests_per_s` / `redraws_per_s` | Frames asked for / frames the desktop let Axion draw. |
| `acquire_max_ms`, `present_max_ms`, `gpu_ms` | Waiting for the screen, and the graphics card's time per frame. |
| `monitor`, `refresh_hz`, `surface`, `fullscreen` | Where the window is and how big. |
| `cpu_main_pct`, `cpu_chromium_pct` | CPU use of Axion and of its Chromium processes (100 = one core). |

## Axion freezes for a moment

When the whole window stops reacting for a second or more (a click, typing, opening a page),
the stall log says what Axion was busy with.

1. Close Axion.
2. Start it from a terminal with both logs on:

   ```bash
   AXION_STALL_LOG=1 AXION_PERF_LOG=1 axion 2>&1 | tee axion-stall.log
   ```

   For a freeze you can repeat, add `AXION_STALL_STACK=1` as well: a freeze longer than a second
   then also logs where in the code Axion's main thread is stuck (this needs `eu-stack` from
   elfutils or `gdb` installed, and lets debuggers attach to Axion while it runs, so use it
   only for this).
3. Do what froze, wait until Axion reacts again, then quit it.
4. Attach `axion-stall.log` to your report, with what you did and roughly when.

The lines to look at are marked `stall`:

| Line | Meaning |
|---|---|
| `main thread busy for N ms, in "…"` | Axion stopped reacting; the quoted step is what it was doing (input, the Chromium pump, a frame, settings, …). |
| `kernel wait …, syscall …` | What it waited for: `futex` is a lock or another thread, `poll`/`epoll_wait` a socket or pipe (network, D-Bus, the clipboard), `wait4` another program, `running` busy computing. |
| `still busy after N ms` | Repeated every second while it lasts. |
| `main thread was busy for N ms` | The freeze is over: how long it lasted. |
| `main thread stack:` | With `AXION_STALL_STACK=1`: the functions it was in. |

A stall of a few hundred milliseconds while Axion starts is normal. `AXION_STALL_LOG=400` reports
only stalls longer than 400 ms.

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

## Chrome Web Store: "Add to Chrome" does nothing

Installing extensions from the Chrome Web Store is not supported; Axion does not load
extensions yet. The extension system is switched off (`[extensions] enabled = false`, the
default), so the Web Store's install button fails harmlessly. Turning it on is not recommended:
Chromium's install dialog has no window to attach to in Axion and would crash the browser.
