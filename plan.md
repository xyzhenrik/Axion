# Axion Browser — Plan

Status: **Phase 5 done** (2026-10-25; Phase 3 leftovers open) · Target: v0.1 (daily-drivable on Linux)

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
- **Start page**: favourites grid, frequently visited, Shields stats; customizable background.

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
- The "Private" section can be **locked with a password** (like Safari's locked private
  browsing); unlocking asks for it after the browser was in the background or restarted.

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
- Keyboard: Safari-like defaults mapped to Ctrl on Linux (Ctrl+T, Ctrl+L, Ctrl+Shift+\ for
  overview, Ctrl+Shift+L for sidebar, Ctrl+1…9, …). Fully remappable in `keymap.toml`
  and the settings UI; conflict detection.
- Touchpad: two-finger swipe back/forward with physics preview, pinch for tab overview,
  smooth scrolling forwarded to the page.
- Mouse: drag tabs/glass elements, middle-click behaviours, mouse back/forward buttons.

### 2.7 Media player & fullscreen (priority 1)
- **Own media controls** for every HTML5 `<video>`/`<audio>`: native page controls are hidden
  (injected script/CSS) and replaced by our glass control bar — scrubber with thumbnail
  preview, play/pause, ±10 s, volume, speed, captions, PiP, AirPlay-style cast slot.
  Driven via a small injected bridge (`HTMLMediaElement` API ↔ IPC). Per-site opt-out.
- **Own fullscreen**: we handle CEF fullscreen requests ourselves — the page/video fills the
  window, our glass controls float above, auto-hide after inactivity, Esc/double-click exits.
- **Picture-in-Picture**: our own floating glass window, draggable with physics, snaps to corners.
- **Now Playing** in the sidebar + MPRIS integration (media keys, desktop widgets).
- **Playback has priority 1**:
  - tabs playing media are never frozen/discarded and are exempt from the memory budget;
  - their renderer gets raised OS priority, background tabs are throttled harder while media plays;
  - hardware video decoding (VA-API) enabled by default;
  - the compositor never drops video frames for UI effects — glass degrades first
    (frame pacing locked to video cadence while in fullscreen).
- Keybinds: Space, ←/→, ↑/↓, F, M, C, `<`/`>` — remappable like everything else.

### 2.8 Customization
- Settings UI (glass sheet) + `~/.config/axion/config.toml`, live-reloaded.
- Themes: accent colour, light/dark/auto, glass parameters, density, corner radius.

## 3. Out of scope for v0.1
Extensions, sync, reader mode, profiles, Windows/macOS builds, touchscreen polish.

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
  - [ ] Avoid the per-frame copy + GPU wait in the import (keep CEF's texture alive until our
    composite is submitted).
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
- [x] Private tabs (in-memory CEF context, dark chrome) with Argon2 password lock after N s in
  the background (`[privacy]`), lock screen, settings to set/remove the password.
- [x] Settings sheet (General, Appearance, Liquid Glass, Keyboard, Privacy), changes live.
- [ ] Share sheet (Share currently copies the link), reading list entries, tab groups.
- [ ] Tab pills: open/close/reorder with physics.
- [x] Sidebar (overlay + docked), responsive breakpoints.

### Phase 3 — Liquid Glass integration
- [x] Glass renderer in the compositor; all chrome uses the glass material (thin/regular/thick).
- [x] Liquid motion in the real UI: page menu and suggestions drip out of the toolbar's glass
  and back (design's liquid spring), glass swells under a pressed button, floating sidebar
  slides on a spring, overview zoom; all honour Reduce Motion.
- [x] Adaptive legibility (light/dark chrome from the page's top colour); Reduce Transparency.
- [ ] Drag a tab pill: stretch, reorder, pull out into its own glass droplet; fling.
- [ ] Performance mode for weak GPUs, and measuring the full app (not just the playground)
  against the 120 fps / iGPU budget.

### Phase 4 — Shields
- [x] adblock-rust request interception via CEF `ResourceRequestHandler` (IO thread, behind
      the engine's `RequestPolicy` trait). Lists: EasyList, EasyPrivacy, uBlock filters +
      privacy, Peter Lowe; downloaded in the background, cached compiled in the profile,
      refreshed every 96 h. Standard spares first-party requests (unless `$important`).
- [x] Cosmetic filters: host-specific and generic non-class selectors injected as a style
      sheet at load start. *Open:* class/id generic rules (needs a DOM-scanning content
      script → `hidden_class_id_selectors`) and scriptlets (need uBO's resources).
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
- [ ] Media bridge script: detect media elements (incl. shadow DOM / iframes), hide native controls, IPC.
- [ ] Glass control bar, scrubber, captions, speed, volume.
- [ ] Custom fullscreen + auto-hide; PiP window.
- [ ] Playback priority: lifecycle exemption, process priority, VA-API, frame pacing.
- [ ] MPRIS + media keys; Now Playing in sidebar.

### Phase 6 — Bookmarks & history
- [ ] SQLite schema + migrations, sidebar views, search, import.

### Phase 7 — v0.1 polish
- [ ] Downloads, find-in-page, zoom, context menus, permissions prompts (all glass).
- [ ] Packaging: AUR / Flatpak / AppImage.
- [ ] Crash handling, logging, first-run onboarding.

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

- 2026-10-25: Shields levels per site are stored by host (without `www.`) in the profile's
  `site_settings` table; a level equal to the default is not stored, so changing the default
  in Settings reaches every site without its own choice. Changing a level reloads the page.

- 2026-10-25: Unloaded ("discarded") tabs keep their `ViewId`; the engine closes the CEF
  browser but keeps the view's state, so the rest of the browser never sees tab identities
  change.

## 8. Open questions
- App name final? ("Axion" for now.)
- Default search engine?
- Licensing (MIT/Apache-2.0 vs MPL-2.0 vs GPL)?
- Distribution channel priority (AUR first for CachyOS?).
