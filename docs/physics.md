# Physics: Apple constants and where they come from

Axion's motion is meant to feel like iPadOS/iOS 26. Every constant below is marked
**Documented** (stated in Apple documentation or WWDC material, or in source of WebKit/UIKit
analyses), **Derived** (follows mathematically from documented values) or **Reconstructed**
(our measurement/estimate; Apple does not publish it). Code: `crates/axion-physics`.

## Springs

| Item | Value | Status |
|---|---|---|
| Parametrisation | `stiffness = (2π/response)²`, `damping = 4π·ζ/response`, mass 1 (`spring(response:dampingFraction:)`; CASpringAnimation takes mass/stiffness/damping directly) | Documented (WWDC23 "Animate with springs", 10158; SwiftUI `Spring` docs). Matches `Spring::stiffness/damping`, unit-tested. |
| `ζ = 1` critically damped, `< 1` bounces | | Documented |
| `Spring(duration:bounce:)` → `response = duration`, `ζ = 1 - bounce` (bounce ≥ 0), `1/(1+bounce)` (bounce < 0) | | Derived/Reconstructed: the exact mapping is the commonly quoted one, Apple's page does not print the formula. |
| `.smooth` bounce 0, `.snappy` 0.15, `.bouncy` 0.3, default duration 0.5 s | `Spring::SMOOTH/SNAPPY/BOUNCY` = (0.5, 1.0/0.85/0.7) | Documented (WWDC23 10158, SwiftUI docs) |
| `interactiveSpring` | response 0.15, dampingFraction 0.86 | Documented (SwiftUI `interactiveSpring(response:dampingFraction:blendDuration:)` defaults) |
| Integrator | exact analytic solution per frame (`Spring::propagator`), velocity carried when the target changes | Ours; tested against the textbook closed form at many times (`integrator_matches_the_analytic_solution`) |
| Velocity hand-off on release | the spring continues with the release velocity (`Animated` keeps `velocity`; the switch/slider knob sets it from the tracked pointer speed) | Documented behaviour (WWDC18 "Designing Fluid Interfaces", WWDC23 10158) |

`motion.snappy/smooth/bouncy/interactive` default to these presets. `motion.liquid` (press swell), `liquid_open`,
`liquid_close`, `liquid_small`, `liquid_small_close`, `fling`, `tab_close`, `switch`, `CRISP` and the drip/merge
springs are **Reconstructed/Axion-specific** (no Apple spring is published for Liquid Glass
merging, popover drains, UISwitch knob travel); they are still expressed in response/damping.
`motion.switch` (0.18 / 0.80) was left unchanged for that reason.

## Liquid surfaces (sheets, popovers, menus)

A sheet, popover or menu is one drop of glass: it grows out of its origin, stays joined to it
through the popover bridge (surface tension) until the body has left, and settles in a single
gentle approach. Code: `crates/axion-app/src/drip.rs` (every edge of the shape is a spring).
All four presets are **Reconstructed** and live in config (`motion.*`, see docs/settings.md):

| Preset | Used by | response | damping | overshoot cap | Analytic overshoot |
|---|---|---|---|---|---|
| `liquid_open` | sheets opening (Settings, Passwords, …) | 0.30 s | 0.78 | 3 % | 2.0 % |
| `liquid_close` | sheets draining back | 0.20 s | 1.0 | 0 | 0 |
| `liquid_small` | popovers and menus opening | 0.23 s | 0.80 | 3 % | 1.5 % |
| `liquid_small_close` | popovers and menus draining back | 0.15 s | 1.0 | 0 | 0 |

- Closing is critically damped and quicker than opening; the neck (the edge facing the origin)
  sinks into the origin's glass on a spring `motion.liquid_neck_lead` times as slow (0.7), the
  body follows, and the shape is gone as soon as it fits inside the origin: no recoil, no
  second drop.
- Hard clamp: each edge may pass its target by at most the preset's `overshoot` share of the
  distance it still has to travel, and never by more than `motion.max_overshoot_px` (8 px), also
  after a retarget or an interrupted open. A zero cap means the edge stops at its target.
- Content follows the glass (visual-frame transform) and shows once the glass has grown to
  `motion.content_reveal_from` (0.8) of its size, fully from `content_reveal_to` (0.96): text is
  drawn at 0.8-1.02 of its size (it used to be 0.55-1.08).
- Reduce Motion and flat mode have no drop: the surface sits at its target and fades with a
  `motion.fade_scale_from` (0.96) to 1.0 scale on a critically damped `motion.reduce_response`
  (0.16 s) spring; nothing is bridged to the origin.
- The old `motion.drain*` keys (an underdamped drain with a recoil and an absorbed drop) are
  ignored.

Measured with the unit simulation `drip::motion_tests::motion_table` (240 Hz, run with
`cargo test -p axion-app --release --bin axion motion_table -- --ignored --nocapture`; the
scenarios are a 760x560 sheet from the address field, a 320x260 popover from a 34 px button and
a 220x300 context menu from the pointer). Peak = how far an edge passes its target; settle = last
time any edge is more than 1 % of its travel from the target; recoil = how far an edge turns
back on its way into the origin; closed = until the shape is gone.

