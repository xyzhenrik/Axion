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

- All code, comments, commits, docs and UI strings: **English**.
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
planned:
  axion-shields/     adblock-rust integration, popup/redirect policy, fingerprinting protections
  axion-tabs/        tab lifecycle (active/background/frozen/discarded), groups, memory budget
  axion-media/       media bridge (injected JS ↔ IPC), controls state, fullscreen/PiP, MPRIS
  axion-store/       SQLite: history, bookmarks, session restore
```

Dependency direction: `app` → everything; `ui` → `compositor`, `glass`, `config`;
`compositor` → `glass`; `engine`, `shields`, `tabs`, `store`, `config` must **not** depend on
UI/rendering crates (`engine` uses wgpu only to hand out frame textures).

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
   A GPU luminance probe of the page's top edge flips the chrome light/dark
   (`appearance.adapt_to_page`).
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

Provide a "Reduce Transparency" / "Reduce Motion" setting that degrades gracefully.

## Conventions

- Verify visual changes with `--capture` (never screenshot the user's desktop) and perf
  changes with `--bench` on **both** GPUs (`AXION_GPU=low` is the Radeon 610M budget case).
- `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings` must pass.
- Errors: `thiserror` in library crates, `anyhow` only in `axion-app`.
- Logging: `tracing`. No `println!` outside examples.
- No `unwrap()`/`expect()` in non-test code unless the invariant is commented.
- `unsafe` only in `axion-engine` (CEF FFI) and GPU interop; every block gets a `// SAFETY:` comment.
- Shaders are WGSL files in each crate's `shaders/` directory, embedded with `include_str!`
  (hot reload in debug builds is still to do).
- Every user-facing behaviour gets a config key in `axion-config` with a sensible default.
  Hard-coded magic numbers for timing, sizes or limits are a bug.
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
AXION_GPU=low ...            # force the integrated GPU
AXION_OSR=software|gpu ...   # force page frame sharing off/on (default: on for NVIDIA only)
AXION_CONFIG_DIR / AXION_PROFILE_DIR   # use throwaway settings/profile (do this for test runs)
```

## Things to watch

- CEF OSR + dmabuf on Wayland is the biggest technical risk — see Phase 0 in plan.md.
  **Never force `AXION_OSR=gpu` on the AMD iGPU** while investigating: it has caused a GPU
  context loss (hard recovery) on the user's machine.
- Test runs of the browser use a throwaway `AXION_PROFILE_DIR`/`AXION_CONFIG_DIR` in the
  scratchpad, never the user's real profile.
- Memory: Chromium renderers are heavy. Respect the user's memory budget in `axion-tabs`;
  freezing uses the DevTools protocol (`Page.setWebLifecycleState`), discarding closes the
  browser and keeps a snapshot + navigation state for restore.
- Never weaken a Shields default to "fix" a site; add a per-site exception mechanism instead.
- Don't copy Apple assets (SF Symbols, wallpapers, fonts). Recreate the *feel* with our own assets.
