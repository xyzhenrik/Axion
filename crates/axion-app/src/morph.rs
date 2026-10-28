//! A modal sheet (Settings, Passwords) that comes out of the glass it was opened from and
//! drains back into it. Its glass is the drip of [`Drip`]: it starts as a sliver along the
//! middle of the origin glass (the address field, the account button), flows out of it to the
//! sheet's frame, and on close shrinks back into that glass's centre, neck first.
//!
//! The sheet lies in a plane of its own above the chrome (so the window behind it can dim),
//! where its glass can't merge with the origin's. So while the drop is small and near its
//! origin (grown less than [`HANDOFF`]) it is drawn instead by a proxy in the origin's plane:
//! glass of the origin's level and the sheet's density, joined to the origin glass by the
//! popover bridge (see [`crate::popover::bridge_frame`]) in the same pass, so the smooth-min
//! makes them one liquid shape. Past the hand-off the sheet's own (thick) glass takes over and
//! the sheet stands on its own: it never stays joined to the bar while open. The content shows only once the glass has grown past
//! the hand-off (axion-ui maps it onto the animated glass and fades it with its growth), and
//! the backdrop's dim fades in after it (and out before it).

use std::collections::HashMap;

use axion_compositor::Rect as Frame;
use axion_config::{Color, Config};
use axion_ui::taffy::prelude::*;
use axion_ui::{Background, CAPSULE, Paint, Ui, Widget};
use glam::Vec2;

use crate::drip::{Drip, Phase};

/// Glass bigger than this in both directions (logical px: the sidebar) is no origin of its
/// own: the control that was clicked on it is, as a capsule.
const ORIGIN_MAX: f32 = 120.0;
/// Below this growth (0 at the origin, 1 open) the drop is drawn in the origin's plane, merged
/// with the origin glass. The content shows only well above it (from 0.55 of the sheet's
/// size, see axion-ui), so the swap only ever trades bare glass for bare glass; it is low so
/// the drop is small when its glass changes from the bar's level to the sheet's thicker one.
const HANDOFF: f32 = 0.35;

/// Where a sheet comes from (logical px): a point in a plane (a child of the root). The glass
/// there is found again when the sheet closes, so it drains into it wherever it is by then.
#[derive(Clone, Copy, Debug)]
pub struct Origin {
    pub plane: NodeId,
    pub point: Vec2,
    /// The control that was clicked (used when the glass under it is a big surface).
    pub control: Frame,
}

/// The drop's stand-in in an origin's plane: its glass and the bridge to the origin glass.
#[derive(Clone, Copy)]
struct Proxy {
    glass: NodeId,
    bridge: NodeId,
}

pub struct SheetMorph {
    backdrop: NodeId,
    sheet: NodeId,
    /// The backdrop's dim, faded out with the sheet's growth as it drains …
    dim: f32,
    /// … and in as it opens, unless something on the backdrop dimmed it already (the
    /// Passwords unlock alert).
    dim_in: bool,
    radius: f32,
    drip: Drip,
    origin: Option<Origin>,
    /// The origin glass the drop flows out of and back into (frame and corner radius).
    join: Option<(Frame, f32)>,
    bridge_depth: f32,
    /// Built per origin plane on first use.
    proxies: HashMap<NodeId, Proxy>,
}

impl SheetMorph {
    /// For `sheet` (glass, corner `radius`) on `backdrop` (a plane; filled with
    /// black at `dim` while it is open).
    pub fn build(sheet: NodeId, backdrop: NodeId, radius: f32, dim: f32) -> Self {
        Self {
            backdrop,
            sheet,
            dim,
            dim_in: true,
            radius,
            drip: Drip::new(),
            origin: None,
            join: None,
            bridge_depth: 0.0,
            proxies: HashMap::new(),
        }
    }

    /// Whether the sheet is on screen (open, or still flowing in or out).
    pub fn is_shown(&self) -> bool {
        self.drip.phase() != Phase::Closed
    }

    fn proxy(&mut self, ui: &mut Ui, plane: NodeId) -> Proxy {
        if let Some(&proxy) = self.proxies.get(&plane) {
            return proxy;
        }
        let glass = ui.add(
            plane,
            Widget::container()
                .with_background(Background::Glass(crate::popover::LEVEL))
                .with_glass_density(crate::popover::DENSITY)
                .with_radius(self.radius),
            Style { position: Position::Absolute, ..Default::default() },
        );
        // After the drop, so its wide smooth union flows into it (see `crate::popover`).
        let bridge = ui.add(
            plane,
            Widget::container().with_background(Background::Glass(crate::popover::LEVEL)).as_join(),
            Style { position: Position::Absolute, ..Default::default() },
        );
        ui.set_visible(glass, false);
        ui.set_visible(bridge, false);
        let proxy = Proxy { glass, bridge };
        self.proxies.insert(plane, proxy);
        proxy
    }

    /// The origin's glass: frame and corner radius (logical px), leaving out our proxies.
    fn origin_glass(&self, ui: &Ui, origin: &Origin) -> Option<(Frame, f32)> {
        if !ui.is_visible(origin.plane) {
            return None;
        }
        let skip: Vec<NodeId> = self.proxies.values().flat_map(|p| [p.glass, p.bridge]).collect();
        match ui.glass_at(origin.plane, origin.point, &skip) {
            Some((frame, radius, _)) if frame.size.min_element() <= ORIGIN_MAX => Some((frame, radius)),
            Some(_) => Some((origin.control, CAPSULE.min(origin.control.size.min_element() / 2.0))),
            None => None,
        }
    }

