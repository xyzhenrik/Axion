# Changelog

## Unreleased

### Fixed
- **Updater (source checkout)**: clicking "Update from Git" in the confirmation sheet did nothing
  visible: the sheet's buttons shared their action ids (9200-9202) with the tab overview's header,
  so the click opened a new tab behind the sheet and the sheet never closed. The update only ran
  when the app was restarted by hand.

- **Sign-in with Google and other OAuth/SSO flows** returned nowhere: `window.open` popups became
  unrelated tabs without `window.opener`. Popups are now real Chromium popups with an opener
  (`postMessage`, `window.close()` work; `tabs.popups_keep_opener`), and sign-in provider iframes
  keep their cookies (`shields.sign_in_hosts`).
- **Copy**: copy buttons on websites (`navigator.clipboard`, `execCommand`, `ClipboardItem`), Ctrl+C,
  Ctrl+X and the context-menu Copy now reach the desktop clipboard on Wayland (`clipboard.page_copy`).
- **Second launch showed the Chromium UI** (and crashed the first instance): launching Axion again,
  or opening a link from another app, now opens it as a tab in the running window of that profile
  (`general.single_instance`).
- **Downloads "Open folder"** did nothing: the password eye buttons shared their action ids with the
  sidebar rows. Three more latent id collisions fixed, and a table test guards all id blocks.
- **Video previews** (hover tiles, autoplay cards) no longer count as playback for the Playlist add
  button, the page menu or the pause check.
- **Translate offer** shows once per page load, a dismissal is final (`translation.offer_once_per_page`).
- **Onboarding** with an existing account no longer asks for an 8-character password.
- **Glass of a freshly opened popover, menu or sheet** can no longer leap on its first frame
  (`motion.open_step_max_ms`, `motion.open_guard_frames`).

### New
- **Network**: **Use X for this session** in the address-bar chip routes all tabs through a
  connection until switched off (`network.session_route_persist`). Connection secrets are encrypted
  with the device key, so connecting needs no vault password; old vault secrets migrate on unlock.
- **Tab bar** hides while exactly one plain tab is open (`layout.hide_single_tab_bar`); right-click on
  the address field opens the tab menu; closing actions are red.
- **History and Downloads** are glass sheets like Settings and Passwords (`general.list_sheets`);
  downloads show the site icon.
- **Context menus everywhere** through one registry (sidebar rows, toolbar buttons, overview cards,
  start page tiles, settings rows, extensions, network chip, media player); Menu key / Shift+F10.
- **Default browser**: Settings > General, an onboarding step and an optional startup hint.
- **Languages**: Spanish, French, Turkish, Japanese, Portuguese (Brazil), Italian, Russian, Simplified
  Chinese, Korean, Dutch, Polish (machine-translated, review welcome), a picker in Settings, and all
  58 Mozilla page-translation languages as translation targets.
- **New app icon** (glass globe with orbit ring).
- **Native start page** (`home.native`, on by default; set it to false for the HTML page): the start page rendered by Axion with
  real glass. Not yet on par with the HTML page (no video wallpaper, no IME, no zoom/find).
- **Updater**: the sheet closes at once when you confirm; progress (step, newest output line, bar,
  **Cancel**), the result ("Updated to X", **Restart Axion** / **Later**) and failures (reason, last
  output, **Try again**) are on Settings > Info & Updates. The version line says "restart to finish
  the update" instead of showing the stale build. Only changes to tracked files block a Git update
  (untracked files no longer do), the dirty files are listed, and a checkout ahead of or diverged
  from the remote gets a clear message.
- **Updater (tarball / portable installs)**: downloads and checks the release tarball, unpacks it
  next to the install folder and swaps it in at the restart (old folder kept as `.old-<version>`).
  See docs/features/updates.md.
- **Desktop notification** when an update is found while the window is in the background
  (`updates.desktop_notification`, Settings > Info & Updates > Notify me about updates).

## 0.3.0

### New
- **Network & VPN**: route an account, private tabs or single sites through a connection, with a
  **kill switch** (a down connection blocks instead of falling back to direct, with an explanation
  page), leak protection (WebRTC, DNS, background services), a connection chip with a per-site
  popover in the address field, and the `toggle_network` shortcut. Passwords and keys live in the
  password vault. See docs/network.md.
  - **Proxy** (SOCKS5, HTTP, HTTPS; with login).
  - **WireGuard**: import a `.conf`; a fully userspace client (boringtun + smoltcp: no root, no TUN
    device) with DNS inside the tunnel, AllowedIPs enforcement and a failure reason in the
    connection list. TCP only (no UDP/QUIC through the tunnel).
  - **Tor** (built-in Arti client, not Tor Browser): per-site circuits, `.onion` support, plain
    bridges, bootstrap progress in the chip, and a **circuit display** (route button in the address
    field) listing the guard, middle and exit relays of the site with country and IP, plus "New Tor
    circuit for this site" and "New Tor identity".
- **Link preview**: hovering a link shows where it leads (host emphasised, middle-ellipsis, moves
  out of the way of the pointer and of toasts).
- **Import site icons** from Chromium-family browsers, Firefox and HTML bookmark exports.
- **Fullscreen player**: the title, channel and site on top (`Title - Channel - Site`), YouTube
  livestreams are recognised as live, and the video fills the window when YouTube's chat is open.
- **New app logo** and refreshed README screenshots.

### Fixed
- Video previews (YouTube grids, hover previews) no longer count as playback in the mini player.
- Muting a Playlist video no longer unloads it; a live stream's timer starts at 0:00.
- The fullscreen player hides the pointer together with its controls.
- Glass pills that sit close together (for example in the network popover) no longer merge into one blob.
- Tor shutdown and restart no longer block the interface; background work runs at low priority.
- Reduce Transparency no longer covers the label under a slider or segmented knob.

### Notes
- `rusqlite` went back to 0.40 (required by Arti). The `priority-queue` crate is used under MPL-2.0
  (see docs/third-party.md).

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
