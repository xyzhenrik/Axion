# CLAUDE.md — Axion Browser

Guidance for Claude (and humans) working in this repository.

## What this is

Axion is a desktop web browser written in **Rust** whose UI is modelled closely on
**iPadOS 26+ Safari** — sidebar, compact tab bar, tab overview grid — rendered with
**real Liquid Glass**: translucent, refractive, physically animated material that
bends and tints the web page behind it. Privacy/blocking is on par with Brave Shields.
Everything is customizable, including memory budgets and tab-freezing policy.

The roadmap lives in [plan.md](plan.md). Read it before starting a new milestone.
The visual design (Claude Design canvas, exported) lives in [docs/design/](docs/design/): every
board is an HTML file; `Main.dc.html` ("Foundations") holds the tokens that `axion-config`
defaults come from. Match new UI to these boards.

## Communication

- All code, comments, commits and docs: **English**. UI strings: **English is the source language** (written as `tr("…")` literals); German is a translation in `crates/axion-i18n/locales/de.toml` (see plan.md §7, language).
- Clarifying questions to the user: **German**.
- Ask before guessing on anything that changes product behaviour or architecture.

## Key decisions (settled — don't re-litigate without the user)

| Area | Decision |
|---|---|
| Language | Rust (stable, 2024 edition). C/C++ only behind FFI where unavoidable. |
| Web engine | Chromium via **CEF**, using **off-screen rendering (OSR)**. Engine sits behind the `Engine` trait so another backend (e.g. Servo) can be added later. |
| Rendering | **wgpu** (Vulkan on Linux). We own the whole frame: page textures + UI + glass are composited by us. |
| Windowing/input | `winit`, plus Wayland pointer-gestures directly where winit lacks them. |
| Platform | **Linux first** (Wayland primary, X11 supported). Keep code portable: no Linux-only APIs outside `platform/` modules. |
| Input | Mouse + keyboard primary, full touchpad gesture support, fully remappable keybinds. Touch screen is a nice-to-have. |
| Blocking | Brave's `adblock-rust` for network + cosmetic filtering; own logic for popups/redirects. |
| Storage | SQLite (`rusqlite`) for bookmarks/history; TOML for user config. |
| v0.1 scope | Tabs, sidebar, tab overview, Liquid Glass, Shields, tab freezing/discarding, bookmarks & history, **own media player + fullscreen**. |
| Media | Our own glass media controls, fullscreen and PiP replace the page's. **Playback is priority 1**: media tabs are never frozen, video frames are never dropped for UI effects. |

## Architecture (Cargo workspace)

```
crates/
  axion-app/         binary `axion`: window, input routing, tabs, chrome (bar + sidebar), wiring
  axion-engine/      Engine trait + CEF backend (OSR, dma-buf frames, input, popups, GPU pinning)
  axion-compositor/  frame = layers (page images, glass, content: quads/text/icons), luminance probe
  axion-glass/       Liquid Glass: SDF shapes, merging, refraction, blur, specular, shaders (WGSL)
  axion-physics/     springs, drag/fling, rubber-banding, gesture → motion mapping
  axion-ui/          retained widget tree, taffy layout, glyphon text, hit-testing, focus, editing
  axion-config/      settings schema (design-token defaults), TOML load/save, live reload, keymap
  axion-shields/     adblock-rust engine + list cache, popup/redirect/cookie policy, tracking params
                     (the engine asks it through `axion_engine::RequestPolicy`, glue in app/shields.rs)
  axion-store/       SQLite: history, bookmarks, meta, per-site settings, session
  axion-accounts/    local accounts: shared accounts.toml, per-account directories, migration
                     (switching relaunches; UI in app/accounts.rs, chrome.rs, settings.rs)
  axion-tabs/        tab lifecycle planner (freeze/discard by idle time + memory budget), PSS monitor
  axion-i18n/        UI translations: `tr("English")` lookup, catalog `locales/de.toml`, source scanner
  axion-media/       media bridge script (bridge.js) + its protocol; controls live in app/player.rs,
                     MPRIS in app/mpris.rs
```

Dependency direction: `app` → everything; `ui` → `compositor`, `glass`, `config`;
`compositor` → `glass`; `accounts` → `config`; `engine`, `shields`, `tabs`, `store`, `config`,
`accounts` must **not** depend on UI/rendering crates (`engine` uses wgpu only to hand out
frame textures).

