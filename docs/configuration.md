# Configuration files

You can do almost everything in **Settings**. Behind it are plain text files you can edit, copy and
back up. Axion watches them, so most edits apply as soon as you save.

## Where things are stored

Axion separates settings from browsing data, and each account has its own folders.

```text
~/.config/axion/                      settings (the "config base")
  accounts.toml                       the list of accounts and the last one used
  accounts/<id>/
    config.toml                       this account's settings
    keymap.toml                       this account's keyboard shortcuts

~/.local/share/axion/                 data (the "data base")
  profile/accounts/<id>/              this account's browsing data
    axion.sqlite                      history, bookmarks, downloads list, Playlist, site settings
    vault.sqlite                      the encrypted password vault
    home/backgrounds/                 start page backgrounds
    playlist/                         downloaded Playlist videos and thumbnails
    ...                               Chromium's own cache and cookies
  device.tpm or device.key            the vault's device secret (see below)
```

`<id>` is a short name Axion picks when you create an account. The ids are listed in `accounts.toml`.
Data from before accounts existed was moved into the first account on first start.

**The device secret** (`device.tpm` or `device.key`) belongs to the computer, not to an account. It is
needed to open every vault. **Never delete or lose it**, and include it in backups. See
[Accounts and privacy](features/accounts-and-privacy.md).

## `config.toml`

