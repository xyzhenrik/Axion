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

`motion.snappy/smooth/bouncy/interactive` default to these presets. `motion.liquid`, `drain`,
`drain_neck`, `drain_absorb`, `fling`, `tab_close`, `switch`, `CRISP` and the drip/merge
springs are **Reconstructed/Axion-specific** (no Apple spring is published for Liquid Glass
merging, popover drains, UISwitch knob travel); they are still expressed in response/damping.
`motion.switch` (0.18 / 0.80) was left unchanged for that reason.

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
