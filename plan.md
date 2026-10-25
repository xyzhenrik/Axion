# Axion Browser — Plan

Status: **planning** · Target: v0.1 (daily-drivable on Linux)

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
- [ ] **CEF OSR spike**: open one page with CEF (`cef` crate / cef-rs), receive frames via
  `OnAcceleratedPaint` dmabuf, import into wgpu (Vulkan external memory), display under winit
  on Wayland. Measure latency + CPU. Fallback: `OnPaint` CPU upload.
- [ ] **Input forwarding spike**: mouse, keyboard (incl. IME), scroll, touchpad gestures into CEF.
- [x] **Glass playground**: standalone wgpu example with a static image backdrop — SDF shapes,
  merging, refraction, blur, specular. Get the look right *before* integration.
  Result (2026-10-25): 1440p frame = 7.6 ms on Radeon 610M (2 CU iGPU), 0.55 ms on RTX 3070 Ti.
- [ ] Decide: wayland pointer-gestures via winit or smithay-client-toolkit.

Exit criteria: a page renders at 60+ fps with one glass capsule refracting it.

### Phase 1 — Skeleton
- [ ] Cargo workspace with all crates, CI (fmt, clippy, test). *(workspace started: axion-glass, axion-physics)*
- [ ] `Engine` trait + CEF backend: create/close browser, navigate, back/forward, reload, title/favicon/URL events.
- [ ] Compositor frame graph: page layer → backdrop capture → UI/glass passes → present.
- [ ] `axion-ui` basics: tree, taffy layout, glyphon text, hit-testing, focus.
- [ ] `axion-config` with TOML + live reload; keymap.

### Phase 2 — Safari UI
- [ ] Compact top bar + address field (suggestions from history/bookmarks).
- [ ] Tab pills: open/close/reorder with physics.
- [ ] Sidebar (overlay + docked), responsive breakpoints.
- [ ] Tab overview with snapshots and zoom transition.
- [ ] Settings sheet.

### Phase 3 — Liquid Glass integration
- [ ] Port playground shaders into the compositor; all UI uses glass material.
- [ ] Merging between UI elements, drag stretch, fling, spring-back.
- [ ] Adaptive tint/legibility; Reduce Transparency/Motion; perf mode.
- [ ] Perf budget: 120 fps at 1440p on integrated GPU.

### Phase 4 — Shields
- [ ] adblock-rust request interception via CEF `ResourceRequestHandler`.
- [ ] Cosmetic filters + scriptlets.
- [ ] Popup & redirect policy (`OnBeforePopup`, `OnBeforeBrowse` with user-gesture info).
- [ ] Tracking param stripping, cookie policy, fingerprinting protections.
- [ ] Shields popover + per-site settings; list updates.

### Phase 5 — Tabs & memory
- [ ] Lifecycle state machine + freeze/discard + restore.
- [ ] Memory budget enforcement and per-tab memory reporting.
- [ ] Session restore.

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

## 7. Open questions
- App name final? ("Axion" for now.)
- Default search engine?
- Licensing (MIT/Apache-2.0 vs MPL-2.0 vs GPL)?
- Distribution channel priority (AUR first for CachyOS?).
