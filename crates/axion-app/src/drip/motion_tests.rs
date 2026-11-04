//! Measured motion of the drip springs (see docs/physics.md).

use super::*;

/// Measured motion of one drip run: per edge, how far it passes its target (px, share of its
/// travel), how far it turns back after its extreme on the way in, and when it is at rest.
#[derive(Debug, Default)]
struct Run {
    peak_px: f32,
    peak_pct: f32,
    reversal_px: f32,
    settle_s: f32,
    done_s: f32,
    /// Largest the frame ever is beyond the final target rect (opening), px.
    out_of_target_px: f32,
}

const DT: f32 = 1.0 / 240.0;

fn run_open(mut drip: Drip, config: &Config, anchor: Frame, start: Frame, target: Frame) -> (Run, Drip) {
    drip.open_from(config, anchor, start, target);
    let from = edges_of(drip.frame());
    let to = edges_of(target);
    let mut run = Run::default();
    let mut extreme = from;
    let mut t = 0.0;
    for _ in 0..(3.0 / DT) as usize {
        let phase = drip.step(DT);
        t += DT;
        let now = edges_of(drip.frame());
        for i in 0..4 {
            let dir = (to[i] - from[i]).signum();
            let travel = (to[i] - from[i]).abs().max(1.0);
            let over = (now[i] - to[i]) * dir;
            run.peak_px = run.peak_px.max(over);
            run.peak_pct = run.peak_pct.max(over / travel * 100.0);
            if (extreme[i] - now[i]) * dir < 0.0 {
                extreme[i] = now[i];
            } else {
                run.reversal_px = run.reversal_px.max((extreme[i] - now[i]) * dir);
            }
            if (now[i] - to[i]).abs() > 0.01 * travel {
                run.settle_s = t;
            }
        }
        let f = drip.frame();
        let out = (target.origin.x - f.origin.x)
            .max(target.origin.y - f.origin.y)
            .max(f.origin.x + f.size.x - target.origin.x - target.size.x)
            .max(f.origin.y + f.size.y - target.origin.y - target.size.y);
        run.out_of_target_px = run.out_of_target_px.max(out);
        if phase == Phase::Open {
            run.done_s = t;
            break;
        }
    }
    (run, drip)
}

/// Closing: how far an edge turns back (recoils) after it started draining, and when it is gone.
fn run_close(mut drip: Drip, config: &Config) -> Run {
    drip.close(config);
    let from = edges_of(drip.frame());
    let mut run = Run::default();
    let mut extreme = from;
    let mut t = 0.0;
    for _ in 0..(3.0 / DT) as usize {
        let phase = drip.step(DT);
        t += DT;
        let now = edges_of(drip.frame());
        for i in 0..4 {
            let moved = now[i] - from[i];
            let sign = if moved.abs() > 1.0 { moved.signum() } else { 0.0 };
            if sign != 0.0 {
                if (extreme[i] - now[i]) * sign < 0.0 {
                    extreme[i] = now[i];
                } else {
                    run.reversal_px = run.reversal_px.max((extreme[i] - now[i]) * sign);
                }
            }
        }
        if phase == Phase::Closed {
            run.done_s = t;
            break;
        }
    }
    run
}

/// (name, is a sheet, anchor, start, target)
fn scenarios() -> Vec<(&'static str, bool, Frame, Frame, Frame)> {
    let sliver = |a: Frame| Frame::new(a.origin.x, a.center().y, a.size.x, 0.0);
    let a1 = Frame::new(560.0, 14.0, 300.0, 36.0); // the address field (Settings origin)
    let t1 = Frame::new(220.0, 80.0, 760.0, 560.0);
    let a2 = Frame::new(1180.0, 14.0, 34.0, 34.0); // a toolbar button
    let t2 = Frame::new(900.0, 58.0, 320.0, 260.0);
    let a3 = Frame::new(500.0, 300.0, 4.0, 4.0); // a context menu from the pointer
    let t3 = Frame::new(500.0, 300.0, 220.0, 300.0);
    vec![
        ("sheet (Settings/History)", true, a1, sliver(a1), t1),
        ("popover (Shields)", false, a2, a2, t2),
        ("context menu", false, a3, a3, t3),
    ]
}

