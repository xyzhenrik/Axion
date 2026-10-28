//! The screenshot tool's interface: a dim overlay over the page with a drag-to-select
//! rectangle, a glass toolbar to choose what to capture (selection, visible area, full
//! page), and a glass review card with the result (copy, save). Capturing itself is the
//! engine's (see `browser/capture.rs`).

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_i18n::tr;
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::print_dialog::{button, chip, gap};

const MODE_BASE: u32 = 9400;
/// The toolbar's buttons (for the capture aids of `--open`).
pub(crate) const MODE_VISIBLE: ActionId = ActionId(9401);
pub(crate) const MODE_FULL: ActionId = ActionId(9402);
const CLOSE: ActionId = ActionId(9405);
const REVIEW_BACKDROP: ActionId = ActionId(9410);
const COPY: ActionId = ActionId(9411);
const SAVE: ActionId = ActionId(9412);
const DISMISS: ActionId = ActionId(9413);

const CARD_WIDTH: f32 = 460.0;
const PREVIEW_MAX: Vec2 = Vec2::new(416.0, 280.0);
const RADIUS: f32 = 24.0;
const START_SCALE: f32 = 0.88;
const BACKDROP_DIM: f32 = 0.28;
const TOOLBAR_MARGIN: f32 = 24.0;
const EDGE: f32 = 2.0;

/// What the next capture takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShotMode {
    Selection,
    Visible,
    FullPage,
}

const MODES: [ShotMode; 3] = [ShotMode::Selection, ShotMode::Visible, ShotMode::FullPage];

/// A click in the tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShotAction {
    Mode(ShotMode),
    Close,
    Copy,
    Save,
    Dismiss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// Choosing what to capture.
    Select,
    /// Waiting for the engine.
    Capturing,
    /// Showing the result.
    Review,
}

pub struct ShotTool {
    overlay: NodeId,
    dims: [NodeId; 4],
    edges: [NodeId; 4],
    size_chip: NodeId,
    size_label: NodeId,
    toolbar: NodeId,
    review_root: NodeId,
    review_card: NodeId,
    phase: Phase,
    mode: ShotMode,
    page: Frame,
    /// The selection's two corners (window coordinates) while or after dragging.
    drag: Option<(Vec2, Vec2)>,
    dragging: bool,
    toolbar_dirty: bool,
    review_dirty: bool,
    review: Option<Review>,
    appear: Animated<f32>,
    review_open: bool,
}

struct Review {
    image: (wgpu::TextureView, (u32, u32)),
    info: String,
}

fn absolute(ui: &mut Ui, parent: NodeId, widget: Widget) -> NodeId {
    ui.add(
        parent,
        widget,
        Style {
            position: Position::Absolute,
            size: Size { width: length(0.0), height: length(0.0) },
            ..Default::default()
        },
    )
}