| Surface | | Peak overshoot | Settle to 1 % | Close recoil | Closed after |
|---|---|---|---|---|---|
| Sheet | before | 50.8 px (8.4 %) | 0.50 s | 56.9 px | 0.83 s |
| | after | 8.0 px (2.0 %, clamped) | 0.31 s | 0 | 0.17 s |
| Popover | before | 23.4 px (8.4 %) | 0.50 s | 21.9 px | 0.82 s |
| | after | 4.2 px (1.5 %) | 0.23 s | 0 | 0.11 s |
| Context menu | before | 24.7 px (8.4 %) | 0.50 s | 31.9 px | 0.78 s |
| | after | 4.5 px (1.5 %) | 0.23 s | 0 | 0.17 s |
| Flat / Reduce Motion (fade) | before | 0 | 0.5 s (flat; Reduce Motion still travelled) | 0 | 0.61 s flat, 0.43 s reduce |
| | after | 0 | 0.17-0.32 s | 0 | 0.19-0.25 s |

Other springs, by their damping (overshoot of a step = `exp(-πζ/√(1-ζ²))`): `snappy` 0.85 → 0.6 %,
`interactive` 0.86 → 0.5 %, `fling` 0.85 → 0.6 %, `tab_close` 0.95 → 0.01 %, `switch` 0.80 → 1.5 %
(all unchanged and small), `bouncy` 0.70 → 4.6 % (glass springing back after a pull: unchanged,
the drag lens is not excessive), `liquid` 0.62 → 8.4 % (only the press swell of a button, a scale
of a few percent: unchanged).

## Rubber band

`f(x, d, c) = (1 - 1/(x·c/d + 1))·d`, `c = 0.55`, `x` the overscroll distance, `d` the view's
dimension along the axis. **Documented/analysed** UIScrollView behaviour (Ole Begemann, "Inertia,
Bouncing, and Rubber-Banding with UIKit Dynamics"; Lucas Holko 2014, equivalent closed form
`x·d·c/(d+c·x)`; WebKit `WKScrollView` `_rubberBandOffsetForOffset`). Not a published API: it is
the well-established reverse-engineered result. Checks: `f(d) = 0.3548·d`, `f(2d) = 0.5238·d`,
slope `c` at the boundary (unit tests).

Use in Axion:
- Dragged glass stretch: `drag_stretch = max · f(distance, d, c) / d`, `d` = max(the glass's
  largest side, `glass.drag_range_min` = 120 px), so the curve stays nearly linear for a long pull.
  `glass.drag_stretch_max` (now 5 %) is the asymptote. **Reconstructed** choice (Apple's glass
  stretch magnitude is not documented). Content (icons, labels, nested glass, any depth) undergoes
  the glass's own transform: `p' = C + offset + swell·R·diag(1+e, 1/(1+e))·Rᵀ·(p - C)` about the
  glass centre `C` (positions exact; sizes by the matrix diagonal, exact for axis pulls; text only
  scales uniformly, by the horizontal factor).
- Knob over-pull: `f` with `d = 0.18` of the travel (**Reconstructed** limit).
- Config: `glass.rubber_band_c` (default 0.55, advanced). `glass.drag_resistance` (px scale of
  the old exponential curve) is deprecated and ignored.

## Fling projection

`UIScrollView.DecelerationRate.normal = 0.998`, `.fast = 0.99` per millisecond (**Documented**).
Rest point `project(v, rate) = v/1000 · rate/(1-rate)` (WWDC18 "Designing Fluid Interfaces",
**Documented**). Used for switch/segmented release: the side is decided by the position
projected with the fast rate (`Knob::projected`). Tab-drag release still picks the slot under the
tab (not projected; unchanged).

## Not documented, kept from before (Reconstructed)

Press swell (`PRESS_SCALE`, `LENS_PRESS_SCALE`), pull limit, squash while moving
(`glass.squash`), tab lift scale, sheet/popover spring choices. Apple publishes none of these
numbers; they are now driven by the preset springs above but the magnitudes are ours.

## Sources

- WWDC23 "Animate with springs" (10158): https://developer.apple.com/videos/play/wwdc2023/10158/
- SwiftUI `Spring`: https://developer.apple.com/documentation/swiftui/spring
- WWDC18 "Designing Fluid Interfaces" (803): https://developer.apple.com/videos/play/wwdc2018/803/
- `UIScrollView.DecelerationRate`: https://developer.apple.com/documentation/uikit/uiscrollview/decelerationrate
- Holko, "UIScrollView's Inertia, Bouncing and Rubber-Banding with UIKit Dynamics": https://www.holko.pl/2014/07/06/inertia-bouncing-rubber-banding-uikit-dynamics/
- Begemann, "Inertia, Bouncing, and Rubber-Banding": https://oleb.net/blog/2014/05/scrollview-inertia-bouncing-rubberbanding/