#[test]
#[ignore = "prints the before/after table of docs/physics.md"]
fn motion_table() {
    let mut config = Config::default();
    for (mode, flat, reduce) in [("liquid", false, false), ("flat", true, false), ("reduce", false, true)] {
        config.glass.style =
            if flat { axion_config::GlassStyle::Flat } else { axion_config::GlassStyle::Liquid };
        config.glass.reduce_motion = reduce;
        for (name, sheet, anchor, start, target) in scenarios() {
            let (open, drip) = run_open(new_drip(sheet), &config, anchor, start, target);
            let close = run_close(drip, &config);
            println!(
                "{mode:7} {name:26} open: peak {:5.1}px {:5.1}%  back {:5.1}px  settle1% {:.3}s  open {:.3}s  beyond-target {:5.1}px | close: recoil {:5.1}px  closed {:.3}s",
                open.peak_px,
                open.peak_pct,
                open.reversal_px,
                open.settle_s,
                open.done_s,
                open.out_of_target_px,
                close.reversal_px,
                close.done_s
            );
        }
    }
}

fn new_drip(sheet: bool) -> Drip {
    if sheet { Drip::sheet() } else { Drip::new() }
}

/// A unit step response of a preset: (peak overshoot as a share of the step, seconds until it
/// stays within 1 % of the target).
fn step_response(spec: LiquidSpec) -> (f32, f32) {
    let mut x = Animated::new(0.0f32, liquid_spring(&Config::default(), spec));
    x.target = 1.0;
    let (mut peak, mut settle) = (0.0f32, 0.0);
    for i in 1..=(3.0 / DT) as usize {
        x.step(DT);
        peak = peak.max(x.value - 1.0);
        if (x.value - 1.0).abs() > 0.01 {
            settle = i as f32 * DT;
        }
    }
    (peak, settle)
}

#[test]
fn every_preset_overshoots_at_most_its_cap_and_settles_in_time() {
    let m = Config::default().motion;
    // (name, preset, settle bound in seconds)
    let presets = [
        ("liquid_open", m.liquid_open, 0.40),
        ("liquid_close", m.liquid_close, 0.25),
        ("liquid_small", m.liquid_small, 0.28),
        ("liquid_small_close", m.liquid_small_close, 0.20),
    ];
    for (name, spec, bound) in presets {
        let (peak, settle) = step_response(spec);
        assert!(peak <= spec.overshoot + 1e-3, "{name}: overshoot {peak} over the cap {}", spec.overshoot);
        assert!(spec.overshoot <= 0.03 + 1e-6, "{name}: cap {} over 3 %", spec.overshoot);
        assert!(settle <= bound, "{name}: settles in {settle}s, bound {bound}s");
    }
    // Closing never overshoots, and is quicker than opening.
    assert_eq!(step_response(m.liquid_close).0, 0.0);
    assert_eq!(step_response(m.liquid_small_close).0, 0.0);
    assert!(step_response(m.liquid_close).1 < step_response(m.liquid_open).1);
    assert!(step_response(m.liquid_small_close).1 < step_response(m.liquid_small).1);
}

#[test]
fn simulated_surfaces_stay_within_the_overshoot_bounds() {
    let config = Config::default();
    let cap_px = config.motion.max_overshoot_px;
    for (name, sheet, anchor, start, target) in scenarios() {
        let spec = if sheet { config.motion.liquid_open } else { config.motion.liquid_small };
        let (run, drip) = run_open(new_drip(sheet), &config, anchor, start, target);
        assert!(run.peak_px <= cap_px + 1e-3, "{name}: {}px over the {cap_px}px clamp", run.peak_px);
        assert!(run.peak_pct <= spec.overshoot * 100.0 + 0.1, "{name}: {}%", run.peak_pct);
        assert!(run.settle_s <= 0.4, "{name}: settles in {}s", run.settle_s);
        // No visible second swing: the edge turns back once, by its overshoot (plus the sub-pixel
        // undershoot after a clamped peak).
        assert!(run.reversal_px <= cap_px + 0.5, "{name}: reversal {}px", run.reversal_px);
        let close = run_close(drip, &config);
        assert!(close.reversal_px <= 0.5, "{name}: closing recoils {}px", close.reversal_px);
        assert!(close.done_s <= 0.3, "{name}: closes in {}s", close.done_s);
        assert!(close.done_s < run.settle_s + 0.05, "{name}: closing is not quicker");
    }
}

