# Changelog

## Unreleased

### New
- **Network & VPN**: route an account, private tabs or single sites through an upstream proxy
  (SOCKS5, HTTP, HTTPS; with login), with a **kill switch** (a down connection blocks instead of
  falling back to direct, with an explanation page), leak protection (WebRTC, DNS, background
  services), a connection chip with a per-site popover in the address field, and the
  `toggle_network` shortcut. Passwords and keys live in the password vault. WireGuard (import a
  `.conf`) and Tor are prepared and arrive with their backends. See docs/network.md.

## 0.2.0

### New
- **Chrome extensions** (Manifest V3) with the **Chrome Web Store**: install from the store's own
  "Add to Chrome" button (our confirmation dialog lists the permissions), from a folder or a
  `.crx`. Popups open as a glass overlay under the extensions button. CRX3 signature and id are
  verified; installed store extensions update automatically. Tampermonkey runs (`chrome.userScripts`).
  Extensions that use `webRequest` / `declarativeNetRequest` have those APIs switched off in Axion's
  copy (they stall page loads in this engine); Shields does the blocking.
- **Tab groups** (named, coloured; sidebar, tab bar and overview), **tab search**
  (`Ctrl+Shift+A`), per-tab **sound indicator and mute**, middle-click closes tabs, an animated tab
  close, and dragging a tab out of the window to paste its link into other apps (with a drag image).
- **Command palette** (`Ctrl+K`), **per-site zoom**, "Share…" and search for selected text.
- **Reader view** (page menu) and **local page translation** (Bergamot, 23 languages, no server;
  models are downloaded on demand).
- **Print / Save as PDF** (`Ctrl+P`; Passwords moved to `Ctrl+Alt+P`) and a **screenshot tool**
  (`Ctrl+Shift+S`: selection, visible area, full page).
- **Screen sharing picker**, DevTools on native Wayland, window size/position and panel state restored
  after relaunch.
- **New logo** and **Lucide** icons throughout; the start page uses the chrome's neutral ground colour.

### Media
- Playlist: videos load through a proxy (CORS, cookies, referer, range requests), Twitch live streams,
  loading ring, error state with retry, audio-only items show their cover, mute no longer unloads the
  video, speed/sleep menus open as clean cards above their buttons.
- Picture-in-Picture resizes smoothly from any edge or corner at the video's aspect ratio and
  remembers its size; fullscreen/PiP controls are pure glass.
- The mini player keeps its card while paused, drops it when the page is left, and ignores muted
  autoplay previews; Pause and Stop work reliably (also from MPRIS).
- Live streams show a full timeline; Discord and call streams are ignored.

### Liquid Glass and motion
- Springs and the drag rubber band follow Apple's published parametrisation (`docs/physics.md`).
  Dragged glass stretches with resistance; icons and nested glass deform with it exactly.
- Glass buttons are pure glass: no drop shadow, no hover/press plate, grey fills replaced.
- Switches are faster; toasts avoid open popovers.

### Stability
- GPU errors are logged instead of panicking, panics are written to `last-panic.txt`, crashed
  renderers reload, shutdown is bounded by a watchdog, and the database no longer blocks the UI.
- Fixed a crash when a tab drag started after the tab list was rebuilt.

### Known limits
- Extension installs and toggles need a relaunch; ad-blocking extensions do not work.
- Hardware video decoding on native Wayland with NVIDIA can show a black picture on some systems:
  try `AXION_OSR=software` or turn hardware decoding off in Settings › Media.
- The scalable app icon is a raster wrapper (no vector master yet).