impl ShotTool {
    pub fn build(ui: &mut Ui) -> Self {
        let overlay = ui.add(
            ui.root(),
            Widget::container().without_feedback(),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
                ..Default::default()
            },
        );
        let dim = |ui: &mut Ui| {
            absolute(
                ui,
                overlay,
                Widget::container()
                    .without_feedback()
                    .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), 1.0)),
            )
        };
        let dims = [dim(ui), dim(ui), dim(ui), dim(ui)];
        let edge = |ui: &mut Ui| {
            absolute(
                ui,
                overlay,
                Widget::container()
                    .without_feedback()
                    .with_background(Background::Fill(Paint::Fixed(Color::rgb(0xFFFFFF)), 0.95)),
            )
        };
        let edges = [edge(ui), edge(ui), edge(ui), edge(ui)];
        let size_chip = ui.add(
            overlay,
            Widget::container()
                .without_feedback()
                .with_radius(CAPSULE)
                .with_background(Background::Glass(GlassLevel::Thin)),
            Style {
                position: Position::Absolute,
                padding: Rect {
                    left: length(10.0),
                    right: length(10.0),
                    top: length(4.0),
                    bottom: length(4.0),
                },
                ..Default::default()
            },
        );
        let size_label = ui.add_label(size_chip, "", TextStyle::Footnote, Style::default());
        ui.set_label_lines(size_label, 1);
        let toolbar = ui.add(
            overlay,
            Widget::container().with_radius(CAPSULE).with_background(Background::Glass(GlassLevel::Regular)),
            Style {
                position: Position::Absolute,
                gap: gap(8.0, 0.0),
                align_items: AlignItems::CENTER,
                padding: Rect {
                    left: length(10.0),
                    right: length(10.0),
                    top: length(8.0),
                    bottom: length(8.0),
                },
                ..Default::default()
            },
        );
        let review_root = ui.add(
            ui.root(),
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), BACKDROP_DIM))
                .without_feedback()
                .with_action(REVIEW_BACKDROP),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let review_card = ui.add(
            review_root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(RADIUS),
            Style {
                size: Size { width: length(CARD_WIDTH), height: auto() },
                max_size: Size { width: percent(0.94), height: auto() },
                flex_direction: FlexDirection::Column,
                gap: gap(0.0, 14.0),
                padding: Rect {
                    left: length(22.0),
                    right: length(22.0),
                    top: length(22.0),
                    bottom: length(20.0),
                },
                ..Default::default()
            },
        );
        ui.set_visible(overlay, false);
        ui.set_visible(review_root, false);
        Self {
            overlay,
            dims,
            edges,
            size_chip,
            size_label,
            toolbar,
            review_root,
            review_card,
            phase: Phase::Idle,
            mode: ShotMode::Selection,
            page: Frame::new(0.0, 0.0, 0.0, 0.0),
            drag: None,
            dragging: false,
            toolbar_dirty: false,
            review_dirty: false,
            review: None,
            appear: Animated::new(0.0, Spring::SNAPPY),
            review_open: false,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Test aid: shows `rect` as if it had been dragged.
    pub fn test_selection(&mut self, rect: Frame) {
        self.drag = Some((rect.origin, rect.origin + rect.size));
    }

    pub fn is_active(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// The pointer is over the toolbar (its buttons get the click).
    pub fn toolbar_contains(&self, ui: &Ui, p: Vec2) -> bool {
        ui.is_visible(self.toolbar) && ui.frame(self.toolbar).contains(p)
    }

    pub fn review_contains(&self, ui: &Ui, p: Vec2) -> bool {
        ui.frame(self.review_card).contains(p)
    }

    /// Starts choosing what to capture over `page` (the page area, window coordinates).
    pub fn start(&mut self, ui: &mut Ui, page: Frame) {
        self.phase = Phase::Select;
        self.mode = ShotMode::Selection;
        self.page = page;
        self.drag = None;
        self.dragging = false;
        self.toolbar_dirty = true;
        ui.set_visible(self.overlay, true);
    }

    /// Back to nothing: the overlay and the review go away.
    pub fn close(&mut self, ui: &mut Ui) {
        self.phase = Phase::Idle;
        self.drag = None;
        self.dragging = false;
        self.review = None;
        ui.set_visible(self.overlay, false);
        self.review_open = false;
        self.appear.target = 0.0;
    }

    pub fn set_mode(&mut self, mode: ShotMode) {
        self.mode = mode;
        self.toolbar_dirty = true;
        if mode != ShotMode::Selection {
            self.drag = None;
        }
    }

    /// The tool waits for the engine; the overlay goes away so the page is captured clean.
    pub fn capturing(&mut self, ui: &mut Ui) {
        self.phase = Phase::Capturing;
        ui.set_visible(self.overlay, false);
    }

    pub fn action(&self, id: ActionId) -> Option<ShotAction> {
        match id {
            CLOSE => Some(ShotAction::Close),
            COPY => Some(ShotAction::Copy),
            SAVE => Some(ShotAction::Save),
            DISMISS | REVIEW_BACKDROP => Some(ShotAction::Dismiss),
            ActionId(n) if (MODE_BASE..MODE_BASE + 3).contains(&n) => {
                Some(ShotAction::Mode(MODES[(n - MODE_BASE) as usize]))
            }
            _ => None,
        }
    }

    // ---- selecting ----

    fn clamp(&self, p: Vec2) -> Vec2 {
        p.clamp(self.page.origin, self.page.origin + self.page.size)
    }

    pub fn pointer_down(&mut self, p: Vec2) {
        if self.phase != Phase::Select || self.mode != ShotMode::Selection {
            return;
        }
        let p = self.clamp(p);
        self.drag = Some((p, p));
        self.dragging = true;
    }

    pub fn pointer_move(&mut self, p: Vec2) {
        if !self.dragging {
            return;
        }
        let p = self.clamp(p);
        if let Some((_, end)) = &mut self.drag {
            *end = p;
        }
    }

    /// Ends the drag: the selected rectangle (window coordinates) if it is big enough.
    pub fn pointer_up(&mut self, p: Vec2, min: f32) -> Option<Frame> {
        if !self.dragging {
            return None;
        }
        self.pointer_move(p);
        self.dragging = false;
        let rect = self.selection()?;
        if rect.size.x < min || rect.size.y < min {
            self.drag = None;
            return None;
        }
        Some(rect)
    }

    pub fn selection(&self) -> Option<Frame> {
        let (a, b) = self.drag?;
        let (lo, hi) = (a.min(b), a.max(b));
        Some(Frame { origin: lo, size: hi - lo })
    }

    // ---- review ----

    /// Shows the result: `image` is its texture, `info` the line under it.
    pub fn show_review(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        image: (wgpu::TextureView, (u32, u32)),
        info: String,
    ) {
        self.phase = Phase::Review;
        self.review = Some(Review { image, info });
        self.review_dirty = true;
        ui.set_visible(self.overlay, false);
        ui.set_visible(self.review_root, true);
        self.review_open = true;
        self.appear.spring = if config.glass.reduce_motion {
            Spring::new(0.15, 1.0)
        } else {
            Spring::new(
                config.motion.snappy.response * config.motion.speed,
                config.glass.damping(config.motion.snappy.damping),
            )
        };
        self.appear.snap(0.0);
        self.appear.target = 1.0;
    }

    fn rebuild_toolbar(&mut self, ui: &mut Ui) {
        self.toolbar_dirty = false;
        ui.clear_children(self.toolbar);
        let names = [tr("Selection"), tr("Visible Area"), tr("Full Page")];
        for (i, mode) in MODES.iter().enumerate() {
            chip(ui, self.toolbar, ActionId(MODE_BASE + i as u32), names[i], self.mode == *mode);
        }
        chip(ui, self.toolbar, CLOSE, tr("Cancel"), false);
    }

    fn rebuild_review(&mut self, ui: &mut Ui) {
        self.review_dirty = false;
        let Some(review) = &self.review else { return };
        let card = self.review_card;
        ui.clear_children(card);
        let head = ui.add(
            card,
            Widget::container(),
            Style { align_items: AlignItems::CENTER, gap: gap(12.0, 0.0), ..Default::default() },
        );
        ui.add(head, Widget::icon(Icon::Screen, 26.0).with_paint(Paint::Accent), Style::default());
        let title = ui.add_label(head, tr("Screenshot"), TextStyle::BodyEmphasis, Style::default());
        ui.set_label_lines(title, 1);
        let (w, h) = review.image.1;
        let scale = (PREVIEW_MAX.x / w as f32).min(PREVIEW_MAX.y / h as f32).min(1.0);
        let (pw, ph) = ((w as f32 * scale).max(1.0), (h as f32 * scale).max(1.0));
        let holder = ui.add(
            card,
            Widget::container().with_radius(12.0).with_background(Background::Fill(Paint::Label, 0.06)),
            Style {
                justify_content: JustifyContent::CENTER,
                padding: Rect {
                    left: length(8.0),
                    right: length(8.0),
                    top: length(8.0),
                    bottom: length(8.0),
                },
                ..Default::default()
            },
        );
        let image = ui.add(
            holder,
            Widget::image().with_radius(6.0),
            Style {
                size: Size { width: length(pw), height: length(ph) },
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        ui.set_image(image, Some(review.image.clone()));
        let info = ui.add_label(card, &review.info, TextStyle::Footnote, Style::default());
        ui.set_label_lines(info, 2);
        if let Some(wid) = ui.widget_mut(info) {
            wid.set_paint(Paint::Secondary);
        }
        let buttons = ui.add(
            card,
            Widget::container(),
            Style {
                gap: gap(10.0, 0.0),
                margin: Rect { left: length(0.0), right: length(0.0), top: length(4.0), bottom: length(0.0) },
                ..Default::default()
            },
        );
        button(ui, buttons, DISMISS, tr("Close"), false);
        button(ui, buttons, COPY, tr("Copy"), false);
        button(ui, buttons, SAVE, tr("Save"), true);
    }

    /// Lays the overlay out and advances animations. Call every frame.
    pub fn animate(&mut self, ui: &mut Ui, config: &Config, page: Frame, dt: f32) {
        if self.phase == Phase::Select {
            self.page = page;
            if self.toolbar_dirty {
                self.rebuild_toolbar(ui);
            }
            self.layout_overlay(ui, config);
        }
        if self.review_dirty {
            self.rebuild_review(ui);
        }
        if ui.is_visible(self.review_root) {
            self.appear.step(dt);
            let k = self.appear.value.clamp(0.0, 1.2);
            if !self.review_open && k < 0.01 {
                ui.set_visible(self.review_root, false);
                return;
            }
            ui.set_opacity(self.review_root, k.min(1.0));
            let frame = ui.frame(self.review_card);
            if frame.size.x > 0.0 {
                let scale = START_SCALE + (1.0 - START_SCALE) * k;
                let size = frame.size * scale;
                let origin = frame.origin + (frame.size - size) / 2.0;
                ui.set_visual_frame(self.review_card, Some(Frame { origin, size: size.max(Vec2::ONE) }));
            }
        }
    }

    fn layout_overlay(&self, ui: &mut Ui, config: &Config) {
        let page = self.page;
        let (px, py) = (page.origin.x, page.origin.y);
        let (pr, pb) = (px + page.size.x, py + page.size.y);
        let dim = config.screenshot.dim_opacity.clamp(0.0, 1.0);
        let sel = match self.mode {
            ShotMode::Selection => self.selection(),
            _ => None,
        };
        let rects: [Frame; 4] = match sel {
            Some(s) => {
                let (sx, sy) = (s.origin.x, s.origin.y);
                let (sr, sb) = (sx + s.size.x, sy + s.size.y);
                [
                    Frame::new(px, py, page.size.x, sy - py),
                    Frame::new(px, sb, page.size.x, pb - sb),
                    Frame::new(px, sy, sx - px, s.size.y),
                    Frame::new(sr, sy, pr - sr, s.size.y),
                ]
            }
            None => [
                Frame::new(px, py, page.size.x, page.size.y),
                Frame::new(0.0, 0.0, 0.0, 0.0),
                Frame::new(0.0, 0.0, 0.0, 0.0),
                Frame::new(0.0, 0.0, 0.0, 0.0),
            ],
        };
        for (node, rect) in self.dims.iter().zip(rects) {
            ui.set_visual_frame(*node, Some(rect));
            ui.set_opacity(*node, dim);
        }
        match sel {
            Some(s) => {
                let (sx, sy, sw, sh) = (s.origin.x, s.origin.y, s.size.x, s.size.y);
                let lines = [
                    Frame::new(sx, sy - EDGE, sw, EDGE),
                    Frame::new(sx, sy + sh, sw, EDGE),
                    Frame::new(sx - EDGE, sy - EDGE, EDGE, sh + 2.0 * EDGE),
                    Frame::new(sx + sw, sy - EDGE, EDGE, sh + 2.0 * EDGE),
                ];
                for (node, rect) in self.edges.iter().zip(lines) {
                    ui.set_visual_frame(*node, Some(rect));
                }
                ui.set_text(
                    self.size_label,
                    &format!("{} \u{00D7} {}", sw.round() as i32, sh.round() as i32),
                );
                let chip = ui.frame(self.size_chip).size;
                let below = sy + sh + 8.0 + chip.y <= pb;
                let y = if below { sy + sh + 8.0 } else { (sy - 8.0 - chip.y).max(py) };
                let x = (sx + sw / 2.0 - chip.x / 2.0).clamp(px, (pr - chip.x).max(px));
                ui.set_visible(self.size_chip, true);
                ui.set_visual_frame(self.size_chip, Some(Frame { origin: Vec2::new(x, y), size: chip }));
            }
            None => {
                for node in &self.edges {
                    ui.set_visual_frame(*node, Some(Frame::new(0.0, 0.0, 0.0, 0.0)));
                }
                ui.set_visible(self.size_chip, false);
            }
        }
        let bar = ui.frame(self.toolbar).size;
        let x = px + (page.size.x - bar.x) / 2.0;
        let y = pb - TOOLBAR_MARGIN - bar.y;
        // Placed by layout (not a visual frame): its glass buttons must move with it.
        let mut style = ui.style(self.toolbar);
        let inset = Rect { left: length(x.max(px)), top: length(y.max(py)), right: auto(), bottom: auto() };
        if style.inset != inset {
            style.inset = inset;
            ui.set_style(self.toolbar, style);
        }
    }
}