Settings use [TOML](https://toml.io): `[sections]` containing `key = value` lines.

- Every section and key is optional. Whatever is missing uses its default.
- Axion only saves the values that differ from the defaults, so the file stays short.
- Unknown keys or wrong types are errors. Axion then keeps the last working settings (or the defaults
  on start) and logs which file and line to look at.
- Changes in the file are picked up live. Changes made in Settings are written to the file right
  away.

The [Settings reference](settings.md) lists the key of every setting. A worked example:

```toml
# ~/.config/axion/accounts/<id>/config.toml

[general]
language = "en"                      # "system", "en" or "de"
search_url = "https://search.brave.com/search?q={}"   # {} becomes the search words

[appearance]
theme = "dark"                       # "light", "dark" or "auto"
accent = "#F2742B"
text_scale = 1.1

[glass]
style = "liquid"                     # "liquid" or "flat"
reduce_motion = false

[glass.light]
refraction = 0.45                    # 0 to 1

[layout]
tab_bar = "compact"                  # "separate" or "compact"
float_sidebar = true
bookmarks_bar = "never"              # "always", "home" or "never"
fullscreen_reveal_chrome = true      # F11: pointer at the top edge shows the bars

[tabs]
freeze_after_minutes = 5             # 0 = never
discard_after_minutes = 30
limit_memory = true
memory_budget_mb = 3072              # 0 = automatic (40 % of memory)

[shields]
default_level = "aggressive"         # "off", "standard" or "aggressive"

[home]
show_clock = true
clock_format = "24h"                 # "auto", "12h" or "24h"
background = "my-wallpaper.jpg"      # a file in home/backgrounds/
background_dim = 0.3

[downloads]
directory = "/home/me/Downloads/Axion"
ask_where_to_save = false
```

### Sections

| Section | What it holds |
|---|---|
| `general` | Homepage, search engine, language. |
| `home` | The start page: widgets, top sites, backgrounds. |
| `appearance` | Theme, accent, text size, icon weight, page colour. |
| `glass` | Liquid Glass or flat, the glass material, Reduce Transparency and Motion. |
| `colors`, `radii`, `motion` | Fine design values: neutral colours, corner radii, spring timings. |
| `layout` | Tab bar, sidebar, bookmarks bar, page card, tab widths. |
| `privacy` | Locking of private tabs. |
| `permissions` | What sites may ask for (microphone, camera, screen, notifications, …): ask or block. |
| `shields` | Level, switches, filter lists. |
| `tabs` | Freezing, unloading, memory budget, session restore, closing old tabs. |
| `media` | The video player, trusted sites, Picture in Picture, desktop integration. |
| `playlist` | The Playlist. |
| `print`, `screenshot` | Print / Save as PDF and the screenshot tool: paper, margins, formats, folders. |
| `downloads`, `data` | Download behaviour, what "Clear Browsing Data" clears. |
| `passwords` | Saving, filling, generator, clipboard. |
| `accounts` | Profile picture size and limit. |
| `import` | Limits of the import from other browsers (history entries, bookmarks, file sizes). |
| `extensions` | Chrome extensions: on or off (`enabled`), the Chrome Web Store (`web_store`), automatic updates (`auto_update`, `update_interval_hours`, `timeout_seconds`), user scripts (`allow_user_scripts`), the request APIs (`block_request_apis`) and the popup's size limits (`popup_*`). `show_in_address_bar` is gone (old files that set it still load). See [Chrome extensions](features/extensions.md). |
| `hardware` | Acceleration, page frames, graphics card. |
| `devtools`, `find`, `clipboard`, `toast`, `link_preview`, `sidebar`, `ui` | Smaller options. |
| `updates` | Update source and schedule. |

### Handy keys that have no Settings page

| Key | Meaning | Default |
|---|---|---|
| `home.top_sites_count` | How many top site tiles at most. | `5` |
| `home.top_sites_window_days` | Only visits in the last N days count. | `90` |
| `home.top_sites_columns`, `home.top_sites_rows` | Tiles per row, rows per page. | `6`, `2` |
| `home.clock_format` | `auto`, `12h` or `24h`. | `auto` |
| `home.max_backgrounds` | How many start page backgrounds you can keep. | `24` |
| `home.max_background_mb` | Largest background file. | `200` |
| `appearance.font_family` | Font of Axion's own interface. | `Inter` |
| `layout.sidebar_width` | Width of the sidebar in pixels. | `316` |
| `layout.tab_width` | Preferred tab width in pixels. | `260` |
| `layout.sidebar` | `auto`, `docked` or `overlay`. | `auto` |
| `tabs.check_interval_seconds` | How often memory is measured and tab rules applied. | `5` |
| `tabs.max_frame_rate` | Limit pages to N frames per second (0 follows the display). | `0` |
| `tabs.max_frame_rate_cap` | Following the display, pages draw at most this many frames per second (0: no cap). | `120` |
| `media.skip_seconds` | How far the skip buttons jump. | `10` |
| `media.playback_speeds` | The speeds the speed button cycles through. | `[1.0, 1.25, 1.5, 2.0, 0.5, 0.75]` |
| `media.pip_lock_aspect` | The Picture in Picture window keeps the video's shape when resized. | `true` |
| `media.pip_min_width` | Narrowest the Picture in Picture window can be made, in pixels. | `272` |
| `media.pip_max_width` | Widest it can be made, in pixels. | `1280` |
| `media.pip_max_screen_fraction` | It never grows beyond this share of its monitor, either way (0.1 to 1). | `0.9` |
| `media.pip_default_width` | Size of a first Picture in Picture window: the video fits a square of this side. | `480` |
| `window.restore_pip_size` | Open Picture in Picture as big as you last made it. | `true` |
| `passwords.generator_length` | Length of generated passwords. | `20` |
| `passwords.generator_symbols` | Generated passwords include symbols. | `true` |
| `find.match_case` | Find on page matches upper and lower case exactly. | `false` |
| `privacy.prompt_unlock_on_open` | Show the password card by itself when locked private tabs open. | `true` |
| `permissions.screen_picker` | Who picks what `getDisplayMedia()` shares: `auto` (Axion's own picker on X11, the desktop's portal dialog on Wayland), `own` (Axion's picker wherever it can list sources), `system` (Chromium and the desktop decide). | `"auto"` |
| `permissions.screen_picker_max_sources` | Most screens and windows the picker lists (screens first). | `12` |
| `permissions.screen_picker_thumbnail_width` | Width of the picker's thumbnails in logical pixels. | `160` |
| `permissions.stop_reload_after_ms` | After **Stop** in the recording popover, a page that still records this long after is reloaded. | `1500` |
| `ui.tooltip_delay_ms` | How long before a tooltip appears. | `700` |
| `toast.duration_seconds` | How long short notices stay. | `2.0` |
| `print.paper` | Paper of **Save as PDF**: `a4`, `a3`, `a5`, `letter`, `legal`. The dialog keeps its last choices in `print.*`. | `a4` |
| `print.landscape`, `print.backgrounds`, `print.header_footer` | Landscape, background colours and images, title / address / page numbers. | `false` |
| `print.margins` | `none`, `narrow`, `default`, `wide`. | `default` |
| `print.margin_narrow_inches`, `print.margin_default_inches`, `print.margin_wide_inches` | Width of the margin presets. | `0.2`, `0.4`, `1.0` |
| `print.scale_percent` | Content scale in percent (25 to 200). | `100` |
| `print.directory` | Folder for PDFs when **Ask where to save each file** is off (empty: the download folder). | empty |
| `print.open_after_saving` | Open the PDF in a tab once it is saved. | `false` |
| `screenshot.format` | `png` or `jpeg`. | `png` |
| `screenshot.jpeg_quality` | JPEG quality, 1 to 100. | `90` |
| `screenshot.after_capture` | `ask` (show it with Copy / Save), `copy` or `save`. | `ask` |
| `screenshot.directory` | Folder for screenshots when **Ask where to save each file** is off (empty: the download folder). | empty |
| `screenshot.full_page_max_height` | A full page screenshot stops at this height (CSS pixels). | `16000` |
| `screenshot.scroll_settle_ms` | A full page screenshot scrolls a screen at a time and waits this long after each scroll (ms). | `250` |
| `screenshot.dim_opacity` | How dark the page gets around the selection (0 to 1). | `0.45` |
| `screenshot.min_selection` | Smaller drags are ignored (logical pixels). | `8` |

## `keymap.toml`

Shortcuts, one line per action. See [Keyboard shortcuts](keyboard-shortcuts.md) for the names and the
rules.

```toml
focus_address_bar = ["Alt+D", "Ctrl+E"]
tab_overview = []            # no key
```

## `accounts.toml`

The list of accounts (id, name, colour, creation time) and `last_used`. Switching accounts and
editing them in Settings update it. To start Axion with a specific account use
`axion --account=ID`.

## Environment variables

Set these before starting Axion, for example `AXION_GPU=low axion`. They are meant for one-off runs
and tests.

| Variable | Effect |
|---|---|
| `AXION_CONFIG_DIR` | Use this folder instead of `~/.config/axion` for settings. |
| `AXION_IMPORT_HOME` | Look for other browsers to import from in this folder (a fake home with `.config/…`, `.mozilla/…`) instead of `$HOME`. For tests and screenshots. |
| `AXION_PROFILE_DIR` | Use this folder instead of `~/.local/share/axion/profile` for browsing data. |
| `AXION_DEVICE_KEY_DIR` | Use this folder instead of `~/.local/share/axion` for the vault's device secret. |
| `AXION_GPU` | `low`, `high` or part of a card's name. Overrides **Graphics card**. |
| `AXION_OSR` | `software` or `gpu`. Overrides **Page frames**. Never use `gpu` on AMD graphics. |
| `AXION_MONITOR` | Open the window on this monitor, for example `HDMI-A-1`. Works on X11 and XWayland (start with `env -u WAYLAND_DISPLAY`). |
| `AXION_CHROMIUM_SWITCHES` | Extra Chromium command-line switches, separated by commas, for example `AXION_CHROMIUM_SWITCHES=autoplay-policy=no-user-gesture-required`. |
| `AXION_NO_SANDBOX` | Set to `1` to turn off Chromium's sandbox, for containers and tests. Unsafe for browsing: see [Install](install.md#the-sandbox). The packaged launcher refuses to start when the sandbox cannot work, unless this is set. |
| `AXION_SANDBOX_CHECK` | Set to `0` to skip the packaged launcher's up-front test of user-namespace support (the AppImage and the tarball's `axion`). Chromium still needs a working sandbox. |
| `AXION_FAKE_MEDIA_DEVICES` | Set to `1` for tests: pages get Chromium's fake camera (a test pattern) and microphone (a beep) instead of your real ones. The permission prompt still asks. |

To try things out safely, point the first three at empty temporary folders. Your real accounts,
settings and vault key are then not touched.

## Back up, move and reset

**Back up.** Close Axion, then copy these:

- `~/.config/axion/` (settings and shortcuts)
- `~/.local/share/axion/` (browsing data **and** the device secret)

Restoring both on the same computer brings everything back. On a different computer the vault cannot
be opened, because the device secret does not travel. Use **Export Encrypted Copy** in
**Settings > Passwords** for your passwords, and restore them there.

**Reset one setting or all settings of an account.** Close Axion and delete the line, or the whole
`config.toml` of the account. Defaults return at the next start. Keep a copy first.

**Reset shortcuts.** Delete `keymap.toml` of the account.

**Start fresh.** Move `~/.config/axion` and `~/.local/share/axion/profile` away. Leave
`device.key` or `device.tpm` where it is, or your old vault stays unreadable.