    /// `origin`, or `fallback` when no glass is there, with its glass.
    fn resolve(&self, ui: &Ui, origin: Origin, fallback: Origin) -> (Origin, Option<(Frame, f32)>) {
        match self.origin_glass(ui, &origin) {
            Some(glass) => (origin, Some(glass)),
            None => (fallback, self.origin_glass(ui, &fallback)),
        }
    }

    fn use_origin(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        origin: Origin,
        glass: Option<(Frame, f32)>,
    ) -> Frame {
        self.origin = Some(origin);
        self.join = glass;
        self.bridge_depth = config.glass.bezel_width.max(0.0);
        let proxy = self.proxy(ui, origin.plane);
        if let Some(w) = ui.widget_mut(proxy.bridge) {
            w.glass_merge = Some(config.glass.popover_bridge_fillet());
        }
        glass.map_or(Frame { origin: origin.point, size: Vec2::ZERO }, |(f, _)| f)
    }

    /// Flows out of `origin` (or `fallback`, when nothing is there) to `target`, the sheet's
    /// laid out frame (logical px). `dim_in`: the backdrop's dim fades in with it (else it is
    /// dimmed already).
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        origin: Origin,
        fallback: Origin,
        target: Frame,
        dim_in: bool,
    ) {
        if matches!(self.drip.phase(), Phase::Open | Phase::Opening) {
            self.drip.retarget(target);
            return;
        }
        self.dim_in = dim_in;
        let (origin, glass) = self.resolve(ui, origin, fallback);
        let anchor = self.use_origin(ui, config, origin, glass);
        // A sliver along the origin's middle: nothing shows over it at first.
        let start = Frame::new(anchor.origin.x, anchor.center().y, anchor.size.x, 0.0);
        if self.drip.phase() == Phase::Closed {
            self.drip.open_from(config, anchor, start, target);
        } else {
            self.drip.show(config, anchor, target);
        }
        self.animate(ui, 0.0);
    }

    /// The window resized: the open sheet glides to its new frame.
    pub fn retarget(&mut self, target: Frame) {
        self.drip.retarget(target);
    }

    /// Drains back into the glass it came from (or `fallback`, when that is gone).
    pub fn close(&mut self, ui: &mut Ui, config: &Config, fallback: Origin) {
        if self.drip.phase() == Phase::Closed {
            return;
        }
        let (origin, glass) = self.resolve(ui, self.origin.unwrap_or(fallback), fallback);
        let anchor = self.use_origin(ui, config, origin, glass);
        self.drip.close_into(config, anchor);
        self.animate(ui, 0.0);
    }

    /// Advances the flow; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) -> Phase {
        let phase = self.drip.step(dt);
        let current = self.origin.and_then(|o| self.proxies.get(&o.plane).copied());
        // Proxies of other planes (an earlier origin) stay hidden.
        for proxy in self.proxies.values().filter(|p| Some(p.glass) != current.map(|c| c.glass)) {
            if ui.is_visible(proxy.glass) {
                ui.set_visible(proxy.glass, false);
                ui.set_visible(proxy.bridge, false);
            }
        }
        if phase == Phase::Closed {
            if let Some(proxy) = current
                && ui.is_visible(proxy.glass)
            {
                ui.set_visible(proxy.glass, false);
                ui.set_visible(proxy.bridge, false);
            }
            ui.set_opacity(self.sheet, 1.0);
            ui.set_visual_frame(self.sheet, None);
            // Ready for the next time the backdrop shows (its owner hides it now).
            self.set_dim(ui, self.dim);
            return phase;
        }
        // The origin glass may move (the bar while the window changes).
        if let Some(origin) = self.origin
            && let Some(glass) = self.origin_glass(ui, &origin)
        {
            self.join = Some(glass);
        }
        let frame = self.drip.frame();
        let growth = self.drip.growth();
        // Flat mode has no drop to hand off: the sheet itself fades in and out.
        let near = phase != Phase::Open && growth < HANDOFF && current.is_some() && !ui.theme().flat();
        ui.claim_space(self.sheet);
        ui.set_visual_frame(self.sheet, Some(frame));
        ui.set_opacity(self.sheet, if near { 0.0 } else { self.drip.fade() });
        if let Some(proxy) = current {
            ui.set_visible(proxy.glass, near);
            ui.set_visual_frame(proxy.glass, near.then_some(frame));
            let bridge = self.join.filter(|_| near).and_then(|(join, r)| {
                crate::popover::bridge_frame(join, r, frame, self.radius, self.bridge_depth)
            });
            ui.set_visible(proxy.bridge, bridge.is_some());
            ui.set_visual_frame(proxy.bridge, bridge);
        }
        // Dims after the hand-off, so the drop near the bar isn't dimmed.
        let k = ((growth - HANDOFF) / (1.0 - HANDOFF)).clamp(0.0, 1.0);
        let fading = phase == Phase::Closing || self.dim_in;
        self.set_dim(ui, if fading { self.dim * k } else { self.dim });
        phase
    }

    fn set_dim(&self, ui: &mut Ui, dim: f32) {
        if let Some(w) = ui.widget_mut(self.backdrop) {
            w.background = Background::Fill(Paint::Fixed(Color::rgb(0x000000)), dim);
        }
    }
}