### Rendering model

1. CEF renders each visible tab off-screen; frames arrive as dmabuf (`OnAcceleratedPaint`)
   and are imported into wgpu as textures (CPU `OnPaint` upload is the fallback path).
   Chromium's GPU process is pinned to wgpu's GPU via `--render-node-override`. GPU import is
   on by default **only on NVIDIA**; on RADV it currently causes a GPU fault (see plan.md).
2. The compositor draws a `Scene`: an ordered list of layers. Every glass layer samples the
   **already-rendered layers beneath it** (backdrop texture), so glass genuinely refracts
   page content and other UI; glass inside glass (sidebar cards) is just a later layer.
   `axion-ui` groups glass by nesting depth and level (thin/regular/thick) into layers.
3. The page starts below the bar and beside a docked sidebar, and ends there: the page never
   shows behind the bar (user decision). The page area is a rounded card
   (`layout.page_corner_radius`, `layout.page_margin`); the OS window keeps its normal frame. Floating panels (overlay sidebar) do refract it.
   By default the chrome is neutral Apple grey and ignores the page (`appearance.tint_mode = neutral`).
   Opt-in: a GPU luminance probe of the page's top edge flips the chrome light/dark
   (`appearance.adapt_to_page`) and tints the window (`tint_from_page`).
4. Glass shapes are described as **signed distance fields**. Neighbouring shapes are merged
   with a smooth-min, which gives the "two drops connect" effect for free.
5. The refraction offset comes from the SDF's gradient (surface normal) and a lens height
   profile; add chromatic dispersion, a downsampled blur, edge specular/rim light, and a
   subtle tint adapted to backdrop luminance.

### Liquid Glass quality bar

Glass is the product's identity. It must:
- refract (displace) content near edges, not just blur it;
- merge smoothly when shapes come close and split again with surface-tension-like motion;
- stretch/squash under drag and spring back (physics in `axion-physics`, never hard-coded easing);
- adapt contrast for legibility on any backdrop (light, dark, busy);
- hold **120 fps on an integrated GPU** at 1440p. Profile before adding passes.

**Every glass button is full glass (user rule, always).** No grey/opaque fill, plate or halo
behind a glass button or any glass control: only the refractive glass layer (tint goes through
the glass tint parameters, never through a background fill). Check every new button on light
and dark backdrops with `--capture` before calling it done.

Provide a "Reduce Transparency" / "Reduce Motion" setting that degrades gracefully.

**Flat mode** (`glass.style = "flat"`, the performance mode): the same UI without glass: solid
cards, one toolbar surface, no refraction/blur/merging/drips/stretch/bounce, and no `Layer::Glass`
at all (the compositor skips every glass pass). New UI must work in both modes: use
`Background::Glass` (+ `FlatRole` where it is not a card), take springs from config through
`drip::spring` / `Glass::damping`, chrome geometry from `Config::effective_layout`, and never read
`glass.squash` / `merge_distance` / `popover_bridge_fillet` directly (use the methods).

## Conventions

