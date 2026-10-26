# Axion Browser — Plan

Status: **Phase 5b done except PiP** (2026-10-25; Phase 3 leftovers open) · Target: v0.1 (daily-drivable on Linux)

## 1. Vision

A fast, memory-conscious browser for Linux that looks and feels like iPadOS 26 Safari,
with genuine Liquid Glass, Brave-level blocking, and every knob exposed.

## 2. Feature spec (v0.1)

### 2.1 Layout (iPadOS 26 Safari)
- **Compact top bar**: floating glass capsule containing tabs + address field; the page
  scrolls *underneath* it and is visibly refracted.
- **Tab bar**: tabs as glass pills; active tab expands into the address field; drag to
  reorder (with physics), drag out to detach/close.
- **Sidebar** (toggle, resizable, collapsible): Tab Groups, Bookmarks, History, Shields
  report. Floats as a glass panel over the page, or docks and pushes content (setting).
- **Tab overview**: grid of live/snapshot thumbnails, pinch-in (touchpad) or shortcut to
  open; zoom transition from tab to card; swipe card away to close; search field.
- **Responsive**: layout switches between *wide* (sidebar docked), *regular* (sidebar
  overlay) and *narrow* (bottom-ish compact bar) by window width.
- **Start page** (`axion://home/`, default homepage and new tab page): clock + greeting,
  search field with engine picker, top sites (most visited, pinned/edited/removed, pages with
  dots), Shields stats (blocked, ≈ bandwidth and time saved), background images (several,
  copied into `<profile>/home/backgrounds/`, picked from thumbnails in the gear panel and in
  Settings › General, dim/blur), each toggled from a glass gear panel on the page; controls
  use Axion's accent colour; private tabs get a dark private variant. **Live wallpapers**:
  animated GIF/WebP/APNG and muted looping videos (webm, mp4 with H.264) as backgrounds
  (`home.live_wallpapers`, `home.max_background_mb`); paused while hidden/frozen, still frame
  only if the user opts in to follow Reduce Motion (`home.live_wallpapers_ignore_reduce_motion`, default on = they play); the media bridge ignores them (no media tab, no audio icon, no MPRIS).

### 2.2 Liquid Glass
- SDF-based shapes (rounded rects, capsules, circles) with lens-profile refraction.
- Smooth-min merging of nearby elements (tab pills merge into the bar, menus "drip" out of buttons).
- Chromatic dispersion, backdrop blur, specular rim, luminance-adaptive tint.
- Physics: elements follow the pointer when held, stretch along drag direction,
  fling with velocity, spring back, wobble on release.
- Settings: intensity, blur radius, dispersion, tint, motion scale; Reduce Transparency /
  Reduce Motion; performance mode for weak GPUs.

### 2.3 Shields
- Network blocking via `adblock-rust` (EasyList, EasyPrivacy, uBO lists; user-addable lists, auto-update).
- Cosmetic filtering + scriptlets injected per frame.
- **Popups**: block any `window.open`/new window without a user gesture; allow-list per site.
- **Redirects**: block top-level cross-site navigations not caused by a user gesture,
  meta-refresh/JS redirect chains to known ad/tracker domains, and "tab-under" tricks.
- Tracking: strip known tracking query params, block third-party cookies by default,
  fingerprinting mitigations (canvas/audio noise, reduced UA client hints).
- Per-site shield panel (glass popover from the address bar) with counters and toggles:
  Off / Standard / Aggressive.

### 2.3b Private browsing
- Private tabs (separate in-memory profile: no history, cookies or cache kept on disk).
- The "Private" section can be **locked with the account password** (like Safari's locked
  private browsing). One unlock per session (user decision): entering the password anywhere
  unlocks private tabs and password filling until Axion quits or Lock is used; no idle
  re-lock. Opening the Passwords overview and card numbers/CVVs ask again every time.
- Tabs move between normal and private by dragging them in the sidebar (the address reopens
  in the other group; into locked private tabs only after unlocking).

### 2.3c Local accounts
- Each account is a fully separate profile (like Chrome's): tabs, private tabs, history,
  bookmarks, downloads list, favicons, password vault, website data, Shields site settings and
  its own `config.toml`/`keymap.toml`.
- One password per account: it is the vault's master password and the private tabs' lock;
  asked only when needed (locked private tabs, Passwords, filling/saving a password).
- Account button at the bottom of the sidebar (avatar + name) with a glass menu: accounts,
  Add Account…, Account Settings…, Lock.
- Profile pictures: "Choose Photo…" (desktop file chooser) in Account Settings and the Add
  Account sheet. The picture is copied once into the account's directory (`avatar.png`,
  centre-cropped to a square, `accounts.avatar_size_px`, input limit `accounts.avatar_max_mb`),
  so deleting the original never breaks it; removing/replacing/deleting the account deletes
  the file. Without one the coloured initials show. Capture aids: `--open=account-avatar:PATH`,
  `add-account-avatar:PATH`, `account-avatar-remove`.

### 2.4 Tabs & memory
- Lifecycle states: **Active → Background → Frozen → Discarded**.
- Freeze after N minutes in background (`Page.setWebLifecycleState: frozen`).
- Discard after M minutes or when over the memory budget (close CEF browser; keep
  URL, nav history, scroll position, form state where possible, last snapshot).
- Exceptions: pinned tabs, audio playing, active downloads, user-exempt sites.
- Memory budget (MB) for all renderers; renderer process limit; live per-tab memory view
  in the sidebar.
- All thresholds configurable.

### 2.5 Bookmarks & history
- SQLite store, sidebar UI, full-text search from the address bar.
- Import from Firefox/Chromium profiles (bookmarks + history).
- Session restore on startup.

### 2.6 Input
- Keyboard (decision 2026-10-26): tabs are `Alt` + key (Alt+T new, Alt+Q close, Alt+Shift+T
  reopen, Alt+Up/Down or Alt+PageUp/PageDown prev/next, Alt+1…8/9 select, Alt+P pin, Alt+O or
  F9 overview); browser/system things are `Ctrl` + key (Alt+D / F6 / Ctrl+K address, Ctrl+L lock (private tabs + password filling), Ctrl+R reload,
  Ctrl+Shift+L sidebar, Ctrl+B bookmarks, Ctrl+H history, Ctrl+D bookmark page, Ctrl+P
  passwords, Ctrl+F find, Ctrl+G next match, Ctrl+S or Ctrl+, settings). Ctrl+Tab /
  Ctrl+Shift+Tab also cycle tabs (wrapping). Alt+Left/Right stay back/forward; Ctrl+W/T
  are free for pages. Only defaults changed: explicit `keymap.toml` entries still win, and
  their chords are removed from other actions' defaults. Fully remappable in `keymap.toml`
  and the settings UI; conflict detection.
- Touchpad: two-finger swipe back/forward with physics preview, pinch for tab overview,
  smooth scrolling forwarded to the page.
- Developer: F12 / Ctrl+Shift+I developer tools, Ctrl+Shift+J on the Console, Ctrl+U page source (see 2.9).
- Mouse: drag tabs/glass elements, middle-click behaviours, mouse back/forward buttons.

### 2.7 Media player & fullscreen (priority 1)
- **Own fullscreen player** (decision 2026-10-25: fullscreen only): on the page, sites keep their
  own controls. When a video goes fullscreen, Axion's Safari-style glass player takes over —
  play/pause, ±10 s, volume, speed, captions, PiP — auto-hiding after inactivity.
  Driven via a small injected bridge (`HTMLMediaElement` API ↔ IPC). Trusted sites (YouTube,
  Twitch, …) keep their own player also in fullscreen; the list is edited in Settings › Media.