#[test]
fn the_clamp_holds_a_bouncy_or_redirected_surface_inside_the_limit() {
    // A very bouncy preset: the clamp, not the damping, keeps every edge within
    // `max_overshoot_px` of its target.
    let mut config = Config::default();
    config.motion.liquid_small = LiquidSpec { response: 0.2, damping: 0.2, overshoot: 0.5 };
    config.motion.max_overshoot_px = 5.0;
    let anchor = Frame::new(100.0, 10.0, 34.0, 34.0);
    let target = Frame::new(60.0, 60.0, 300.0, 400.0);
    let mut drip = Drip::new();
    drip.open(&config, anchor, target);
    let t = edges_of(target);
    let from = edges_of(drip.frame());
    for _ in 0..(2.0 / DT) as usize {
        drip.step(DT);
        let now = edges_of(drip.frame());
        for i in 0..4 {
            let dir = (t[i] - from[i]).signum();
            assert!((now[i] - t[i]) * dir <= 5.0 + 1e-3, "edge {i} at {} vs {}", now[i], t[i]);
        }
    }
    // Retargeted mid-flight (the list grew): still clamped against the new target.
    let mut drip = Drip::new();
    drip.open(&config, anchor, target);
    for _ in 0..20 {
        drip.step(DT);
    }
    let bigger = Frame::new(60.0, 60.0, 300.0, 600.0);
    drip.retarget(bigger);
    let t = edges_of(bigger);
    for _ in 0..(2.0 / DT) as usize {
        drip.step(DT);
        let bottom = edges_of(drip.frame())[BOTTOM];
        assert!(bottom - t[BOTTOM] <= 5.0 + 1e-3, "bottom {bottom} vs {}", t[BOTTOM]);
    }
}

#[test]
fn a_closing_surface_never_passes_the_origin_even_with_momentum() {
    let config = Config::default();
    let anchor = Frame::new(1180.0, 14.0, 34.0, 34.0);
    let target = Frame::new(900.0, 58.0, 320.0, 260.0);
    let mut drip = Drip::new();
    drip.open(&config, anchor, target);
    // Closed while still opening, with every edge in motion.
    for _ in 0..24 {
        drip.step(DT);
    }
    let c = anchor.center();
    let centre = [c.x, c.y, c.x, c.y];
    let side: Vec<f32> = edges_of(drip.frame()).iter().zip(centre).map(|(e, c)| (e - c).signum()).collect();
    drip.close(&config);
    for _ in 0..(1.0 / DT) as usize {
        if drip.step(DT) == Phase::Closed {
            break;
        }
        let f = edges_of(drip.frame());
        // No edge crosses the anchor's centre on its way in.
        for i in 0..4 {
            assert!((f[i] - centre[i]) * side[i] >= -0.6, "edge {i} crossed: {} vs {}", f[i], centre[i]);
        }
    }
    assert_eq!(drip.phase(), Phase::Closed);
}

#[test]
fn reduce_motion_fades_in_place_without_travel_or_overshoot() {
    let mut config = Config::default();
    config.glass.reduce_motion = true;
    let anchor = Frame::new(1180.0, 14.0, 34.0, 34.0);
    let target = Frame::new(900.0, 58.0, 320.0, 260.0);
    let mut drip = Drip::new();
    drip.open(&config, anchor, target);
    assert!(drip.fades());
    let lo = config.motion.fade_scale_from;
    let mut last = 0.0;
    for _ in 0..(1.0 / DT) as usize {
        drip.step(DT);
        let f = drip.frame();
        // Scale between the start of the fade and full size, never above it; no travel.
        let k = f.size.x / target.size.x;
        assert!(k >= lo - 1e-3 && k <= 1.0 + 1e-3, "scale {k}");
        assert!((f.center() - target.center()).length() < 0.5);
        assert!(drip.fade() >= last - 1e-5 && drip.fade() <= 1.0);
        last = drip.fade();
    }
    assert_eq!(drip.phase(), Phase::Open);
    assert!(drip.fade() > 0.99);
}