- Verify visual changes with `--capture` (never screenshot the user's desktop) and perf
  changes with `--bench` on **both** GPUs (`AXION_GPU=low` is the Radeon 610M budget case).
- `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings` must pass.
- Errors: `thiserror` in library crates, `anyhow` only in `axion-app`.
- Logging: `tracing`. No `println!` outside examples.
- No `unwrap()`/`expect()` in non-test code unless the invariant is commented.
- CEF's request hooks (`cef/network.rs`) run on Chromium's IO thread: they may only hold
  `Send + Sync` state (`Arc<Network>`), never the `Rc`-based `ViewContext`.
- `unsafe` only in `axion-engine` (CEF FFI) and GPU interop; every block gets a `// SAFETY:` comment.
- Shaders are WGSL files in each crate's `shaders/` directory, embedded with `include_str!`
  (hot reload in debug builds is still to do).
- Every user-facing behaviour gets a config key in `axion-config` with a sensible default.
  Hard-coded magic numbers for timing, sizes or limits are a bug.
- **Every glass button is pure refractive glass**: no grey/opaque plate, no `Background::Fill`/`Soft`
  behind it. A glass widget with an action gets the `glass.button_opacity` tint automatically
  (`glass_tint_opacity` in `axion-ui`); text-bearing panels keep their level's tint, primary buttons use
  `TintedGlass`. Never add a fill to a glass button to make it "readable" — raise the tint through that
  config key or the glass tint parameters instead.
- Keybinds go through the keymap (`axion-config::keymap`), never matched directly in UI code.
- Tests: unit tests next to the code; shields filter tests use fixture lists in `tests/fixtures/`.

## Commands

```bash
cargo run -p axion-app --release -- [url]                          # run the browser
cargo run -p axion-app --release -- [url] --capture=out.png        # screenshot after 8 s, then quit
cargo test --workspace                                             # all tests
# First build downloads CEF (~1 GB) into third_party/cef (gitignored).
cargo run -p axion-glass --example playground --release            # glass playground, no CEF needed
cargo run -p axion-glass --example playground --release -- --demo  # scripted drag (merge/stretch)
cargo run -p axion-glass --example playground --release -- --bench # GPU time per 1440p frame
cargo run -p axion-glass --example playground --release -- --capture=out.png  # offscreen screenshot
AXION_GPU=low ...            # force the integrated GPU (also high, or part of an adapter's name; Settings › Hardware has the same choice)
AXION_OSR=software|gpu ...   # force page frame sharing off/on (default: on for NVIDIA only; also `hardware.page_frames`)
AXION_CONFIG_DIR / AXION_PROFILE_DIR   # use throwaway settings/profile bases (do this for test runs)
AXION_DEVICE_KEY_DIR                   # throwaway vault device key (default ~/.local/share/axion; set it too)
cargo test -p axion-vault -- --ignored real_tpm --nocapture   # seal/unseal with the real TPM (needs tss group)
tools/install-desktop.sh [--bin PATH] [--uninstall]   # per-user axion.desktop + icons: needed for the window icon on Wayland
axion --account=ID           # open that account (else the last one used; ids in accounts.toml)
AXION_MONITOR=HDMI-A-1 ...   # open on that monitor (X11/XWayland only: run with `env -u WAYLAND_DISPLAY`)
AXION_CHROMIUM_SWITCHES=autoplay-policy=no-user-gesture-required ...   # extra Chromium switches (comma-separated)
axion url1 url2 --overview --capture=out.png   # several tabs; screenshot of the tab overview
axion url --after=20 --capture=out.png        # capture after 20 s instead of 8
AXION_TYPE_PROBE=page:X,Y,W,H,INTERVAL_MS,START_S,TEXT axion url --capture=… --after=12   # typing latency per character into a page field (rect of an empty, caret-less field; logs `typing probe`; run on native Wayland, the GPU frame path is not used under XWayland)
AXION_TYPE_PROBE=native:INTERVAL_MS,START_S,TEXT axion --open=add-account --lead=8000 …     # same for a focused chrome field
AXION_FRAME_SETTLE_MS=N ...   # override tabs.gpu_frame_settle_ms (0 = copy GPU frames at once)
```

## Things to watch

- CEF OSR + dmabuf on Wayland is the biggest technical risk — see Phase 0 in plan.md.
  **Never force `AXION_OSR=gpu` on the AMD iGPU** while investigating: it has caused a GPU
  context loss (hard recovery) on the user's machine.
- The password vault is device-bound (`axion-vault`: Argon2id(password) + a device key, TPM-sealed
  where `/dev/tpmrm0` is accessible, else `device.key`, both outside the profile; a key file
  moves into the TPM once it becomes usable). Building
  without libtss2: `--no-default-features` (drops the `tpm` feature). Never touch the user's
  real `~/.local/share/axion/device.tpm|device.key`: losing it makes the vault unreadable.
- Test runs of the browser use a throwaway `AXION_PROFILE_DIR`/`AXION_CONFIG_DIR`/`AXION_DEVICE_KEY_DIR` in the
  scratchpad, never the user's real profile.
- Memory: Chromium renderers are heavy. Respect the user's memory budget in `axion-tabs`;
  freezing uses the DevTools protocol (`Page.setWebLifecycleState`), discarding closes the
  browser and keeps a snapshot + navigation state for restore.
- Never weaken a Shields default to "fix" a site; add a per-site exception mechanism instead.
- Don't copy Apple assets (SF Symbols, wallpapers, fonts). Recreate the *feel* with our own assets.