- **Picture-in-Picture**: our own floating glass window, draggable with physics, snaps to corners.
- **Now Playing** in the sidebar + MPRIS integration (media keys, desktop widgets).
- **Playback has priority 1**:
  - tabs playing media are never frozen/discarded and are exempt from the memory budget;
  - their renderer gets raised OS priority, background tabs are throttled harder while media plays;
  - hardware video decoding (VA-API) enabled by default;
  - the compositor never drops video frames for UI effects — glass degrades first
    (frame pacing locked to video cadence while in fullscreen).
- Keybinds: Space, ←/→, ↑/↓, F, M, C, `<`/`>` — remappable like everything else.

### 2.7b Axion Playlist (videos for later, optionally offline)
- **Adding.** Pages with a video get a Playlist button in the address field (quick action,
  `playlist.quick_action`; tooltip with its shortcut): it drips out a popover with the page's
  videos (usually one) — *Add*, and *Add and Download* when the video is a plain file; a video
  already saved shows *Open Playlist* / *Remove* instead. A glass button in the fullscreen
  player (third circle, top left) adds exactly the video that plays (a check once saved), the
  page menu has *Add to Playlist* / *Show Playlist*, and the keymap has `playlist_add`
  (Ctrl+Shift+D, next to Ctrl+D) and `open_playlist` (Ctrl+Shift+P). Private tabs never add
  (button hidden, the shortcut says so).
- **What is saved** (table `playlist` in the account's `axion.sqlite`, so per account): title
  (media-session title, else the tab title), page address, the media file's address when it is
  a plain file (`currentSrc` from the bridge, never `blob:`), thumbnail file in
  `<profile>/playlist/thumbs`, duration, added time, position, watched flag, download state /
  path / size / note, list name (`playlist.list_name`, default "Playlist") and an order.
  The same file (or, for streams, the same page) is never added twice; the downloaded copy
  counts as the same item.
- **Thumbnails.** The page's own picture first (video `poster`, media-session artwork,
  `og:image`; fetched through the engine's network stack so Shields and cookies apply), else a
  frame of the video taken from the tab's rendered picture (CPU read-back of the video's
  rectangle), for downloaded files a frame once they play. Rows without any show the site's
  icon on a neutral glass tile.
- **Downloads** go through the engine's download machinery (`Engine::start_download`, folder
  `playlist.directory`, default `<profile>/playlist` of the account (Axion's own data, not
  ~/Downloads), no save dialog) and the
  Downloads list; Shields' request policy is asked first (blocked = failed with a note).
  Plain files (mp4/webm/audio…) download with progress in the row; **HLS / DASH / `blob:`
  streams can't be fetched** (the row says "Not downloaded"; the *Download* action says why on click) — never a silent
  failure. The wording lives in `playlist.rs` (`DOWNLOAD_ACTION` …). Removing an item deletes its downloaded file (`playlist.delete_files_on_remove`,
  never a file that was the user's own `file://` video); "Remove Download" keeps the item.
- **The page** (native Axion UI, not an `axion://` page; `playlist_page.rs`): it fills the
  window to the right of the sidebar, the toolbar/address/tab row are hidden (the locked-private
  layout, only the sidebar button stays) and the sidebar remains fully usable. A second column
  lists the videos: round play-all, list name with a menu chevron, "N items 17h 17min", *Edit*
  (select, remove, download, move up/down), rows with a rounded thumbnail (download / watched
  badges), up to three title lines, length and state, a "…" menu (Play, Download / Remove
  Download, Show in Folder, Open Page, Remove) and the playing row highlighted.
  The right side is the player: PiP, an expanding glass search field (filters live) and ×
  (Esc closes) on top, the video area (black, rounded), title, progress with elapsed/remaining,
  shuffle / previous / play / next / repeat (off, all, one), speed menu, sleep timer menu
  (`playlist.sleep_timer_minutes`, end of video) and fullscreen.
- **Playing in the page: only the video, never the site.** Adding captures the real media
  source (`axion_media::MediaSource`: address, kind file / HLS / DASH, and the `Referer` the
  page sent): the `<video>`'s `currentSrc` (also inside embedded hoster iframes; the bridge
  reports each player's frame address), and for `blob:` / MSE players the manifest or file the
  page's network traffic showed (the engine reports `.m3u8` / `.mpd` / video responses as
  `EngineEvent::MediaSniffed`, stream pieces excluded). It is stored with the item (`source`
  column). Playback uses a *media view* — a CEF view of its own on `axion://media/` (a blank
  page with one `<video>`, our bridge, no site scripts, no navigation, no popups) with the
  stored `Referer` set on its requests and Shields still judging them as requests of the
  original page; files play natively, HLS with the vendored hls.js 1.5.17 (Apache-2.0, licence
  in `crates/axion-media/src/media_view/`), downloaded files as a plain `file://` document.
  Our controls drive it through the bridge protocol. It resumes at the saved position (a
  little before: `playlist.resume_rewind_seconds`), saves it every
  `playlist.position_save_seconds` and marks the video watched at `playlist.watched_fraction`;
  tabs showing a saved video (page or downloaded file) save their position too.
  Items saved without a source are *resolved lazily* on first play: the page loads in a hidden,
  muted view (its player clicked now and then, popups ignored) just long enough to capture the
  source, which is then cached; the player area says "Finding video…". A source that stops
  working (signed links expire) is resolved again once. What can't be captured or played (DRM,
  MSE streams that show no manifest, DASH, unreachable) shows a message in the player area with
  an *Open page* button that opens the page in a normal tab; the page is never loaded in the
  player area. Fullscreen opens Axion's fullscreen player directly on the media view (no tab),
  and Picture in Picture plays it in the PiP window; both are generalised to "a view with media
  state" (tab or Playlist playback), at the same position.
- **Opening, order and resume** (decisions 2026-10-26). Opening the page starts the topmost video
  (`playlist.autoplay_on_open`, default on) unless something plays already (kept, e.g. from PiP);
  an empty list says "No Videos". The order is one rule: the list is sorted by `playlist.sort`
  (enum `newest` default / `oldest` by added time / `manual` = the Edit move up/down order; the
  page's sort button next to *Edit* and Settings › Media › Playlist › Sort set it, saved in the
  config); then, with `playlist.continue_on_top` (default on), the most recently played
  *unfinished* item (`played_at` column, set when playback starts) is lifted to the top. Finished
  items (watched fraction reached, or watched and reset) are never lifted and keep their sorted
  place. Autoplay-on-open therefore resumes the item you were watching, also after a restart.
  Moving an item by hand turns the current display order into the manual order and switches to
  `manual`. Positions are stored per item (`position`, `watched`) and written on: every
  `position_save_seconds` while playing, pause, switching item, end, closing the page, closing a
  tab, and quit / window close (`Browser::drop`, synchronous SQLite write; pending settings are
  flushed too). Resume starts `resume_rewind_seconds` earlier; finished videos start from 0.
  When a video ends the next one starts (`playlist.autoadvance`, default on; repeat-one wins).
- Settings › Media › Playlist: button on/off, auto-download, delete files on remove, list name
  (renaming moves the items), folder. While the page is open the sidebar's selection highlight
  sits on the *Playlist* row; its menus grow out of their buttons (`Placement::Over` / `OverCentered`).

### 2.8 Customization
- Settings UI (glass sheet) + `~/.config/axion/config.toml`, live-reloaded.
- Themes: accent colour, light/dark/auto, glass parameters, density, corner radius.

