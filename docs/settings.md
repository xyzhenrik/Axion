# Settings reference

Open Settings with **Ctrl+,** (or **Ctrl+S**), or from the page menu. The list on the left has one
page per topic. The search field at the top finds a setting by name (in English or German).

![Settings, Appearance page](../assets/screenshots/appearance-light.webp)

Every change is saved at once. A few changes need a restart; those say so.

**How to read the tables**

- **Setting** is the label you see in the app.
- **Default** is what a fresh install uses.
- **Config key** is the name in `config.toml`, written as `section.key`. You never need it unless you
  edit the file by hand. See [Configuration files](configuration.md).
- Sliders show their range in the description.

Pages, in the order of the list:
[Account](#account) -
[General](#general) -
[Appearance](#appearance) -
[Liquid Glass](#liquid-glass) -
[Tabs & Memory](#tabs--memory) -
[Media](#media) -
[Reading & Translation](#reading--translation) -
[Keyboard](#keyboard) -
[Shields](#shields) -
[Privacy](#privacy) -
[Data & Downloads](#data--downloads) -
[Passwords](#passwords) -
[Hardware](#hardware) -
[About & Updates](#about--updates)

---

## Account

Settings for the account you are using. Each account has its own tabs, history, bookmarks,
passwords, website data and settings. More in [Accounts and privacy](features/accounts-and-privacy.md).

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Name** (and **Save**) | The account's name, shown in the sidebar and in the greeting on the start page. | Your system user name | Stored in `accounts.toml` |
| **Photo** (**Choose Photo…**, **Remove Photo**) | A profile picture, picked with your desktop's file chooser. It is cropped to a square and copied into the account, so deleting the original is safe. Without a photo, coloured initials show. | None | `accounts.avatar_size_px` (256), `accounts.avatar_max_mb` (20) set the size and the largest file accepted |
| **Colour** | The colour of the initials circle. | First swatch | Stored in `accounts.toml` |
| **Password** (**Set Password** / **Change Password**) | One password for the whole account. It unlocks **Passwords** and the **Private** tabs. Axion cannot recover it if you forget it. | No password | Never stored |
| **Delete Account…** | Removes the account with its tabs, history, bookmarks, passwords, website data and settings. Click again to confirm. Not available for the only account. | - | - |

## General

![The start page](../assets/screenshots/start-light.webp)

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Homepage** | The page that opens in new windows and tabs. Leave the field empty for Axion's own start page. | Axion Home (`axion://home/`) | `general.homepage` |
| **Search engine** | Where searches typed in the address field and on the start page go. Choices: DuckDuckGo, Google, Brave, Startpage, Ecosia. | DuckDuckGo | `general.search_url` |
| **Language** | The language of Axion's own interface: System, English or Deutsch. Applies after you restart Axion. | System | `general.language` (`system`, `en`, `de`) |
| **Add Axion to the application menu** | Linux: adds Axion's desktop entry and icons for your user, which gives the window its icon in the taskbar and the application menu (on Wayland this is the only way). Applies after you restart Axion. | On | `general.desktop_integration` |
| **Welcome tour** (**Show Welcome Tour**) | Runs the first-start introduction again. | - | `general.onboarding_done` |
| **Show home button** | Puts a home button next to back and forward. It opens your homepage. | On | `layout.home_button` |

### Start page

What the start page shows. The same switches are in the gear on the page itself. See
[Start page](features/start-page.md).

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Clock** | Shows the large clock. | On | `home.show_clock` |
| **Greeting** | Shows "Good morning, name" under the clock. | On | `home.show_greeting` |
| **Search field** | Shows the search field. | On | `home.show_search` |
| **Top sites** | Shows tiles for your pinned and most visited sites. | On | `home.show_top_sites` |
| **Shields statistics** | Shows how many trackers and ads were blocked, and the bandwidth and time saved. | On | `home.show_stats` |

### Start page background

| Setting | What it does | Default | Config key |
|---|---|---|---|
| Background picker | Choose **None**, one of your backgrounds, or add one. Click the cross on a card to remove it. | None | `home.background` |
| **Dim** | Darkens the background so text stays readable. Shown once a background is selected (search lists it as "Dim background image"). 0 to 80 %. | 20 % | `home.background_dim` |
| **Blur** | Blurs the background. Shown once a background is selected ("Blur background image" in search). Off to 30 px. | Off | `home.background_blur` |
| **Chrome colour as default background** | Without a background image the start page uses the neutral grey of the window around the page (light or dark) instead of plain white or black. Private tabs stay dark. | On | `home.glass_ground` |
| **Live wallpapers (animated images and videos)** | Lets animated images (GIF, WebP) and videos (WebM, MP4) play, muted and looping. Off: they show a still frame. | On | `home.live_wallpapers` |
| **Play live wallpapers with Reduce Motion** | Keeps live wallpapers playing while **Reduce motion** is on. Off: they show a still frame then. | On | `home.live_wallpapers_ignore_reduce_motion` |
| **Add from file** (**Add**) | Type a path such as `~/Pictures/sea.jpg` and press **Add**. The file is copied into your account. MP4 needs H.264 support. | - | - |

### Developer

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Developer tools** | Allows Chromium's developer tools (F12, Ctrl+Shift+I) and view source (Ctrl+U). Off: none of these shortcuts do anything. See [Developer tools](features/developer-tools.md). | On | `devtools.enabled` |

## Appearance

How Axion looks. See [Appearance and Liquid Glass](features/appearance-and-glass.md).

![Appearance in dark mode](../assets/screenshots/appearance-dark.webp)

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Appearance** | **Automatic** follows your desktop's light or dark preference, or force **Light** or **Dark**. | Automatic | `appearance.theme` (`auto`, `light`, `dark`) |
| **Glass** | **Liquid Glass** is the full glass look. **Flat** uses solid cards without refraction, blur or bounce. It is much lighter on weak graphics hardware. | Liquid Glass | `glass.style` (`liquid`, `flat`) |
| **Accent colour** | The colour of selections, focus rings and primary buttons. Six swatches. | Blue | `appearance.accent` (a colour such as `"#2E6BFF"`) |
| **Icon weight** | The line thickness of icons: **Regular**, **Medium** or **Bold**. | Medium | `appearance.icon_stroke_light`, `appearance.icon_stroke_dark` |
| **Favicon contrast** (config only) | What happens to website icons that would be hard to see on the current bar (for example a black icon on the dark bar): `invert` flips single-colour icons, `backing` puts them on a light or dark tile, `off` leaves them as they are. | `invert` | `appearance.favicon_contrast` |
| **Light or dark chrome to match the page** | Switches the toolbar between light and dark to match the page's top edge. Off: it follows **Appearance** only. | Off | `appearance.adapt_to_page` |
| **Text size** | Scales all of Axion's own text. 85 to 130 %. Web pages have their own zoom (Ctrl++, Ctrl+-). | 100 % | `appearance.text_scale` |
| **Page corner radius** | How round the corners of the page card are. 0 to 24 px. | 10 px | `layout.page_corner_radius` |
| **Float sidebar over the page** | Always floats the sidebar over the page as a glass card instead of docking it beside the page. | Off | `layout.float_sidebar` |
| **Show where a link leads** | A small glass bubble at the bottom corner of the page shows the address of the link under the pointer (also links inside text, image links and links reached with Tab): the host stands out, the path is dimmer, and a very long address is shortened in the middle. After a moment it grows to show the whole address on two lines. It moves to the other corner when the pointer comes close. `javascript:` links show nothing. | On | `link_preview.enabled` |
| **Link bubble timing** (config only) | `link_preview.show_delay_ms` (120) before it appears, `link_preview.hide_delay_ms` (80) before it goes, `link_preview.expand_after_ms` (1200) before it shows the whole address, `link_preview.max_width_fraction` (0.6 of the page width), `link_preview.pointer_clearance` (36 px; closer than this to the pointer it moves to the other corner), `link_preview.hide_javascript_urls` (true). | | `link_preview.*` |

## Liquid Glass

Fine-tune the glass material. These settings only matter with **Glass** set to **Liquid Glass**. See
[Appearance and Liquid Glass](features/appearance-and-glass.md).

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Reduce transparency** | Replaces glass with opaque surfaces. | Off | `glass.reduce_transparency` |
| **Reduce motion** | Replaces springs and stretching with short fades. | Off | `glass.reduce_motion` |
| Motion springs | Apple's presets, `response` = SwiftUI duration, `damping` = damping fraction (`bounce` = 1 - damping): `snappy` 0.5 / 0.85 (bounce 0.15), `smooth` 0.5 / 1.0, `bouncy` 0.5 / 0.7, `interactive` 0.15 / 0.86 (following the pointer). `liquid`, `drain*`, `fling`, `tab_close`, `switch` are Axion's own (see docs/physics.md). `speed` scales every response. | see left | `motion.*` |
| Switch animation | The spring of a switch's knob (travel, and swelling into a glass lens while held) and of segmented controls: `response` is the period in seconds, `damping` the damping ratio. The default settles in about 0.2 s with a small bounce. Edit in `config.toml`; Reduce motion snaps it. | `response` 0.18, `damping` 0.80 | `motion.switch` |
| Tap hold | The shortest time (seconds) a click or touchpad tap holds the press swell of a glass button before it springs back, so even a tap that is over within one frame animates. 0 turns it off; Reduce motion has no swell anyway. Edit in `config.toml`. | 0.09 | `motion.tap_hold` |
| **Refraction** | How strongly the glass bends what is behind it. 0 to 100 %. | 55 % | `glass.light.refraction`, `glass.dark.refraction` |
| **Blur** | How much the backdrop is blurred, as a share of the normal amount for each glass thickness. 0 to 200 %. | 100 % | `glass.light.blur`, `glass.dark.blur` (`thin`, `regular`, `thick`) |
| **Colour dispersion** | Colour fringes along the glass rim. 0 to 40 %. | 12 % (10 % in dark mode) | `glass.light.dispersion`, `glass.dark.dispersion` |
| **Squash when moving** | How far glass stretches along the direction you drag it. 0 to 20 %. | 8 % | `glass.squash` |
| **Drag stretch (maximum)** | The most a glass control (a held button, a switch or slider knob) stretches when you pull it far from where it rests; icons, labels and everything inside the glass are deformed exactly like it (same transform about its centre, also in button groups). 0 to 20 %. Off with Reduce motion and in flat mode. | 5 % | `glass.drag_stretch_max` |
| Drag range (minimum) | Advanced, `config.toml` only. The rubber band's range `d` is the control's size but at least this many logical px, so small buttons need a long pull to approach the maximum. | 120 | `glass.drag_range_min` |
| Rubber-band constant | Advanced, `config.toml` only. The drag curve is UIScrollView's rubber band `(1 - 1/(x·c/d + 1))·d` over the control's size `d`; the maximum stays **Drag stretch (maximum)**. Smaller `c` is stiffer. | 0.55 (Apple's) | `glass.rubber_band_c` (`glass.drag_resistance` is deprecated: still accepted, ignored) |
| **Merge distance** | How close two glass shapes must be before they flow together. 0 to 40 px. | 20 px | `glass.merge_distance` |
| Menu gap | Clear space between a menu that stands apart from its button (the Playlist's speed and sleep menus, above their buttons) and that button. Such a menu is a card of its own: no liquid bridge, its glass never flows into the button's. Edit in `config.toml`. | 10 px | `glass.menu_anchor_gap` |
| Button shadow | Share of the glass drop shadow that a glass button casts. 0 (the default) means buttons are pure refractive glass with nothing dark around them; panels, sheets and popovers keep their shadow. Edit in `config.toml`. | 0 | `glass.button_shadow` |
| Reduced lens opacity | Reduce Transparency: how strongly a held knob's lens (switch, slider, segmented pill) is tinted. Stays see-through so the label or track under it remains visible. Edit in `config.toml`. | 0.22 | `glass.reduced_lens_opacity` |

## Tabs & Memory

How tabs look and how Axion saves memory. See [Tabs and memory](features/tabs-and-memory.md).

| Setting | What it does | Default | Config key |
|---|---|---|---|
| Tab bar picture (**Separate** / **Compact**) | **Separate** puts tabs in their own row below the address bar. **Compact** uses one row where all tabs are the same width and the current tab holds the address; the address field widens while you type (`layout.compact_address_width`). | Separate | `layout.tab_bar` (`separate`, `compact`) |
| **Colour in tab bar** | Tints the window around the page with a colour taken from the page, like Safari. | Off | `appearance.tint_from_page` |
| **Page colour** (**Extreme** / **Dominant**) | Shown when **Colour in tab bar** is on. **Extreme** takes the darkest or lightest prominent colour. **Dominant** takes the main colour, evened out. Turning the colour on switches from neutral to **Extreme**. | Neutral (page colour off) | `appearance.tint_mode` (`neutral`, `extreme`, `dominant`) |
| **Open new tabs in background** | Tabs opened from links do not take over the screen. | On | `tabs.open_links_in_background` |
| **Animate closing tabs** | A closed tab shrinks and fades away (tab bar, sidebar and tab overview) while the tabs beside it slide into the gap. With **Reduce motion** or the flat glass style only a short fade stays. The spring is `motion.tab_close`. | On | `tabs.close_animation` |
| **Keep tab widths while closing** | After closing a tab with the pointer in the tab bar, the other tabs keep their widths until the pointer leaves the bar, so the next close button stays under it. | On | `tabs.keep_widths_while_closing` |
| **Middle click closes tabs** | A middle click on a tab (bar, sidebar or tab overview) closes it. | On | `tabs.middle_click_closes_tab` |
| **New tabs join the group they open from** | A tab opened from a tab of a tab group (a link, **New Tab to the Right**) joins that group. | On | `tabs.new_tab_joins_group` |
| **Show tab group names in the tab bar** | Tab groups get a coloured chip with their name in front of their tabs. Off: only a colour line under the tabs. | On | `tabs.group_chips_in_bar` |
| **Show a speaker on tabs playing sound** | Tabs that play sound, or are muted, show a speaker in the tab bar and the sidebar. Click it to mute or unmute. | On | `tabs.show_audio_indicator` |
| **Remember muted sites** | **Mute Site** in a tab's menu is remembered: tabs of that site start muted. | On | `tabs.remember_site_mute` |
| Closed tabs remembered | How many closed tabs **Reopen Closed Tab** and the tab search remember. | 25 | `tabs.closed_tabs_remembered` |
| History results in the tab search | At most this many history entries in the tab search (0: none). | 8 | `tabs.tab_search_history_results` |
| **Drag tabs out as links** | Dragging a tab out of the window into another app drops its address there (a text field gets the link). | On | `tabs.drag_out_links` (`tabs.drag_out_distance`: how far outside the window counts as out, default 12 px) |
| **Show a preview while dragging a tab out** | The dragged tab shows as a small card (a snapshot of the page with the site icon and title; just icon, title and host when there is no snapshot) beside the pointer, like in Firefox. Wayland (native) and X11. | On | `tabs.drag_out_image` (`tabs.drag_out_image_width`: card width, default 200 logical px; `tabs.drag_out_image_opacity`: default 0.9) |
| **Close tabs** | Closes tabs you have not looked at for a while: **Manually** (never), **After a Day**, **After a Week**, **After a Month**. Pinned tabs and tabs playing media stay. | Manually | `tabs.close_after_days` (0, 1, 7, 30) |
| **Bookmarks bar** | When the bookmarks bar shows: **Always**, **On Home Page**, **Never**. | Always | `layout.bookmarks_bar` (`always`, `home`, `never`) |

### Memory

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Freeze background tabs after** | Pauses scripts and timers of a tab after this long in the background. Never to 60 min. | 10 min | `tabs.freeze_after_minutes` |
| **Unload background tabs after** | Unloads a background tab after this long, keeping only its address and a picture. It reloads when you open it. Never to 8 h. | 2 h | `tabs.discard_after_minutes` |
| **Unload tabs when memory runs over the budget** | When Axion uses more memory than the budget, unloads background tabs, the longest unused first. | On | `tabs.limit_memory` |
| **Memory budget** | The budget for the whole browser. **Automatic** is 40 % of your computer's memory. Otherwise pick an amount in steps of 256 MB, up to 6 GB. | Automatic | `tabs.memory_budget_mb` (0 = automatic) |
| **Keep tabs playing audio awake** | Tabs that play sound or video are never frozen or unloaded. | On | `tabs.keep_media_awake` |
| **Reopen last session's tabs at start** | Brings back your tabs when Axion starts. They load when you first look at them. | On | `tabs.restore_session` |

The last line of the page shows how much memory Axion uses right now and the current budget.

## Media

Video, audio and the Playlist. See [Media and Playlist](features/media-and-playlist.md).

![The Playlist page](../assets/screenshots/playlist-dark.webp)

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Axion's player in fullscreen** | When a video goes fullscreen, Axion's own glass controls take over. Inline, pages keep their own controls. | On | `media.fullscreen_player` |
| **Show the title in fullscreen** | Axion's fullscreen player shows what plays at the top centre, as `Title - Channel - Site` in a glass pill between the buttons (from the page's media session, else the page title, else the file name). A too-long title is cut with an ellipsis first, then the channel; the site stays. Hides with the other controls. | On | `media.fullscreen_title` |
| **Ignore video previews** | Hover previews and autoplaying tiles (YouTube's home, search and subscription grids, Twitch's directory) are never the Now Playing card, the desktop media controls or a reason to keep a tab awake. Real playback (watch page, Shorts, miniplayer) is unaffected. | On | `media.detect_previews` |
| Preview width limit | A controls-less video that loops or autoplays and is narrower than this part (0 to 1) of the page counts as a preview; 0 turns the size rule off. Config file only. | 0.4 | `media.preview_max_width` |
| **Hide controls while playing after** | Hides the controls after this long without mouse movement. 1 to 10 s. | 2.5 s | `media.hide_controls_after_seconds` |

### Playlist

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Playlist button in the address field** | Shows the Add to Playlist button on pages with a video. | On | `playlist.quick_action` |
| **Download videos when adding them to the Playlist** | Downloads a video for offline use as soon as you add it, when it is a plain video file. | Off | `playlist.auto_download` |
| **Delete downloaded files when removing videos** | Removing a video from the Playlist also deletes its downloaded file. | On | `playlist.delete_files_on_remove` |
| **Playlist sort** | **Newest First**, **Oldest First** or **Custom Order** (the order you arranged in Edit mode). | Newest First | `playlist.sort` (`newest`, `oldest`, `manual`) |
| **Playlist downloads** (**Change…**, **Default**) | The folder for downloaded videos. Changing it affects new downloads only. | Axion's data folder of the account | `playlist.directory` |

### Sites that keep their own player

A list of sites whose own video player stays in place, also in fullscreen. Type a site name such as
`example.com` under **Add a site** and press **Add**. Subdomains count too. Use the cross to remove
a site. Changes apply to pages loaded next.

| Setting | What it does | Default | Config key |
|---|---|---|---|
| Trusted sites list | Sites that keep their own player. | YouTube, Twitch, Vimeo, Dailymotion, Netflix, Disney+, Prime Video, Crunchyroll, Spotify, SoundCloud | `media.trusted_player_sites` |
| Ignored media sites | Sites whose video/audio elements are calls or chats (Discord…), not media: no player, no Now Playing, no Playlist entry. Camera, microphone and call streams are skipped everywhere. | Discord | `media.ignored_media_sites` |
| Loading indicator | A turning ring on the players while a video loads or buffers. | On | `media.loading_indicator` |
| Picture-in-Picture veil | Opacity of a black veil over the whole Picture-in-Picture picture while its controls show. 0 (the default): none; the controls stay legible through their own glass tint. Edit in `config.toml`. | 0 | `media.controls_scrim` |
| Cover size | Side of the cover picture (the item's artwork or thumbnail) the fullscreen player and Picture-in-Picture show for sound without a picture, as a share of the shorter side. Edit in `config.toml`. | 0.55 | `media.cover_size` |
| Ring speed | Seconds per turn of that ring. | 0.9 | `media.loading_spin_seconds` |
| Keep paused player | How long a paused player that left the page keeps the Now Playing card. | 30 s | `media.now_playing_keep_seconds` |

### Playback

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Hardware video decoding (after restart)** | Decodes video on the graphics card (VA-API). The same switch is on the Hardware page. | On | `media.hardware_decoding` |
| **Desktop media controls and media keys (after restart)** | Shows what is playing in your desktop's media widgets and reacts to your keyboard's media keys (MPRIS). | On | `media.desktop_integration` |

## Reading & Translation

Reader View and page translation. See [Reading and translation](features/reading-and-translation.md).

### Reader View

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Reader View in the page menu** | Offers Reader View in the page menu (≡) on pages that have an article. Off: the Reader row is dimmed and the shortcut does nothing. The key kept its old name from when this was an Aa button in the address field. | On | `reader.button` |
| **Remember Reader View's look for each site** | Reader Options' choices belong to the site you are on (kept in the profile). Off: they are the same everywhere and saved in `config.toml`. | On | `reader.remember_per_site` |
| **Font** | Serif, Sans or Mono. | Serif | `reader.font` |
| **Theme** | Auto (light or dark like Axion), Light, Sepia or Dark. | Auto | `reader.theme` |
| *(Reader Options)* Text size | Size of the text in pixels. | 20 | `reader.font_size` |
| *(Reader Options)* Column width | Width of the text column in pixels. | 680 | `reader.width` |
| *(file only)* Line height | Multiple of the text size. | 1.6 | `reader.line_height` |
| *(file only)* Article threshold | A page counts as readable with at least this many characters of text. | 600 | `reader.min_chars` |
| *(file only)* Check delay | Milliseconds after loading before a page is checked for an article. | 700 | `reader.detect_delay_ms` |
| *(file only)* Reader Options limits | Smallest, largest and step of text size and column width. | 14 / 36 / 2 and 480 / 1000 / 80 | `reader.min_font_size`, `reader.max_font_size`, `reader.font_step`, `reader.min_width`, `reader.max_width`, `reader.width_step` |

### Translation

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Offer to translate pages in other languages** | Shows the Translate button when a page is in a language other than the target and can be translated. | On | `translation.offer` |
| **Open the offer by itself** | The offer opens once when such a page has loaded. | On | `translation.prompt` |
| **Translate text that appears later** | New text on a translated page (feeds, comments) is translated too. | On | `translation.translate_changes` |
| **Translate pages into** | The target language: Interface (Axion's own language), English, German, French or Spanish. Any other code from the list below can be set in the file. | Interface | `translation.target_language` (empty = interface) |
| **Translate** | Where translating runs: **On this computer** (Bergamot models, nothing leaves the computer) or **Your server** (a LibreTranslate-compatible server; page text is sent to it). | On this computer | `translation.backend` (`local` or `endpoint`) |
| **Downloaded language data** | Deletes the models downloaded for local translation. | - | - |
| *(file only)* Server address | LibreTranslate-compatible endpoint, for example `http://localhost:5000/translate`. Only used with `backend = "endpoint"`; empty by default. | empty | `translation.endpoint_url` |
| *(file only)* Server API key | Sent with each request when set. | empty | `translation.endpoint_api_key` |
| *(file only)* Server timeout | Seconds one request may take. | 30 | `translation.endpoint_timeout_seconds` |
| *(Translate popover)* Never for this site | Sites (and their subdomains) that are never offered. | none | `translation.never_sites` |
| *(file only)* Known languages | Languages you read: pages in them are not offered. | none | `translation.known_languages` |
| *(file only)* Batch size | At most this many characters and pieces of text are translated per request. | 4000 / 60 | `translation.batch_chars`, `translation.batch_pieces` |
| *(file only)* Language detection | Pages without a declared language are guessed from at least this many characters. | 120 | `translation.detect_min_chars` |
| *(file only)* Engine idle time | The hidden page that runs the local engine closes after this many seconds without work, which frees the models' memory. | 120 | `translation.engine_idle_seconds` |
| *(file only)* Translation scope | `visible` (default): the part of the page you see plus the margin below is translated first, the rest while you scroll. `page`: the visible part first, then the whole page in the background. | `visible` | `translation.scope` |
| *(file only)* Viewport margin | With `visible`: how many screens above and below the window are translated ahead of scrolling. | 1.0 | `translation.viewport_margin` |
| *(file only)* First request size | The first request carries at most this many pieces of text, so the first translation appears quickly. | 12 | `translation.first_pieces` |
| *(file only)* Requests in flight | Requests a page keeps waiting for the engine at once. | 2 | `translation.inflight_batches` |
| *(file only)* Engine workers | Translation threads working in parallel (0: half the processor cores, at most 4). Each holds its own copy of the model in memory. | 0 | `translation.engine_workers` |
| *(file only)* Warm up the engine | Starts the engine and loads the model while the offer is showing, so the first translation doesn't wait for them. | On | `translation.prewarm` |
| *(file only)* Model sources | Where the model list and the files come from (Mozilla Remote Settings). | Mozilla's servers | `translation.models_records_url`, `translation.models_attachments_url` |

Local translation covers English, German, French, Spanish, Italian, Portuguese, Dutch, Polish,
Russian, Ukrainian, Czech, Swedish, Danish, Norwegian, Finnish, Hungarian, Romanian, Bulgarian,
Greek, Turkish, Arabic, Japanese and Korean. Pairs without English go through English.

## Keyboard

A searchable list of all shortcuts, grouped by area. To change one, edit `keymap.toml`; edits
apply immediately. See [Keyboard shortcuts](keyboard-shortcuts.md).

### Command palette and page zoom

Both are controlled in `config.toml` only (no Settings page yet).

| Setting | What it does | Default | Config key |
|---|---|---|---|
| Command palette (**Ctrl+K**) | A search sheet over every browser action with its shortcut; Up/Down choose, Enter runs, Esc closes. New actions appear in it by themselves. | - | keymap action `command_palette` |
| Palette rows | How many commands are listed at once. | 8 | `command_palette.max_rows` |
| Palette shortcuts | Shows each command's shortcut. | On | `command_palette.show_shortcuts` |
| Palette width | Width of the sheet in logical pixels (shrinks with the window). | 560 | `command_palette.width` |
| Default zoom | Zoom of sites without a zoom of their own (percent). | 100 | `zoom.default_percent` |
| Zoom step | One **Ctrl +** / **Ctrl -** step, in Chromium zoom levels (each level is 1.2x; 0.5 gives 100, 110, 120, 131 ...). | 0.5 | `zoom.step` |
| Zoom limits | Smallest and largest zoom (percent). | 25 / 500 | `zoom.min_percent`, `zoom.max_percent` |
| Remember zoom per site | Zoom set with **Ctrl +/-/0** is remembered for the site (all its pages and tabs; not in private tabs). Off: it belongs to the tab and resets when the next page loads. | On | `zoom.remember_per_site` |
| Zoom indicator | Shows the zoom in the address field while it is not the default; a click resets it. | On | `zoom.show_indicator` |
| Share selected text | The page menu's **Share...** row for selected text opens a new e-mail with the text and the page's address. | On | `ui.page_context_menu_share` |

## Shields

Blocking of ads, trackers, popups and redirects. See [Shields](features/shields.md).

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Shields on sites you haven't set** | The level for every site that has no level of its own: **Off**, **Standard** or **Aggressive**. | Standard | `shields.default_level` (`off`, `standard`, `aggressive`) |
| **Hide empty ad spaces** | Hides the gaps and placeholders that blocked ads leave behind. | On | `shields.hide_ad_elements` |
| **Block popups opened without a click** | Blocks tabs and windows a page opens by itself. | On | `shields.block_popups` |
| **Block redirects to ad and tracker sites** | Blocks pages that send you elsewhere without a click, to sites on the block lists. | On | `shields.block_redirects` |
| **Remove tracking parameters from links** | Cuts click-tracking parts such as `fbclid` and `gclid` out of addresses. | On | `shields.strip_tracking_parameters` |
| **Block third-party cookies** | Stops sites from setting cookies for other sites. | On | `shields.block_third_party_cookies` |
| **Filter lists** (**Update Now**) | The block lists in use and how often they refresh. **Update Now** refreshes them at once. | 5 lists, every 96 hours | `shields.lists`, `shields.update_interval_hours` |

## Network & VPN

Details and threat model: [network.md](network.md). Connections keep their passwords and keys in
the password vault, not in `config.toml`.

| Setting (`[network]`) | Default | What it does |
|---|---|---|
| Connections (`connections`) | none | Proxies (SOCKS5, HTTP, HTTPS), WireGuard (import a `.conf`) and Tor. |
| Tor options (per Tor connection: `use_bridges`, `bridges`, `exit_country`, `isolation`) | per site | `isolation` = `site` (own circuits per site), `context` (shared) or `none`. Plain bridges only. Not Tor Browser: no fingerprinting resistance. See [network.md](network.md#tor). |
| `default_connection` | `direct` | Route of this account's tabs. |
| `private_connection` | unset | Route of private tabs (unset: same as the account). |
| `site_rules` | none | `host` + `connection`: a site (and its subdomains) with its own route. Beats both defaults. |
| `kill_switch` | on | A down connection blocks its requests (with an explanation page) instead of going direct. |
| `leak_protection` | on | WebRTC, DNS prefetch, secure DNS and background services stay off the real network. |
| `disable_quic_when_routed` | on | QUIC off while a route exists (after restart). |
| `strict_dns` | off | When everything is routed, no name is resolved on this computer (after restart). |
| `check_ip_url` | empty | Address the "Check My IP" button opens in a tab. Axion never calls it itself. |
| `front_trust_own_processes` | on | The loopback front lets in Axion's own processes (Chromium) without a login, found by process tree (Linux). Everyone else needs the per-session login. |
| `connect_timeout_secs`, `health_check_secs`, `max_relay_connections` | 10, 10, 512 | Timing and limits (after restart). |
| `wireguard_handshake_timeout_secs` | 20 | How long a WireGuard handshake may stay unanswered before the connection is shown as failed (wrong key, dead server and blocked UDP look the same). |
| `connect_at_start`, `show_chip`, `notify_changes` | on | Start default routes at launch; show the chip in the address field; toast on changes. |

## Privacy

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Lock private tabs with the account password** | Private tabs ask for the account password before they open. Needs an account password; the page offers to create one. | On | `privacy.lock_private_tabs` |
| **Browsing history** (**Clear History…**) | Deletes your whole browsing history. Click again to confirm. For a time range, use **Clear Browsing Data…** | - | - |

### Extensions

Chrome extensions (Manifest V3). They are installed from the Chrome Web Store, or from the
extensions button in the toolbar; see [Chrome extensions](features/extensions.md).

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Chrome extensions (after restart)** | Runs Chromium's extension system and shows the extensions button. Off: no extension loads. | On | `extensions.enabled` |
| **Install from the Chrome Web Store (after restart)** | "Add to Chrome" on the Chrome Web Store installs the extension, after Axion shows what it can do and you say yes. Off: the store is not opened. | On | `extensions.web_store` |
| **Update Web Store extensions automatically** | Looks for newer versions in the background and downloads them. They run after the next restart. An update that asks for more permissions waits for your yes. | On | `extensions.auto_update` |

Config file only: `extensions.update_interval_hours` (24), `extensions.timeout_seconds` (60),
`extensions.allow_user_scripts` (on), `extensions.block_request_apis` (on) and the popup's size
limits and timing: `extensions.popup_min_size` (25), `extensions.popup_max_width` (800),
`extensions.popup_max_height` (600), `extensions.popup_measure_ms` (150) and
`extensions.popup_load_timeout_ms` (3000). See
[Chrome extensions](features/extensions.md#how-it-works).

### Site Permissions

What happens when a site asks for your microphone, camera, screen and the like and you have not
decided for that site yet. **Ask** shows the permission prompt under the address field; **Block**
refuses without asking. A decision you made for a site wins over these. See
[Site permissions](features/site-permissions.md).

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Microphone** | Calls and voice chat (`getUserMedia` with audio). | Ask | `permissions.microphone` (`ask`, `block`) |
| **Camera** | Video calls (`getUserMedia` with video). | Ask | `permissions.camera` |
| **Screen sharing** | Sharing a screen or window (Discord, Meet). Always asks: allowing is never remembered. | Ask | `permissions.screen_sharing` |
| **Notifications** | Desktop notifications from the site. | Ask | `permissions.notifications` |
| **Location** | Your position. | Ask | `permissions.location` |
| **Clipboard** | Reading what you copied (writing to the clipboard needs no permission). | Ask | `permissions.clipboard` |
| **Other permissions** | MIDI devices, the local network, window placement on all displays, installed fonts, idle detection, pointer and keyboard lock, motion sensors, several downloads at once. | Ask | `permissions.other` |

### Remembered for Sites

| Setting | What it does | Default | Config key |
|---|---|---|---|
| Decision list (**Reset All**) | Every site decision you made with **Always Allow** or **Don't Allow**: the site, what, and **Allowed** or **Blocked**. Remove one with its cross, or all with **Reset All** (click again to confirm). The site asks again next time. | - | - |

Decisions in private tabs are kept in memory only and are forgotten when Axion quits; they are not
listed here.

## Data & Downloads

### Downloads

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Download location** (**Change…**) | The folder for downloads. | Your desktop's Downloads folder | `downloads.directory` |
| **Ask where to save each file** | Shows a file chooser for each download. Off: saves straight into the download location. | On | `downloads.ask_where_to_save` |
| **Open “safe” files after downloading** | Opens pictures, movies, sounds, PDF and text documents when they finish. | Off | `downloads.open_safe_files` |
| **Remove download list items** | When finished downloads leave the list: **Manually**, **After One Day**, **When Axion Quits**. | Manually | `downloads.remove_list_items` (`manually`, `after_one_day`, `on_quit`) |

### Print and screenshots

Choices in the **Print or Save as PDF** dialog (Ctrl+P, or **Print or Save as PDF…** in the page menu) and in the
screenshot tool (Ctrl+Shift+S, or **Screenshot…**). The dialog remembers its last choices; the rest are config
file keys (see [Configuration](configuration.md)). Both save the way downloads do: with a file chooser when
**Ask where to save each file** is on, else straight into their folder.

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Paper** (dialog) | A4, A3, A5, Letter or Legal. | A4 | `print.paper` (`a4`, `a3`, `a5`, `letter`, `legal`) |
| **Orientation** (dialog) | Portrait or landscape. | Portrait | `print.landscape` |
| **Margins** (dialog) | None, Narrow, Default or Wide. | Default | `print.margins` (`none`, `narrow`, `default`, `wide`) |
| **Scale** (dialog) | Content scale in steps of 10%. | 100% | `print.scale_percent` |
| **Backgrounds** (dialog) | Print background colours and images. | Off | `print.backgrounds` |
| **Header and footer** (dialog) | Title, date, address and page numbers. | Off | `print.header_footer` |
| PDF folder | Where PDFs go when not asking. | The download location | `print.directory` |
| Open PDF after saving | Shows the PDF in a tab. | Off | `print.open_after_saving` |
| Screenshot format | PNG or JPEG. | PNG | `screenshot.format` (`png`, `jpeg`) |
| After taking a screenshot | Show it with **Copy** and **Save**, or copy / save at once. | Show | `screenshot.after_capture` (`ask`, `copy`, `save`) |
| Screenshot folder | Where screenshots go when not asking. | The download location | `screenshot.directory` |

### Import from another browser

Brings bookmarks and history from the other browsers on this computer into the current account.
Axion only reads the other browser's files (it copies its databases to a temporary file first, so a
running browser is no problem) and never changes them. Passwords and cookies are not imported:
other browsers encrypt them with the desktop keyring. Export your passwords there as a CSV file and
import it under [Settings > Passwords](#import-and-export).

| Setting | What it does | Default | Config key |
|---|---|---|---|
| Browser list | The browsers and profiles found (Chrome, Chromium, Brave, Edge, Vivaldi, Opera; Firefox, LibreWolf, Zen, Waterfox, Floorp; also their Flatpak and Snap versions), each with the number of bookmarks and history entries. Tick the ones to import; the most recently used one is ticked at first. | - | - |
| **Bookmarks**, **History** | What to import from the ticked browsers. | Both | - |
| **Import** | Starts the import in the background. The result is shown under the list (for example “Imported 312 bookmarks and 8,450 history entries; 40 already existed”) and as a notification. The sidebar lists, the bookmarks bar and the start page's top sites update by themselves. | - | - |
| **Import from File…** | Imports bookmarks from a file: the HTML export every browser can make (**Export bookmarks**), a Chromium `Bookmarks` file or a Firefox JSON backup. | - | - |
| **Open Passwords** | Goes to the Passwords page. | - | - |
| Limit for history | At most this many history entries are imported per browser profile, the most recently visited first. | 20000 | `import.max_history_items` |
| Limit for bookmarks | At most this many bookmarks per profile or file. | 50000 | `import.max_bookmarks` |
| Size limit for databases | Larger browser databases are skipped (MB). They are copied to a temporary file, so this much space is needed. | 512 | `import.max_database_mb` |
| Size limit for files | Larger bookmark files are refused (MB). | 64 | `import.max_file_mb` |

Entries you already have (same address) are kept as they are and counted as “already existed”, so
importing twice is safe. Bookmarks from a browser's bookmarks bar land on Axion's bookmarks bar; those
in other folders appear in the sidebar's Bookmarks list with their old folder (for example
“Dev / Rust”). Imports always go into the account that is open. The limits have no controls in
Settings; change them in `config.toml`.

### Clear browsing data

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Time range** | How far back to clear: **Last Hour**, **Today**, **Last 7 Days**, **All Time**. A range clears cookies only for sites you visited in it. The cache is always cleared completely. | All Time | `data.clear_range` (`last_hour`, `today`, `last_week`, `all_time`) |
| **Browsing history** | Include history. | On | `data.clear_history` |
| **Cookies and other site data** | Include cookies and site data. | On | `data.clear_cookies` |
| **Cached images and files** | Include the cache. Its size is shown. | On | `data.clear_cache` |
| **Downloads list** | Include the downloads list (not the files). | On | `data.clear_downloads` |
| **Clear Browsing Data…** | Clears what is ticked. Click again to confirm. | - | - |

The ticks are remembered for the next time.

### Cookies and website data

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Clear cookies and site data when Axion quits** | Deletes all cookies and site data on every quit. You are signed out of websites each time. | Off | `data.clear_cookies_on_quit` |
| Site list (**Remove All**) | Lists the sites that store cookies, with the number of cookies. Remove one with its cross or all with **Remove All**. | - | - |

## Passwords

The password manager. See [Passwords](features/passwords.md).

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Open Passwords** | Opens your saved passwords, cards and addresses. | - | - |
| **Offer to save passwords** | After you sign in to a site, asks whether to save the password. | On | `passwords.offer_to_save` |
| **Offer to fill saved passwords** | Offers saved logins on sign-in forms. | On | `passwords.autofill` |
| **Clear clipboard after copying** | Empties the clipboard this long after you copy a password or card number. Never to 5 min. | 60 s | `passwords.clear_clipboard_seconds` |

### Import and export

Type the path of a file in **File** and, for exports and restores, the account **Password**. Then
use a button.

| Button | What it does |
|---|---|
| **Import CSV** | Imports logins from a CSV file exported by Chrome, Firefox or Bitwarden. |
| **Export Encrypted Copy** | Writes an encrypted copy of your vault. It can be restored on any device. |
| **Restore Axion Export** | Reads such an export back into your vault. |

## Hardware

Graphics card and acceleration. Changes apply after a restart; a **Restart Axion to Apply** button
appears when needed. See [Hardware acceleration](features/hardware-acceleration.md).

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Use hardware acceleration** | Master switch. Off: Chromium draws pages, video and canvas in software. Axion's own interface always needs the graphics card. | On | `hardware.enabled` |
| **Hardware video decoding** | Decodes video on the graphics card (VA-API). Also on the Media page. | On | `media.hardware_decoding` |
| **Smooth WebGL, canvas and GPU rasterization** | Lets Chromium draw web graphics on the graphics card. Off adds software drawing. | On | `hardware.gpu_rasterization` |
| **Page frames** | How finished page pictures reach Axion: **Automatic**, **Shared GPU memory** or **Software**. | Automatic | `hardware.page_frames` (`auto`, `gpu`, `software`) |
| **Graphics card** | Which card to use: **Automatic**, **Integrated** (saves power) or **Dedicated** (fastest). | Automatic | `hardware.gpu` (`auto`, `low`, `high`) |
| **Copy Diagnostics** | Copies a plain-text summary (card, driver, mode, Chromium version) to the clipboard for bug reports. | - | - |

The page also lists the graphics cards found (**In use**, **Also available**) and a **GPU status**
block showing what is really active.

## About & Updates

Version information and updates. See [Updates](features/updates.md).

| Setting | What it does | Default | Config key |
|---|---|---|---|
| **Check for Updates** | Asks the update source for the newest version now. | - | - |
| **Check for updates automatically** | Looks for a new version a few seconds after Axion starts, at most once a day. | On | `updates.auto_check_on_start` |
| **Include pre-releases** | Also offers alpha, beta and release-candidate versions. | Off | `updates.channel` (`stable`, `prerelease`) |
| **Repository** (**Save**) | Where releases are published, written `owner/name`. The field is empty and shows the default as a placeholder; typing your own repository overrides it, clearing the field returns to the default (nothing is stored for the default). | `xyzhenrik/Axion` (empty field) | `updates.repository` |

When an update is available the page also shows the release notes and the buttons **Update** (or
**Update from Git**, or **Open Release Page**), **Skip This Version** and **Remind Me Later**.

---

Settings you cannot find here (timings, sizes, limits) live only in `config.toml`. The file format
and the most useful extra keys are in [Configuration files](configuration.md).