### 2.9 Developer tools (user decision: Chromium's own, unchanged - we do not build one)
- `Engine::open_devtools(view, &DevToolsOptions)` / `close_devtools(view)` / `devtools_open(view)` /
  `inspect_element(view, x, y)` (for a context menu's "Inspect": opens the tools with that element
  selected, or selects it in the open window). CEF backend: `browser.host().show_dev_tools(...)`.
- **Mechanism (decided 2026-10-26, verified under XWayland): option (a), Chromium's own DevTools
  window.** CEF 156 runs the Chrome runtime; a windowed `WindowInfo` passed to `show_dev_tools`
  makes Chromium create a normal top-level window (Views/Ozone) next to our OSR page browsers, so it
  needs no parent window from us and no `--remote-debugging-port`. Stock Elements, Console, Sources,
  Network ... all work; the window title is "DevTools - <page url>" and its X11 class / Wayland app id
  is `axion` (`CHROME_DESKTOP=axion.desktop` is set first thing in `run_subprocess_if_needed`, else
  the window inherits the launching program's, which was the wrong desktop entry/icon).
- Rejected: DevTools as an Axion tab. Passing a *windowless* `WindowInfo` to `show_dev_tools` is
  refused by the Chrome runtime ("Windowless rendering is not supported for this DevTools window",
  `chrome_browser_delegate.cc`), so a tab would need the frontend served by a remote-debugging port
  (`--remote-debugging-port` on 127.0.0.1 + `/devtools/inspector.html?ws=...`). That port is
  reachable by every local process (and, with a DNS-rebinding bug, maybe by pages), so it stays
  unbuilt unless the window turns out to be a problem on native Wayland. `open_in = tab` is
  therefore not offered.
- The DevTools window is a browser of its own in the engine (`ViewState::devtools_of`, no tab, no
  events for the app, no page script / bridge / password scripts, no history); it closes with its
  page (`close_view`), on shutdown the engine closes the tools first (`close_browser` on a
  DevTools browser hangs), and "Open in new tab" in the tools opens a normal Axion tab through the
  inspected page's popup path. Private tabs work (same context as the page); Shields apply to the
  page as before and are not touched.
- A tab with open tools is never frozen or discarded (`TabInfo::devtools_open`, like media tabs;
  `Engine::discard` refuses it too); the window is no tab, so nothing is saved in the session.
- Keymap (`keymap.toml` names; Settings > Keyboard, heading "Developer"): `devtools` = F12,
  Ctrl+Shift+I (toggles the window), `devtools_console` = Ctrl+Shift+J (opens on the Console panel
  through `DevToolsAPI.showPanel`, or switches an open window to it), `view_source` = Ctrl+U (new tab
  `view-source:<url>` for http(s)/file pages; not recorded in the history).
- Config `[devtools] enabled = true` (Settings > General > Developer): off, none of the three
  shortcuts, nor the page menu's *Developer Tools* row, do anything.
- Not verifiable by `--capture`: the window is Chromium's own OS window. Tested through its X11
  window (title / class via `xprop`, a single-window grab by id for the content) and the engine log.
- Left: native Wayland untested (Chromium's Ozone-Wayland windows are the same code path as Chrome's
  DevTools, but no test window was opened on the user's screens); window placement is Chromium's
  (it remembers the last size/position); the frontend's own language/theme follow Chromium, not
  Axion's appearance.

## 3. Out of scope for v0.1
Extensions, sync, reader mode, Windows/macOS builds, touchscreen polish.

## 4. Roadmap

### Phase 0 — De-risking spikes (do first)
- [x] **CEF OSR spike**: open one page with CEF (`cef` crate / cef-rs), receive frames via
  `OnAcceleratedPaint` dmabuf, import into wgpu (Vulkan external memory), display under winit
  on Wayland. Measure latency + CPU. Fallback: `OnPaint` CPU upload.
  Result (2026-10-25, CEF 156 / Chromium 156, `crates/axion-spike-cef`): dma-buf import works on
  NVIDIA, ~0.7–1.2 ms per imported frame (copy + GPU wait). Importing across GPUs (CEF on NVIDIA,
  wgpu on the AMD iGPU) fails with a DRM-modifier error, so the spike falls back to software paint
  automatically.
  - [x] Pin Chromium's GPU process to wgpu's adapter (`--render-node-override`, done in Phase 1).
  - [ ] **RADV (AMD) GPU fault:** with both on the iGPU, importing the dma-buf in our Vulkan
    device triggers a GPUVM permission fault and context loss ("hard recovery"). Until found
    (modifier/plane layout in cef-rs' dmabuf import? sync?), GPU sharing is NVIDIA-only by
    default and other drivers use software paint (`AXION_OSR=gpu` forces it — don't on AMD).
  - [x] Avoid the GPU wait in the import: each shared buffer is imported once and cached
    (`cef/dmabuf.rs`); a frame's copy and its drawn check are only submitted, and resolved on
    a later pump (paint callback 1–3 ms → 0.1 ms; spikes of 15–90 ms gone). The copy itself
    stays (CEF's buffer returns to its pool when the callback ends).
  - [x] Frame pacing: Chromium's own begin-frame timer at the monitor's refresh rate
    (`tabs.max_frame_rate`); external begin frames from our loop halved pages' main-thread
    frame rate. Chromium's message loop is pumped when it asks
    (`OnScheduleMessagePumpWork` wakes winit) instead of a busy loop.
- [x] **Typed letters vanishing on GPU frames** (NVIDIA / Wayland default): Chromium reports a
    dma-buf frame once its drawing is submitted and NVIDIA attaches no fence (checked: the
    buffer's sync file is always signalled), so copying at once could read the pooled buffer's
    previous content (the letter just typed missing for a frame, or until the next caret-blink
    paint ~500 ms later). The newest frame is now held `tabs.gpu_frame_settle_ms` (6) before it is
    copied (`cef/settle.rs`). Measured with `AXION_TYPE_PROBE`: 2-9 vanished frames and 120-510 ms
    latency spikes per 24 characters before, 0 and <=40 ms after. A frame later than the settle
    time on a very busy GPU can still be stale: raise the key.
- [ ] **Input forwarding spike**: mouse, keyboard (incl. IME), scroll, touchpad gestures into CEF.
  *(mouse, wheel, keys and text implemented in the spike, not yet verified by hand; IME and
  touchpad gestures still open)*
- [x] **Glass playground**: standalone wgpu example with a static image backdrop — SDF shapes,
  merging, refraction, blur, specular. Get the look right *before* integration.
  Result (2026-10-25): 1440p frame = 7.6 ms on Radeon 610M (2 CU iGPU), 0.55 ms on RTX 3070 Ti.
- [ ] Decide: wayland pointer-gestures via winit or smithay-client-toolkit.

Exit criteria: a page renders at 60+ fps with one glass capsule refracting it. ✅ (spike, 2026-10-25)

### Phase 1 — Skeleton
- [x] Cargo workspace, CI (fmt, clippy, test) in `.github/workflows/ci.yml`.
- [x] `Engine` trait + CEF backend: create/close, navigate, back/forward, reload/stop, zoom,
  hidden/focus, input, title/URL/loading/progress/favicon-URL/cursor/fullscreen events, popup
  requests (blocked without a user gesture), clean shutdown, automatic software fallback.
- [x] Compositor: layer list (page images with UV/edge extension, glass, quads/text/icons),
  ping-pong targets so glass refracts everything below, design icon set via glyphon custom
  glyphs, async luminance probe, read-back for `--capture`.
- [x] `axion-ui`: retained tree, taffy layout, glyphon text, rounded hit-testing, hover/press,
  focus, single-line editing (selection, word motion, clipboard), glass depth/level layering.
- [x] `axion-config`: schema with design-token defaults, TOML load/save, live reload, keymap
  with Safari-style defaults and conflict detection.
- [x] `axion-app`: window with the design's bar (address field, back/forward, reload/stop,
  progress, tab pills, new tab), sidebar (tab list), wide/regular/narrow layouts, tabs,
  keyboard shortcuts, adaptive light/dark chrome over the page.
- Not verified by hand yet: interactive typing/clicking in the real window (logic is unit
  tested; screenshots come from `--capture`). IME and touchpad gestures still open.

### Phase 2 — Safari UI
- [x] Address field suggestions (history/bookmarks, search), real favicons (download + cache).
- [x] Toolbar and sidebar rebuilt after iPadOS 26 Safari (see decisions log).
- [x] Tab overview with snapshots and zoom transition.
- [x] Page menu (≡) with bookmark/zoom/copy link; Bookmarks/History/Reading List panels.
- [x] Private tabs (in-memory CEF context, dark chrome) with a lock screen; the lock uses the
  account password, one unlock per session (`[privacy]`).
- [x] Tab drag and drop: sidebar lists, compact tab capsule and separate tab bar, spring
  physics (lens follows the pointer, rows slide aside), between normal and private tabs
  (`tabs.drag_threshold`).
- [x] Tab bar widths: all tabs equally wide (`layout.tab_width`, shrinking equally to
  `layout.tab_min_width`, below which titles hide and tabs shrink on to icon-only squares of
  `layout.tab_icon_width`, then overflowing; `distribute_tab_widths`), springing to new widths; hairline separators in
  the separate tab bar (hidden beside the open and the hovered tab).
- [x] Tab context menu (right click on tab bar, compact segments, sidebar rows; glass menu
  flowing out of the tab, shortcuts from the keymap; sidebar rows are covered by their menu
  (`Placement::Over`), top-bar tabs get the menu dripping out *below* the tab (`BelowStart`, left edges
  aligned, shallow bridge `BRIDGE_INTO_JOIN` so an inactive tab's label isn't cut): `Over` in the bar
  plane merged with the capsule's own glass into a lumpy outline); pinned tabs (first, favicon only,
  `layout.pinned_tab_width`, kept in the session via the `session_pinned_tabs` meta key).
- [x] Bookmarks bar (`layout.bookmarks_bar` = always/home/never; overflow chevron menu,
  middle click opens in the background, right click: Open in New Tab / Delete). Top-level
  bookmarks only until bookmark folders exist.
- [x] Local accounts (`axion-accounts`): separate profiles, account button + menu in the
  sidebar, Add Account sheet, Settings › Account (name, colour, password, delete), switching
  by relaunching, migration of the pre-accounts profile.
- [ ] Switch accounts in-process (several CEF request contexts) instead of relaunching.
- [x] Settings sheet (General, Appearance, Liquid Glass, Keyboard, Privacy), changes live.
- [ ] Share sheet (Share currently copies the link), reading list entries, tab groups.
- [ ] Tab pills: open/close/reorder with physics.
- [x] Sidebar (overlay + docked), responsive breakpoints.

### Phase 3 — Liquid Glass integration
- [x] Glass renderer in the compositor; all chrome uses the glass material (thin/regular/thick).
- [x] Liquid motion in the real UI: page menu and suggestions drip out of the toolbar's glass
  and back (design's liquid spring), glass swells under a pressed button, floating sidebar
  slides on a spring, overview zoom; all honour Reduce Motion. A pressed button is glass: it
  becomes its own lens (swells up to 1.4×, clearer and lighter, follows the pointer on a rubber
  band, stretches) flowing out of its group's glass, its icon carried along centred in it.
- [x] Popovers are one liquid shape with their bar (`popover.rs`): page menu, account menu,
  Shields, downloads, suggestions and the password prompt drip out of their anchor and stay
  joined to the glass under it by a broad liquid bridge across their shared front (wider smooth
  union of its own, `glass.popover_bridge_fillet`; no tint of its own, so thin bar and dense
  popover tint blend across it), drawn in the bar's pass with thick-glass tint
  (per-shape tint strength, blended across the merge); text sits directly on the glass and
  follows the glass's visual frame (axion-ui), so both come and go together. Menus have
  captions, hairlines and side submenus (Zoom › − 100% +).
- [x] Adaptive legibility (light/dark chrome from the page's top colour); Reduce Transparency.
- [ ] Drag a tab pill: stretch, reorder, pull out into its own glass droplet; fling.
- [x] Performance mode for weak GPUs: flat mode (`glass.style = "flat"`) skips all glass passes
  (see the decisions log; `axion-ui` example `flat_bench`). *Open:* a frame-time overlay in the full app.

### Phase 4 — Shields
- [x] adblock-rust request interception via CEF `ResourceRequestHandler` (IO thread, behind
      the engine's `RequestPolicy` trait). Lists: EasyList, EasyPrivacy, uBlock filters +
      privacy, Peter Lowe; downloaded in the background, cached compiled in the profile,
      refreshed every 96 h. Standard spares first-party requests (unless `$important`).
- [x] Cosmetic filters: host-specific and generic non-class selectors injected as a style
      sheet at load start. *Open:* class/id generic rules (needs a DOM-scanning content
      script → `hidden_class_id_selectors`) and scriptlets (need uBO's resources).
- [x] Hoster tricks (tested on aniworld.to): a cross-site iframe may only open tabs to its
      own site, and may not send the whole tab to a third site (initiator check in the
      resource-request hook) — VOE's ad tab on the first player click is blocked.
- [x] Popup & redirect policy: popups need a click and must not go to a listed ad site
      (`$popup` rules are rewritten to `$document`, adblock-rust has no popup type);
      redirects without a click to listed sites are cancelled in `OnBeforeBrowse`;
      Aggressive also cancels every cross-site redirect without a click.
- [x] Tracking parameters (fbclid, gclid, … and `utm_*` on Aggressive) stripped from page loads.
- [x] Third-party cookies blocked per request (CEF cookie access filter). Chromium's
      `profile.cookie_controls_mode` pref is refused by CEF 156, so it isn't used.
      *Open:* `document.cookie` in third-party iframes; fingerprinting protections.
- [x] Shields popover (drips from the shield: counter, Off/Standard/Aggressive per site,
      stored in the profile), Settings › Shields, grey shield when off.

### Phase 5 — Tabs & memory
- [x] Lifecycle (`axion-tabs`): Active → Background → Frozen → Discarded. Freezing uses
      DevTools `Page.setWebLifecycleState`; discarding closes the page and keeps URL, title and
      a GPU copy of the last frame (overview snapshot); showing the tab resumes/reloads it.
      Defaults: freeze after 10 min, unload after 2 h (`[tabs]`, Settings › Tabs & Memory).
- [x] Tabs playing audible media are never frozen/unloaded. Detection: an injected script
      reports play/pause through a marked console message (stopgap until the 5b media bridge).
- [x] Memory budget: PSS of the whole process tree, measured on a background thread; over
      budget the longest-hidden tab is unloaded (one per check). Default: 40 % of RAM.
      *Open:* per-tab memory reporting (renderer processes don't map to tabs in CEF).
- [x] Session restore (`session_tabs` table): the active tab loads, the others come back
      unloaded. *Open:* back/forward history and thumbnails don't survive unloading/restart.

### Phase 5b — Media player & fullscreen
- [x] Media bridge (`axion-media/src/bridge.js`), injected into every frame at load start:
      finds media in the DOM, shadow roots (patched `attachShadow`) and detached `new Audio()`
      (patched `play`); iframes report to their parent via `postMessage`, the top frame to the
      browser as a marked console message; commands go back down the same way. Inline,
      pages keep their controls; a fullscreen `<video>`'s built-in controls are hidden.
      Commands run through DevTools
      `Runtime.evaluate` with a user gesture (play with sound, fullscreen need one).
- [x] Glass controls, fullscreen only (Safari's player), always dark glass
      (`Ui::set_plane_dark`): close, PiP, volume, ±10 s, play, progress, speed menu, captions.
      The inline replacement of native controls and site players was dropped (user decision):
      pages keep their own player until fullscreen. `media.fullscreen_player` turns Axion's
      player off; `media.trusted_player_sites` (YouTube, Twitch, Vimeo, Netflix…, editable in
      Settings › Media: × per site, field + Add) keep theirs also in fullscreen.
      Play/pause of a site player in fullscreen is a real click at the video's centre (hosters
      only load their stream on their own play click), retried up to 3× if the hoster ate it.
- [x] Own fullscreen: the element's real fullscreen; in a site's own fullscreen (YouTube…)
      the video "takes over" (everything else `visibility:hidden`) under our controls.
      Controls and pointer hide after 2.5 s; media keys (Space/K, ←/→, ↑/↓, M, F/Esc, C, <, >)
      only while fullscreen.
- [ ] **H.264/AAC**: the prebuilt CEF has no proprietary codecs (ffmpeg is inside
      libcef.so). `tools/build-cef.sh` builds CEF 156.0.14 with them (hours, ~100 GB);
      then point `CEF_PATH` at the result. Needed for most hosters (aniworld's VOE is x264).
      Checked 2026-10-26 (probe-media, which now also reports codec support): on aniworld every
      hoster serves H.264/AAC — VOE loads its `_h.mp4` and JW Player fails with 224003
      (unsupported format), `MediaSource.isTypeSupported('video/mp4; codecs="avc1.42E01E"')`
      and AAC are false, `canPlayType` is ""; Filemoon/Doodstream are x264/AAC too. Shields,
      popups, cookies and the bridge are not the cause (VOE's stream URL loads; its ad tab is
      blocked as intended). Vidmoly additionally shows "AdBlock detected" because its bait
      scripts (IAB `adblockDetector.js`, `adsbygoogle.js`) are blocked outright: needs
      adblock-rust redirect resources / scriptlets (open, see Phase 3).
- [x] Picture in Picture as its own OS window (user decision): a plain window of the window
      manager without a title bar, only the video in it (letterboxed in black, no corners,
      margins or transparency of ours); dragging the video moves it, its edges resize it
      (`drag_resize_window`), glass controls on hover. Always on top (X11 window level; KDE
      Wayland via a KWin script — untested on Wayland so far). The video keeps playing from
      its hidden tab (takeover, still rendering).
      Aspect lock (`media.pip_lock_aspect`, default on; was `pip_keep_aspect`): the window is
      always the video's aspect (no black bars), resizes are only linear: X11 gets the
      WM_NORMAL_HINTS aspect, everywhere the window is snapped while resizing (`snap_aspect`:
      dominant axis, every `pip_aspect_live_seconds`, never below `min_window`: `pip_min_width`
      wide and as tall as the controls need, growing with the aspect kept for 21:9 / 9:16).
      Wayland has no aspect lock (xdg-toplevel; KWin scripts have no aspect property): snapping
      only, real WM drags there untested.
- [x] Playback priority: playing tabs are never frozen/unloaded (bridge state); VA-API
      on by default (`chrome://gpu`: Video Decode hardware accelerated); fullscreen controls
      disappear entirely while hidden, so no glass is drawn over playing video.
      *Open:* raising renderer OS priority (no tab → process mapping in CEF).
- [x] MPRIS (`org.mpris.MediaPlayer2.axion.instance<pid>`, zbus): status, metadata from
      `navigator.mediaSession`, position, seek, volume, rate — media keys work through it.
      Now Playing card at the bottom of the sidebar: a compact player (artwork from the page's
      media session / poster / og:image, else the site icon; scrolling title; seekable progress
      bar; previous / play / next via the page's `previoustrack` / `nexttrack` handlers and
      MPRIS Next/Previous). Its glass is its own pass (never merges with the account button);
      the account menu lifts it out of the way. Private tabs stay off the desktop.
      *Open:* captions rendered by sites (not text tracks) are hidden by the fullscreen
      takeover (trusting the site in Settings › Media keeps them).

### Phase 5c — Axion Playlist
- [x] Store table, `[playlist]` config, `playlist_add` / `open_playlist` actions, sidebar
  *Playlist* row, page menu entries, Settings › Media › Playlist.
- [x] Bridge reports `src` / `poster` / the page's `og:image`; Add button + popover, fullscreen
  player button, thumbnails (poster, preview image, frame grab).
- [x] Downloads of plain files through the engine (`Engine::start_download`) with progress,
  streams marked "streaming only".
- [x] The page: list (rows, "…" menu, edit mode, search) + embedded player (progress, shuffle,
  repeat, speed, sleep timer), saved positions and watched state.
- [x] Video source only: source capture (iframes, `blob:` via network sniffing), the media view
  (`axion://media/`, hls.js), lazy source resolving, *Open page* message, Axion's fullscreen
  player and Picture in Picture directly on the media view.
- [ ] HLS/DASH download (needs a segment fetcher + remux, or ffmpeg), DASH playback (dash.js),
  several named lists (the list menu is there, with the one list), drag to reorder (edit mode
  has move up/down), page keyboard navigation (↑/↓ over rows), a Playlist button in the PiP
  window, importing/exporting lists.

### Phase 6 — Bookmarks & history
- [ ] SQLite schema + migrations, sidebar views, search, import.

### Phase 7 — v0.1 polish
- [ ] Downloads, find-in-page, zoom, context menus, permissions prompts (all glass).
- [x] Page context menu (`page_menu.rs`, `browser/page_context.rs`): off-screen CEF has no native
      menu, so `ContextMenuHandler::run_context_menu` reports `EngineEvent::ContextMenu` and
      cancels; the glass menu drips out of the pointer (`Placement::Over`, flips up/left near
      edges, widens for the search row). Link, image, video/audio, selection, editable field and
      plain page rows, dimmed when they don't apply; Copy Image through `Engine::fetch_image` +
      arboard; Save* through `start_download`. Keys `ui.page_context_menu_*`, `ui.copy_image_max_px`.
      Not done: Print, Loop, keyboard navigation in the menu.
- [x] Find on Page: glass bar dripping out of the address field (Ctrl+F, page menu), search as
      you type, "3 of 17", Enter / Shift+Enter / Ctrl+G, case toggle; closes on tab switch and
      new page. Files dragged in from the desktop (X11/XWayland) and in-page drag and drop.
      Pasting images and copied files: on Wayland Axion reads the clipboard (data-control) and
      hands it to the page as a `paste` event, since off-screen Chromium has no focused surface.
      *Open:* native Wayland file drops (winit 0.30 has no Wayland drag and drop; winit 0.31
      or our own `wl_data_device`); copying *from* pages on native Wayland doesn't reach the
      desktop clipboard yet (same cause); the find bar can cover the selected match.
- [ ] Packaging: AUR / Flatpak / AppImage.
- [ ] Crash handling, logging.
- [x] First-run onboarding (2026-10-26): five steps (Welcome + language, Appearance + accent, Tabs and
      sidebar, Glass or flat, Account) as tiles of glass with mini-window pictures, step dots, Back /
      Continue / Skip, directly on a veil of the theme's ground colour (`ui.onboarding_dim`; no panel).
      Choices apply live (config diff-save path); language switches the sheet at once, the rest
      relaunches at the end only if it changed. Shows once on a fresh install (`general.onboarding_done`;
      existing accounts/config/history are marked done; an unfinished tour returns); Settings > General
      "Show Welcome Tour", `--onboarding`. Keyboard: Tab/arrows, Enter, Space, Esc = Back. Glass options
      sit `merge_distance + 3` px apart (never bridged at rest). No bookmark importer exists, so none is
      offered. Capture aids `onboarding-step:N`, `onb-theme|lang|glass|tabs|sidebar|accent|name|password|next|back|tab:…`.

### Safari parity pass (2026-10-25, from the user's recordings — docs/reference/safari-ipados26.md)
- [x] Liquid Glass switch/slider knobs: lens while held, rubber band, frost back.
- [x] Player like Safari's: corner circles, volume capsule, centre ±10/play, progress capsule,
      speed menu; audio indicator in the address field (click mutes the tab).
- [x] Sliders in the players (2026-10-26): the fullscreen Playlist player swallowed presses on sliders
      (`hit_action` ignores non-button widgets) - now `Ui::covers`; thick progress bar
      (`media.progress_thickness` / `progress_hit_height`) with a lens while dragged; volume control
      on the Playlist page and in PiP (speaker = mute, slider opens beside it); one shared Playlist
      volume + mute (`playlist.volume` / `playlist.muted`) set by every control, applied to each video.
- [x] Locked private browsing view + unlock alert card.
- [x] Tab overview top bar [Private | N Tabs], +, search, blue done; light backdrop.
- [x] Grouped settings (cards, sections, notes), scrolling with clipping; Safari's tab options:
      separate/compact tab bar, colour in tab bar, open new tabs in background, close tabs after a day/week/month.
- [ ] Page menu items (translate, find on page, reader, request mobile site), address dropdown
      sections ("Google suggestions", "Bookmarks, History and Tabs").

## 5. Later (v0.2+)
Chrome extension subset, encrypted sync, reader mode, profiles, Servo backend,
Windows/macOS, touchscreen-first mode.

## 6. Risks
| Risk | Mitigation |
|---|---|
| CEF dmabuf OSR flaky on some Wayland drivers | CPU upload fallback; test on AMD/Intel/NVIDIA early |
| Glass too expensive on weak GPUs | Downsampled backdrop, cached blur, perf mode, Reduce Transparency |
| Chromium memory use | Aggressive freeze/discard, renderer process limit, budget UI |
| Breaking sites with Shields | Per-site toggle, Standard vs Aggressive levels |
| CEF binary size / update cadence | Pin CEF version, scripted updates |
| Stock CEF builds lack H.264/AAC (proprietary codecs) and Widevine DRM | Build CEF with `proprietary_codecs`/`ffmpeg_branding=Chrome`; DRM (Netflix etc.) is a separate open question |
| Sites with custom JS players (YouTube) fight our controls | Integrate with their player state where possible, per-site opt-out to native controls |

## 7. Decisions log
- 2026-10-26: **Interface language (English default, German).** `general.language` =
  `system` (default: `LC_ALL`/`LC_MESSAGES`/`LANGUAGE`/`LANG`, else English) | `en` | `de`;
  Settings > General > Language. Crate `axion-i18n` (depends only on `toml`): gettext-style, the
  **English source text is the key**: `tr("Text")` (returns `&'static str`; missing/empty
  translation falls back to English), `tr_fmt!("Delete {count} items", count = n)` (named
  placeholders, reorderable), `tr_n("{n} Tab", "{n} Tabs", n)` (plural, one when n == 1),
  `tr_mark("Text")` for const tables (translate with `tr(x)` where shown). The catalog
  `crates/axion-i18n/locales/de.toml` (`"English" = "Deutsch"`, empty = untranslated) is embedded
  and parsed once; the language is process-wide and applied when Axion starts, so switching in
  Settings takes effect **after a restart** (chrome nodes are retained; a live rebuild would touch
  every widget; `axion_i18n::generation()` exists for a later live switch). The start page gets the
  catalog through `/api/state` (`i18n` map of exactly the keys its HTML (`data-t`,
  `data-t-attrs`) and script (`t("…")`) use, plus `lang`). Not translated: logs, config keys,
  identifiers, URLs, shortcut chord names, brand names. `cargo test -p axion-i18n` scans all
  sources (regex-free scanner, `axion_i18n::scan`) and fails with the list of missing, unused or
  placeholder-mismatching keys; `cargo run -p axion-i18n --example extract` appends missing keys.
  Layout robustness: labels ellipsize instead of clipping; `Ui::set_label_lines` lets a label wrap (settings row labels 2 lines, notes 4, sheet notes 3); settings button rows wrap; capture aids `--open=settings-page:N,i18n-overflow` (use `--after=12 --lead=9000` so the steps run before the capture) log every text that still does not fit. Translator instructions and glossary: `crates/axion-i18n/locales/GLOSSARY.md`. Known gaps:
  library `thiserror` messages shown verbatim (vault/accounts), weekday/month names in history
  headings, PiP window title (matched by a KWin script).
- 2026-10-26: **Window icon on Wayland / `axion://` favicon.** winit 0.30 ignores window icons on
  Wayland (compositors take the icon from the `app_id` `axion` via `axion.desktop` + hicolor), so
  dev/release runs showed the generic icon. `tools/install-desktop.sh [--bin PATH] [--uninstall]`
  installs the entry (Exec= rewritten to the binary) and icons per user. `xdg-toplevel-icon-v1`
  (KWin 6.8 has it) is **not feasible** without forking winit: it needs the `xdg_toplevel` object,
  which winit/sctk own and never expose. PiP uses app_id `axion` too (winit's `with_name` sets the
  app id from `general`; `instance` is X11 only), so it needs no entry of its own. All `axion://`
  pages show the app icon (64 px PNG) as site icon (`Favicons::get`; no cache files, no fetching);
  the start page also serves it as `axion://home/icon.png` (`<link rel=icon>`).
- 2026-10-26: **App icon (updated).** Default is now the **user's own generated glass-compass icon**
  (glass squircle tile with a blue compass; made by the user with an image generator and chosen by
  them as Axion's icon: the artwork and its use are their choice and responsibility). Original kept
  untouched at `assets/icons/source/axion-user-icon.png` (1465x1073 RGBA with a white glow halo).
  Processing (ImageMagick): alpha channel thresholded at 90 % to find the tile, morphological
  open/close + 1 px blur as a clean soft-edged mask (drops the noisy glow, keeps the glass
  translucency and highlights), cropped to the tile, Lanczos-resized to 880 px, subtle drop shadow
  added, centred on a 1024x1024 transparent canvas (~7 % padding) -> `assets/icons/axion-user.png`.
  `hicolor/<n>x<n>/apps/axion.png` (16..512) are Lanczos downscales of it (16/24/32 px lightly
  unsharp-masked); `hicolor/scalable/apps/axion.svg` embeds the 512 px PNG as a base64 data: URI.
  `preview.png` is the contact sheet (256 px + small sizes on light/dark, 4x zoom of 16-32 px).
  The window icon still embeds the 256 px PNG (`app_icon.rs`); the app id is `axion`.
  The earlier SVG variants stay as alternatives: **v2** "lens A" (`axion-v2.svg`, former default,
  most legible at 16-32 px; `axion-small.svg` is its shadow-free small variant), **v1** compass and
  **v3** orb. **To swap back:** copy the chosen `axion-vN.svg` over `hicolor/scalable/apps/axion.svg`
  and re-export the PNGs (`rsvg-convert -w N`); no code change.
- 2026-10-25: The page starts **below** the floating bar and **ends there**, like in any
  browser: nothing of the page shows or refracts behind the bar (user decision; an earlier
  "edge extension" that stretched the page's top row behind the bar was removed).
- 2026-10-25: The page area has **rounded corners** (`layout.page_corner_radius`, default 10 —
  just enough to not be sharp, per user) and a small gap to the window edges (`layout.page_margin`, 8).
  The window itself keeps the system title bar and square corners (a frameless rounded window
  with own window buttons was tried and dropped — not wanted).
- 2026-10-25: Default search engine DuckDuckGo (configurable `general.search_url`).

- 2026-10-25: The chrome follows **iPadOS 26 Safari** (user's reference screenshots): three
  toolbar glass groups (navigation · one capsule with all tabs as equal segments, the active
  one a light pill with close/Shields/address/reload/page menu · share/new tab/overview), and a
  full-height flush sidebar (tab list, Private, Saved: Bookmarks, Reading List, History).
  Differences on purpose: back/forward buttons stay (desktop), no iCloud.
- 2026-10-25: The window area around the page takes the **page's top colour** (solid, measured
  on the GPU; `appearance.tint_from_page`), like Safari. The page itself still never shows
  behind the bar.
- 2026-10-26: **Supersedes the page-colour default above.** The glass's ground is neutral and
  independent of the website (`appearance.tint_mode = "neutral"`, the default): Apple's system
  greys, never pure white/black. Light: ground `#F2F2F7`, raised surfaces `#FAFAFC`, glass tint
  `#F9F9FB`. Dark: ground `#1C1C1E`, surfaces and glass tint `#2C2C2E`, separators `#3A3A3C`
  (tokens in `axion-config` `schema.rs`, also used by flat mode and Reduce Transparency).
  Chrome light/dark follows `appearance.theme` only: `adapt_to_page` and `tint_from_page`
  default to off and stay opt-in switches (Settings > Appearance / Tabs); turning "Colour in
  tab bar" on switches the mode to `extreme`, with an Extreme/Dominant choice. Only defaults
  changed: a config file that sets these keys keeps its values (the writer only saves
  changes from the defaults, but a file written by an older build may hold `true` pinned).

- 2026-10-25: Shields levels per site are stored by host (without `www.`) in the profile's
  `site_settings` table; a level equal to the default is not stored, so changing the default
  in Settings reaches every site without its own choice. Changing a level reloads the page.

- 2026-10-25: Unloaded ("discarded") tabs keep their `ViewId`; the engine closes the CEF
  browser but keeps the view's state, so the rest of the browser never sees tab identities
  change.

- 2026-10-25: Page ↔ browser messages use console messages from an injected script rather
  than a renderer-side CEF app; pages could forge them, but only to fool their own controls.

- 2026-10-25: Axion's player only takes over in fullscreen; inline, every page keeps its own
  player. Trusted sites keep theirs also in fullscreen; non-programmers edit that list in
  Settings › Media (`media.trusted_player_sites` in config.toml). Old keys `own_controls` /
  `replace_site_players` are accepted and ignored; `own_fullscreen_controls` became
  `fullscreen_player`.

- 2026-10-25: **Local accounts.** Shared `<config base>/accounts.toml` lists the accounts and
  the last one used; each account's data lives in `<profile base>/accounts/<id>/` and its
  settings in `<config base>/accounts/<id>/` (bases: `~/.config/axion`,
  `~/.local/share/axion/profile`, or `AXION_CONFIG_DIR`/`AXION_PROFILE_DIR`). The first start
  moves the old profile and `config.toml`/`keymap.toml` into a first account (named after
  `$USER`); if a move fails the account keeps the old directories in place (paths stored in
  accounts.toml), and while the old profile is in use (Chromium's `SingletonLock`) the move
  waits for the next start. The account password is the vault's master password and the
  private tabs' lock — unlocking or locking either does both; the old separate private
  password is dropped. Switching relaunches Axion (`--account=ID`, same size/position) after
  Chromium shut down, since CEF allows one root cache path per process. Deleted accounts'
  directories are removed at the next start.
- **Device-bound vault** (done): the vault key is HKDF-SHA256 over Argon2id(password) and a
  per-device secret, so a copied vault or profile opens nowhere else (`OtherDevice`, distinct
  from a wrong password). The secret is sealed by the TPM 2.0 (`/dev/tpmrm0`, ECC P-256 SRK +
  sealed object, blob in `~/.local/share/axion/device.tpm`), else a 0600 `device.key` there.
  Password-only vaults upgrade on their next unlock (one transaction). Settings › Passwords
  exports a portable copy encrypted with the password alone and restores such exports (into
  the unlocked vault, or as the vault of an account without password). A key file made
  without TPM access moves into the TPM at the first start where it is usable (same secret,
  `device.tpm` verified by unsealing before `device.key` is zeroed and removed; retried each
  start on failure).
- 2026-10-25: The bars no longer hide while scrolling (the "mini pill" was removed; old
  `layout.hide_bar_on_scroll` keys are accepted and ignored). **F11** (`toggle_fullscreen`)
  is whole-page fullscreen instead: the window goes fullscreen, bars slide up and fade, the
  sidebar slides away and the page card grows to the whole screen (springs); Esc or F11
  leaves. The page's own element fullscreen (video) stays separate. `Chrome::is_fullscreen`
  tells transient UI (find bar…) that nothing else may cover the page.

- 2026-10-26: **Sidebar slide, page follows live** (supersedes "the page stands still while the
  sidebar slides"). With `layout.sidebar_slide_resize = "live"` (default; `"final"` = the old
  single resize + page standing still) the shown page is resized while the docked sidebar slides
  (`resize_page_live`): a new size every `layout.sidebar_slide_resize_step` px, sent *ahead* by
  the measured Chromium latency (`page_lag`, first guess `layout.sidebar_slide_lead`) times the
  rectangle's speed, clamped to the settled width, so frames arrive about when the rectangle
  gets there. The latest frame is drawn stretched over the rectangle (a few percent), so no
  edge is uncovered, nothing is cropped, centred and left-aligned pages and Home all follow
  and nothing pops at the end. The software paint path now also drops the "old page scaled with
  a black bar" frames Chromium emits right after a resize (`has_black_bar`; the shared-buffer
  path already did), which showed as a 5-15 px vertical jump per frame in live mode.

- 2026-10-26: **Axion's own pages** are served by the browser through a CEF custom scheme
  (`axion://`, standard + secure + display-isolated + fetch-enabled, not CORS-enabled), behind
  the engine's `InternalPages` trait. Page → browser: `POST /api/message`, accepted only from
  a view's main frame showing that `axion://` origin (checked in the scheme handler on the IO
  thread) → `EngineEvent::InternalMessage`. Browser → page: the page pulls `/api/…` (same
  check); the browser only nudges it with a data-free script. Web pages can't link to, frame,
  navigate to or fetch `axion://` (verified with a test page).
- 2026-10-26: The window tint / chrome light-dark comes from **one colour per page** (its
  `theme-color` for the scheme, else the dominant colour of a 32×32 GPU grid of the frame,
  evened to a calm light/dark tone), recomputed on load, theme-colour change or scheme switch
  and animated on a spring — never from scrolling (`appearance.page_color_delay_ms`).
- 2026-10-26: Favicons are cached **per colour scheme** (`<host>.png`, `<host>.dark.png`, with
  the source URL in `.url`); the engine reports icon links with their `media` queries
  (`EngineEvent::PageMetaChanged`), and a scheme switch re-resolves open pages' icons.
- 2026-10-26 (user feedback, small UI decisions):
  - Chromium runs with `--password-store=basic`: it must never open the desktop keyring
    (KWallet prompts); Axion's own vault holds passwords.
  - The window tint keeps the page's hue but always takes the current mode's tone (light in
    light mode, dark in dark mode), so switching the mode switches it.
  - Toolbar: a home button next to back/forward (`layout.home_button`); empty or the old
    DuckDuckGo homepage become `axion://home/`; on the start page the address shows its
    placeholder in grey.
  - Separate tab bar: equal-width tabs (`layout.tab_width` 260), centred, the strip only as
    wide as its tabs; the bookmarks bar spans the full width and hides while empty.
  - Popovers drain back into their anchor in two stages: plunge with an underdamped spring and
    recoil out once (`motion.drain`), then flow into the anchor (`motion.drain_absorb`).
    Settings opened from the page menu grow out of its Settings row and drain into the address
    field. The find bar floats below the tab/bookmarks bars and is joined to the address
    field only while it flows (`Popover::detached`); Escape closes it from anywhere.
  - Pressing a button inside a glass group moves the whole group as one lens (icons carried);
    standalone buttons are their own lens.
  - Settings has a search field with aliases (English + German) in `settings/search.rs`;
    every new setting needs an entry there.
  - Text fields: click places the caret, drag selects, double click selects a word (URL
    punctuation ends words) and dragging then extends by words, triple click selects all.
  - In small windows the floating sidebar lies above the toolbar.
- 2026-10-26: Default keybinds redesigned (see 2.6): tabs on `Alt`/F keys, browser and system
  things on `Ctrl`. New bound actions: `pin_tab`, `bookmark_page`, `show_bookmarks`,
  `show_history`, `open_passwords`. Explicit `keymap.toml` chords win over other actions'
  new defaults, so old files never collide.

- 2026-10-26: **Axion Playlist** (user request; reference screenshot of an iPad playlist
  rebuilt in Axion's glass style). Decisions where the request was silent:
  - The page is a plane over the page card (everything else of the chrome except the sidebar
    button hides, reusing the locked-private layout); any tab switch, navigation or tab-related
    shortcut closes it first (`keeps_playlist` lists what leaves it open).
  - One list, "Playlist" (`playlist.list_name`; users cannot create lists, so the header has no
    list menu; profiles still saying "Play Later" are migrated at start-up). The table keeps its
    `list` column for a future multi-list UI.
  - Playback lives in a view of its own (not a tab, no history/session). Superseded
    2026-10-26 (see below): Axion's fullscreen and PiP players now work on that view directly.
  - Thumbnails come from the page's own pictures first; frame grabs read the engine's frame
    textures back on the CPU (a few ms, only when adding or the first seconds of a downloaded
    file) instead of asking the page to draw a canvas (cross-origin videos would taint it).
  - Downloads use CEF's `StartDownload` with a one-shot target folder per address
    (`DownloadInfo::original_url` matches them) and are also listed in Downloads; they never
    auto-open. Shields is asked explicitly before (the browser-initiated download bypasses the
    resource hooks).
  - HLS/DASH/blob are streaming-only for downloads (no silent failure: reason shown); they
    play in the media view (HLS) or show the *Open page* message (DASH, uncapturable blobs).
  - Keys: `playlist_add` Ctrl+Shift+D, `open_playlist` Ctrl+Shift+P (both free).

- 2026-10-26: **Playlist plays the video source only** (user requirement). Decisions:
  - The media view is the engine's `set_media_view(view, MediaViewOptions)`: headers for its
    requests, the original page as Shields' page, and a lock that refuses any navigation away
    from its own page (`axion://media/` or the local file). `axion://media/` is served by
    `media_view.rs` with its own CSP (media / XHR from http(s), scripts only from itself) and
    `/api/source?s=<token>` answers only that page.
  - Only `Referer` is replaced: Chromium's network stack refuses a request whose `Origin` was
    changed (`ERR_INVALID_ARGUMENT`). Hosters that check `Origin` (not `Referer`) or send no
    `Access-Control-Allow-Origin` for HLS segments won't play; the message offers *Open page*.
  - Cookies: the media view uses the normal profile, so the host's cookies apply as in a tab.
  - A media view may reach the local network / this computer (a home server, a local test
    page): the engine grants Chromium's local-network permission prompt for media views only.
  - hls.js is Apache-2.0 (not MIT); its licence file is kept next to the vendored script.
  - Playing from the Playlist pauses the page resting under it.
  - `playlist.forward_clicks` is obsolete (kept so old config files load). New keys:
    `resolve_timeout_seconds`, `resolve_settle_seconds`, `resolve_click_seconds`, `sniff_limit`.

- 2026-10-26 (user request, "Mode completely without glass"): **flat mode**, `glass.style = "liquid" |
  "flat"` (Settings > Appearance > Glass; live-switchable, also by editing the config file).
  - One switch, `Theme::flat()` / `Glass::is_flat()`, read in a few central places instead of `if flat`
    in widgets: `axion-ui` paints every `Background::Glass(level)` as a solid card (surface colour,
    1px hairline, soft shadow: `Theme::flat_paint(level, nested)`; thin cards 0.94 opaque, regular
    0.97, thick and anything with `glass_density` thick opaque; glass on glass is a shadowless inset in
    the ground colour; a lens-level background is a faint label-colour highlight; tinted glass is the
    colour at 0.96). It emits plain `Content` quads (new `Quad::ring` border and `card_shadow`), so
    **no `Layer::Glass` exists in flat mode: no backdrop copies, no blur chain**. Reduce Transparency's
    opaque lens colour is not needed (hover/pressed are label-colour tints as there).
  - Physics: `Glass::squash()` / `merge_distance()` / `popover_bridge_fillet()` are 0 and
    `Glass::damping()` raises every spring to critical damping (`Spring::critical()`, `Spring::CRISP`);
    no press swell or lens (`press_target` is `None`), knobs and the segmented pill stay plain,
    tab drag follows the pointer directly. `Drip` in flat mode has no edge animation: the card is at
    its target and fades in/out with a 0.96 -> 1 scale on a critically damped spring (`Drip::fade`);
    popover bridges and the sheet's hand-off proxy are skipped; tooltips use the same fade.
  - Layout (`Config::effective_layout`): the chrome is one conventional toolbar. A `toolbar_band`
    surface (surface colour, bottom hairline) spans the window beside a docked sidebar down to the page
    top; nav/action groups are `FlatRole::Bare`; the address field is a `Field` (separate tab bar) or a
    `Raised` pill on the tab capsule (compact); margins shrink to <= 8 px, the page is square and
    full-bleed (`page_margin` 0), a docked sidebar is a square full-height panel, an overlay sidebar a
    card with shadow. Locked private browsing and the Playlist page hide the band as before (the
    sidebar button then stands on a card). Everything else (popovers anchored to buttons, find bar,
    narrow layout, bookmarks bar) uses the unchanged layout code.
  - Rule for new UI: it must work in both modes (use `Background::Glass`/`FlatRole`, springs from
    config via `drip::spring`/`Glass::damping`, `Config::effective_layout` for chrome geometry).
  - Performance mode (answers the Phase 3 item): `cargo run -p axion-ui --example flat_bench --release`
    renders a browser-like chrome (sidebar rows, toolbar, popover) over a page at 2560x1440. RTX 3070 Ti
    1.80 ms liquid (6 glass passes) -> 0.23 ms flat; Radeon 610M (`AXION_GPU=low`) 29.3 ms liquid
    (34 fps) -> 1.6 ms flat.

## 8. Open questions
- App name final? ("Axion" for now.)
- Default search engine?
- Licensing (MIT/Apache-2.0 vs MPL-2.0 vs GPL)?
- Distribution channel priority (AUR first for CachyOS?).
