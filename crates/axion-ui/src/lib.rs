//! Axion's retained UI: a tree of widgets laid out with taffy (flexbox), drawn as Liquid Glass,
//! shapes, text and icons through the compositor.
//!
//! The app builds the tree once and updates it in place (`set_text`, `set_visible`, …).
//! Input goes through [`Ui::handle`], which hit-tests, tracks hover/press/focus and edits
//! text fields, and reports [`UiEvent`]s. Layout is in logical pixels; painting converts to
//! physical pixels with the window's scale factor.

mod editor;
mod knob;
mod theme;
mod widget;

use std::cell::{Cell, RefCell, RefMut};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use axion_compositor::glyphon::{self, FontSystem};
use axion_compositor::{Content, IconDraw, ImageDraw, Layer, Quad, Rect};
use axion_config::{Key, Modifiers};
use axion_glass::GlassShape;
use axion_physics::{Animated, Spring};
use glam::Vec2;
pub use taffy;
use taffy::prelude::*;

pub use editor::Editor;
use knob::Knob;
pub use theme::{FLAT_BORDER, FlatPaint, GlassLevel, Paint, TextStyle, Theme};
pub use widget::{ActionId, Align, Background, CAPSULE, FlatRole, Shadow, Tooltip, Widget, WidgetKind};
use widget::{break_lines, caret_x, index_at, layout_wrapped, measure, new_buffer, set_buffer_text};

const CARET_BLINK: Duration = Duration::from_millis(530);
const CARET_WIDTH: f32 = 1.5;
/// Feedback fills over buttons, as a fraction of the label colour.
const HOVER_ALPHA: f32 = 0.07;
const PRESS_ALPHA: f32 = 0.13;
const DISABLED_ALPHA: f32 = 0.4;
/// Toggle track size and slider knob diameter (logical px), as on iOS.
/// iPadOS 26 switch: track, and the pill knob inside it (logical px).
const TOGGLE_SIZE: Vec2 = Vec2::new(63.0, 28.0);
const TOGGLE_INSET: f32 = 2.5;
/// Switches are green when on, whatever the accent (system convention).
const SWITCH_ON: [f32; 4] = [0.204, 0.780, 0.349, 1.0];
const TOGGLE_KNOB_ASPECT: f32 = 1.55;
/// A held knob's lens, relative to the knob (width, height).
const LENS_GROWTH: Vec2 = Vec2::new(1.45, 1.5);
/// How much a fast-moving lens stretches along its motion.
const LENS_SQUASH: f32 = 0.012;
/// A moving lens stretches up to this many times `glass.squash`.
const LENS_SQUASH_RATIO: f32 = 1.75;
const SLIDER_KNOB: f32 = 24.0;
const SLIDER_TRACK: f32 = 4.0;
/// Line slider thickness at rest and while dragged.
const LINE_SLIDER: f32 = 4.0;
const LINE_SLIDER_HELD: f32 = 8.0;
/// A line slider with its own thickness swells to this many times it while dragged.
const LINE_SLIDER_SWELL: f32 = 1.5;
/// The lens of a dragged line slider, as a multiple of the track's thickness at rest.
const LINE_LENS: f32 = 2.4;
/// Below this opacity a widget (and its subtree) isn't drawn.
const MIN_VISIBLE: f32 = 0.01;
/// How much glass swells while a button on it is held.
const PRESS_SCALE: f32 = 1.06;
/// Limits of `appearance.text_scale`.
const MIN_TEXT_SCALE: f32 = 0.5;
const MAX_TEXT_SCALE: f32 = 3.0;
/// A held glass grows at most this many logical px at each edge (wide bars barely swell) …
const PRESS_GROW_PX: f32 = 5.0;
/// … and surfaces bigger than this (sheets, the sidebar) don't react at all.
const PRESS_MAX_SIZE: f32 = 200.0;
/// How far held glass follows the pointer when it is dragged away (rubber band limit, px) …
const PULL_LIMIT: f32 = 5.0;
/// … and how much it stretches toward it (relative to the pull over the glass's size).
const PULL_STRETCH: f32 = 1.4;
const PRESS_SETTLED: f32 = 0.001;
/// Clicks closer together than this (time, then distance in logical px) count as a double
/// (or triple) click in a text field.
const MULTI_CLICK_TIME: std::time::Duration = std::time::Duration::from_millis(450);
const MULTI_CLICK_DISTANCE: f32 = 4.0;
/// A pressed button on glass becomes a glass lens of its own: it swells by at most this
/// factor (the round back button, like iPadOS 26) …
const LENS_PRESS_SCALE: f32 = 1.4;
/// … growing at most this many logical px at each edge (wide menu rows barely swell).
const LENS_PRESS_GROW_PX: f32 = 8.0;
/// Glass buttons up to this size (logical px) turn into a lens themselves when pressed;
/// bigger glass (bars, the address field) swells as a group with its content held in place.
const LENS_BUTTON_MAX: f32 = 64.0;
/// A button at least this many times longer than wide is a row: its lens stays inside the
/// glass along its length.
const LENS_ROW_ASPECT: f32 = 2.5;
/// How much lighter a fully pressed lens is than the glass around it (light, dark scheme).
const LENS_PRESS_GLOW: [f32; 2] = [0.16, 0.09];
/// Content on glass whose background is animated (`Widget::visual_frame`: popovers dripping
/// out of a button, alerts scaling in) is mapped onto that background and shows only once it
/// has grown to this share of its layout size …
const CONTENT_REVEAL_FROM: f32 = 0.55;
/// … and fully from this share on, so glass and content appear and vanish together.
const CONTENT_REVEAL_TO: f32 = 0.9;
/// Fading glass is grouped into this many opacity steps (each step is one glass pass).
const GLASS_FADE_STEPS: f32 = 24.0;
/// The resting pill of a segmented control, relative to the surface colour.
const SEGMENT_PILL_OPACITY: f32 = 0.92;

/// Input in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum UiInput {
    PointerMove(Vec2),
    PointerLeave,
    PointerDown,
    PointerUp,
    Key { key: Key, modifiers: Modifiers },
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Clicked {
        action: ActionId,
        node: NodeId,
    },
    /// Enter in a text field.
    Submitted {
        node: NodeId,
        text: String,
    },
    /// Escape in a text field (it loses focus).
    Cancelled {
        node: NodeId,
    },
    FocusChanged {
        node: Option<NodeId>,
    },
    /// The user copied or cut text from a field; put it on the clipboard.
    Copy(String),
    /// The user asked to paste (Ctrl+V) into a field; call [`Ui::paste`] with the clipboard.
    PasteRequested {
        node: NodeId,
    },
    /// A toggle (0 or 1) or slider (0–1) changed.
    ValueChanged {
        node: NodeId,
        value: f32,
    },
}

/// What the pointer is over, for choosing the mouse cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiCursor {
    Default,
    Pointer,
    Text,
}

pub struct Ui {
    tree: TaffyTree<Widget>,
    root: NodeId,
    // In a RefCell so painted layers (borrowing the tree) and text rendering (needing the
    // font system mutably) can coexist.
    fonts: RefCell<FontSystem>,
    theme: Theme,
    /// The theme in dark, for planes that are always dark (see `set_plane_dark`).
    dark_theme: Theme,
    dark_planes: HashSet<NodeId>,
    /// Set while painting a dark plane.
    painting_dark: Cell<bool>,
    scale: f32,
    size: Vec2,
    pointer: Option<Vec2>,
    hover: Option<NodeId>,
    pressed: Option<NodeId>,
    /// A text field whose selection follows the held pointer, and how: by characters, or by
    /// whole words from the word a double click picked (its start, end).
    field_select: Option<(NodeId, Option<(usize, usize)>)>,
    /// The last click in a text field: when, where, and how many in a row.
    last_click: Option<(Instant, Vec2, u8)>,
    /// Glass that swells while a button on it is held, springing back on release.
    press_scale: HashMap<NodeId, Press>,
    /// The glass (or lens) being held and where the pointer went down (for the pull toward
    /// the pointer).
    press_anchor: Option<(NodeId, Vec2)>,
    /// Set while painting a clipped subtree (its glass is drawn in passes of its own, so a
    /// lens there could not flow out of the glass around it).
    painting_clipped: Cell<u32>,
    /// The clip rectangles (physical px) of the clipped subtrees being painted, innermost last:
    /// glass cannot be cut to one, so glass that is not wholly inside it is painted flat.
    clip_stack: RefCell<Vec<Rect>>,
    /// Switch and slider knobs that are held or still animating.
    knobs: HashMap<NodeId, Knob>,
    focus: Option<NodeId>,
    /// A slider being dragged.
    dragging: Option<NodeId>,
    caret_epoch: Instant,
    needs_layout: bool,
    /// Containing blocks that still list a removed absolute box (see `refresh_stale_hoists`).
    stale_hoists: Vec<NodeId>,
}

impl Ui {
    pub fn new(theme: Theme, scale: f32) -> Self {
        let mut tree = TaffyTree::new();
        let root_style =
            Style { size: Size { width: percent(1.0), height: percent(1.0) }, ..Default::default() };
        let root =
            tree.new_leaf_with_context(root_style, Widget::container()).unwrap_or_else(|_| unreachable!());
        Self {
            tree,
            root,
            fonts: RefCell::new(FontSystem::new()),
            dark_theme: Theme::new(theme.config(), true),
            dark_planes: HashSet::new(),
            painting_dark: Cell::new(false),
            theme,
            scale,
            size: Vec2::ONE,
            pointer: None,
            hover: None,
            pressed: None,
            field_select: None,
            last_click: None,
            focus: None,
            dragging: None,
            press_scale: HashMap::new(),
            press_anchor: None,
            painting_clipped: Cell::new(0),
            clip_stack: RefCell::new(Vec::new()),
            knobs: HashMap::new(),
            caret_epoch: Instant::now(),
            needs_layout: true,
            stale_hoists: Vec::new(),
        }
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// The font system, for the compositor's text rendering.
    pub fn fonts(&self) -> RefMut<'_, FontSystem> {
        self.fonts.borrow_mut()
    }

    pub fn set_theme(&mut self, theme: Theme) {
        let text_changed =
            theme.font_family() != self.theme.font_family() || theme.text_scale() != self.theme.text_scale();
        self.dark_theme = Theme::new(theme.config(), true);
        self.theme = theme;
        if text_changed {
            self.reshape_all();
            self.needs_layout = true;
        }
    }

    /// How much moving glass may squash (`glass.squash`; none with Reduce Motion or flat).
    fn squash(&self) -> f32 {
        self.theme.config().glass.squash()
    }

    /// Text is shaped at the display scale times the user's text size.
    fn text_scale(&self) -> f32 {
        self.scale * self.theme.text_scale().clamp(MIN_TEXT_SCALE, MAX_TEXT_SCALE)
    }

    /// Paints a plane (a child of the root) in the dark scheme whatever the theme — for
    /// controls over video, which is dark-surrounded however the browser looks.
    pub fn set_plane_dark(&mut self, plane: NodeId, dark: bool) {
        if dark {
            self.dark_planes.insert(plane);
        } else {
            self.dark_planes.remove(&plane);
        }
    }

    fn paint_theme(&self) -> &Theme {
        if self.painting_dark.get() { &self.dark_theme } else { &self.theme }
    }

    /// Window size in logical pixels and the scale factor to physical pixels.
    pub fn set_viewport(&mut self, size: Vec2, scale: f32) {
        if self.size != size {
            self.size = size;
            self.needs_layout = true;
        }
        if self.scale != scale {
            self.scale = scale;
            self.reshape_all();
        }
    }

    fn reshape_all(&mut self) {
        let family = self.theme.font_family().to_owned();
        let text_scale = self.text_scale();
        let nodes = self.descendants(self.root);
        for node in nodes {
            let Some(widget) = self.tree.get_node_context_mut(node) else { continue };
            match &mut widget.kind {
                WidgetKind::Label(label) => {
                    label.buffer = new_buffer(self.fonts.get_mut(), label.style, text_scale);
                    set_buffer_text(
                        &mut label.buffer,
                        self.fonts.get_mut(),
                        &family,
                        &label.text,
                        label.style,
                        label.align,
                    );
                }
                WidgetKind::TextField(field) => {
                    field.buffer = new_buffer(self.fonts.get_mut(), field.style, text_scale);
                    field.placeholder = new_buffer(self.fonts.get_mut(), field.style, text_scale);
                    let text = field.display_text();
                    set_buffer_text(
                        &mut field.buffer,
                        self.fonts.get_mut(),
                        &family,
                        &text,
                        field.style,
                        Align::Start,
                    );
                    let placeholder = field.placeholder_text.clone();
                    set_buffer_text(
                        &mut field.placeholder,
                        self.fonts.get_mut(),
                        &family,
                        &placeholder,
                        field.style,
                        Align::Start,
                    );
                }
                _ => {}
            }
            let _ = self.tree.mark_dirty(node);
        }
        self.needs_layout = true;
    }

    // ---- building ----

    pub fn add(&mut self, parent: NodeId, widget: Widget, style: Style) -> NodeId {
        let node = self.tree.new_leaf_with_context(style, widget).unwrap_or_else(|_| unreachable!());
        let _ = self.tree.add_child(parent, node);
        self.needs_layout = true;
        node
    }

    pub fn add_label(&mut self, parent: NodeId, text: &str, style: TextStyle, layout: Style) -> NodeId {
        let family = self.theme.font_family().to_owned();
        let text_scale = self.text_scale();
        let widget = Widget::label(self.fonts.get_mut(), &family, text_scale, text, style);
        self.add(parent, widget, layout)
    }

    pub fn add_text_field(
        &mut self,
        parent: NodeId,
        placeholder: &str,
        style: TextStyle,
        layout: Style,
    ) -> NodeId {
        let family = self.theme.font_family().to_owned();
        let text_scale = self.text_scale();
        let widget = Widget::text_field(self.fonts.get_mut(), &family, text_scale, placeholder, style);
        self.add(parent, widget, layout)
    }

    /// Removes `node` and everything below it.
    pub fn remove(&mut self, node: NodeId) {
        let doomed = self.descendants(node);
        // An absolutely positioned box stays in the list of its containing block (see
        // `refresh_stale_hoists`): remember the blocks that outlive it.
        for &n in &doomed {
            if self.tree.style(n).is_ok_and(|s| s.position == Position::Absolute) {
                let mut up = self.tree.parent(n);
                while let Some(u) = up {
                    if self.tree.hoisted_children(u).is_ok_and(|h| h.contains(&n))
                        && !doomed.contains(&u)
                        && !self.stale_hoists.contains(&u)
                    {
                        self.stale_hoists.push(u);
                    }
                    up = self.tree.parent(u);
                }
            }
        }
        for n in doomed.into_iter().rev() {
            if self.focus == Some(n) {
                self.focus = None;
            }
            if self.hover == Some(n) {
                self.hover = None;
            }
            if self.pressed == Some(n) {
                self.pressed = None;
            }
            if self.dragging == Some(n) {
                self.dragging = None;
            }
            if self.press_anchor.is_some_and(|(held, _)| held == n) {
                self.press_anchor = None;
            }
            if self.field_select.is_some_and(|(f, _)| f == n) {
                self.field_select = None;
            }
            self.knobs.remove(&n);
            let _ = self.tree.remove(n);
        }
        self.needs_layout = true;
    }

    /// Replaces `node`'s children with `children` (existing nodes, in this order). Nodes that
    /// are dropped from the list stay alive; remove them with [`Ui::remove`].
    pub fn set_children(&mut self, node: NodeId, children: &[NodeId]) {
        let _ = self.tree.set_children(node, children);
        self.needs_layout = true;
    }

    pub fn parent(&self, node: NodeId) -> Option<NodeId> {
        self.tree.parent(node)
    }

    pub fn children(&self, node: NodeId) -> Vec<NodeId> {
        self.tree.children(node).unwrap_or_default()
    }

    pub fn clear_children(&mut self, node: NodeId) {
        for child in self.tree.children(node).unwrap_or_default() {
            self.remove(child);
        }
    }

    pub fn widget(&self, node: NodeId) -> Option<&Widget> {
        self.tree.get_node_context(node)
    }

    /// Changes appearance (paint, selected, enabled…). Changing the kind's size needs `relayout`.
    pub fn widget_mut(&mut self, node: NodeId) -> Option<&mut Widget> {
        self.tree.get_node_context_mut(node)
    }

    pub fn set_style(&mut self, node: NodeId, style: Style) {
        // Callers re-apply styles every frame (positions that follow other boxes); an unchanged
        // one must not dirty the tree, or every frame lays everything out again.
        if self.tree.style(node).is_ok_and(|s| *s == style) {
            return;
        }
        let _ = self.tree.set_style(node, style);
        self.needs_layout = true;
    }

    pub fn style(&self, node: NodeId) -> Style {
        self.tree.style(node).cloned().unwrap_or_default()
    }

    pub fn set_visible(&mut self, node: NodeId, visible: bool) {
        let mut style = self.style(node);
        let display = if visible { Display::Flex } else { Display::None };
        if style.display != display {
            style.display = display;
            self.set_style(node, style);
            if !visible && self.is_holding() {
                // A hidden control can't receive its button-up: it lets go now.
                let below = self.descendants(node);
                let held =
                    self.dragging.into_iter().chain(self.pressed).chain(self.press_anchor.map(|(g, _)| g));
                if held.into_iter().any(|h| below.contains(&h)) {
                    self.end_interaction();
                }
            }
        }
    }

    pub fn is_visible(&self, node: NodeId) -> bool {
        self.style(node).display != Display::None
    }

    pub fn set_text(&mut self, node: NodeId, text: &str) {
        let family = self.theme.font_family().to_owned();
        let Some(widget) = self.tree.get_node_context_mut(node) else { return };
        if let WidgetKind::Label(label) = &mut widget.kind
            && label.text != text
        {
            label.text = text.to_owned();
            set_buffer_text(&mut label.buffer, self.fonts.get_mut(), &family, text, label.style, label.align);
            let _ = self.tree.mark_dirty(node);
            self.needs_layout = true;
        }
    }

    /// Lets a label wrap onto up to `lines` lines (ellipsis after the last) instead of staying
    /// on one line. The label should be allowed to shrink (`min_size.width: 0`).
    pub fn set_label_lines(&mut self, node: NodeId, lines: usize) {
        let lines = lines.max(1);
        if let Some(Widget { kind: WidgetKind::Label(label), .. }) = self.tree.get_node_context_mut(node)
            && label.max_lines != lines
        {
            label.max_lines = lines;
            label.buffer.set_ellipsize(glyphon::cosmic_text::Ellipsize::None);
            let _ = self.tree.mark_dirty(node);
            self.needs_layout = true;
        }
    }

    pub fn set_label_align(&mut self, node: NodeId, align: Align) {
        let family = self.theme.font_family().to_owned();
        if let Some(Widget { kind: WidgetKind::Label(label), .. }) = self.tree.get_node_context_mut(node)
            && label.align != align
        {
            label.align = align;
            let text = label.text.clone();
            set_buffer_text(&mut label.buffer, self.fonts.get_mut(), &family, &text, label.style, align);
        }
    }

    /// Shows `view` (a texture of `size` pixels) in an image widget, or nothing.
    pub fn set_image(&mut self, node: NodeId, image: Option<(wgpu::TextureView, (u32, u32))>) {
        if let Some(Widget { kind: WidgetKind::Image { view, size, .. }, .. }) =
            self.tree.get_node_context_mut(node)
        {
            match image {
                Some((v, s)) => {
                    *view = Some(v);
                    *size = (s.0.max(1), s.1.max(1));
                }
                None => *view = None,
            }
        }
    }

    /// Shows only part of an image's texture (0–1 coordinates), stretched over its frame.
    pub fn set_image_crop(&mut self, node: NodeId, uv: Option<Rect>) {
        if let Some(Widget { kind: WidgetKind::Image { crop, .. }, .. }) =
            self.tree.get_node_context_mut(node)
        {
            *crop = uv;
        }
    }

    pub fn set_visual_frame(&mut self, node: NodeId, frame: Option<Rect>) {
        if let Some(w) = self.tree.get_node_context_mut(node) {
            w.visual_frame = frame;
        }
    }

    pub fn set_opacity(&mut self, node: NodeId, opacity: f32) {
        if let Some(w) = self.tree.get_node_context_mut(node) {
            w.opacity = opacity.clamp(0.0, 1.0);
        }
    }

    pub fn set_progress(&mut self, node: NodeId, value: f32) {
        if let Some(Widget { kind: WidgetKind::Progress { value: v, .. }, .. }) =
            self.tree.get_node_context_mut(node)
        {
            *v = value.clamp(0.0, 1.0);
        }
    }

    // ---- text fields ----

    pub fn field_text(&self, node: NodeId) -> Option<&str> {
        match &self.widget(node)?.kind {
            WidgetKind::TextField(f) => Some(f.editor.text()),
            _ => None,
        }
    }

    /// Makes a field show bullets instead of its text and refuse copy/cut (passwords).
    pub fn set_secure(&mut self, node: NodeId, secure: bool) {
        let family = self.theme.font_family().to_owned();
        if let Some(Widget { kind: WidgetKind::TextField(field), .. }) = self.tree.get_node_context_mut(node)
        {
            field.secure = secure;
            let text = field.display_text();
            set_buffer_text(
                &mut field.buffer,
                self.fonts.get_mut(),
                &family,
                &text,
                field.style,
                Align::Start,
            );
        }
    }

    pub fn set_field_text(&mut self, node: NodeId, text: &str) {
        self.edit_field(node, |e| e.set_text(text));
    }

    /// Replaces a field's text and selects `selected` (byte range), caret at its end — an
    /// inline completion that the next keystroke replaces.
    pub fn set_field_text_selecting(&mut self, node: NodeId, text: &str, selected: std::ops::Range<usize>) {
        self.edit_field(node, |e| {
            e.set_text(text);
            e.move_to(selected.start, false);
            e.move_to(selected.end, true);
        });
    }

    /// A field's caret (byte index) and selection.
    pub fn field_caret(&self, node: NodeId) -> Option<(usize, Option<std::ops::Range<usize>>)> {
        match &self.widget(node)?.kind {
            WidgetKind::TextField(f) => Some((f.editor.cursor(), f.editor.selection())),
            _ => None,
        }
    }

    pub fn focused(&self) -> Option<NodeId> {
        self.focus
    }

    /// Focuses a text field and selects its content (like clicking the address bar).
    pub fn focus(&mut self, node: NodeId) -> Vec<UiEvent> {
        if !self.widget(node).is_some_and(Widget::is_focusable) {
            return Vec::new();
        }
        self.edit_field(node, Editor::select_all);
        if self.focus == Some(node) {
            return Vec::new();
        }
        self.focus = Some(node);
        self.caret_epoch = Instant::now();
        vec![UiEvent::FocusChanged { node: Some(node) }]
    }

    pub fn blur(&mut self) -> Vec<UiEvent> {
        if self.focus.take().is_some() { vec![UiEvent::FocusChanged { node: None }] } else { Vec::new() }
    }

    /// Inserts clipboard text into the focused field.
    pub fn paste(&mut self, text: &str) {
        if let Some(node) = self.focus {
            self.edit_field(node, |e| e.insert(text));
        }
    }

    fn edit_field(&mut self, node: NodeId, edit: impl FnOnce(&mut Editor)) {
        let family = self.theme.font_family().to_owned();
        let Some(Widget { kind: WidgetKind::TextField(field), .. }) = self.tree.get_node_context_mut(node)
        else {
            return;
        };
        let before = field.editor.text().to_owned();
        edit(&mut field.editor);
        if field.editor.text() != before {
            let text = field.display_text();
            set_buffer_text(
                &mut field.buffer,
                self.fonts.get_mut(),
                &family,
                &text,
                field.style,
                Align::Start,
            );
        }
        self.caret_epoch = Instant::now();
        self.needs_layout = true;
    }

    // ---- layout ----

    pub fn layout(&mut self) {
        if !self.needs_layout {
            return;
        }
        self.needs_layout = false;
        self.refresh_stale_hoists();
        self.compute_layout();
        self.update_frames(self.root, Vec2::ZERO);
    }

    /// Debug aid for translations: every visible label whose text does not fit its box (cut by
    /// an ellipsis or the box's edge) and every text field whose placeholder is wider than the
    /// field, as `(text, box width, text width)` in logical px. Lays out first.
    pub fn clipped_texts(&mut self) -> Vec<(String, f32, f32)> {
        use glyphon::cosmic_text::{Ellipsize, EllipsizeHeightLimit};
        self.layout();
        let scale = self.scale;
        let mut out = Vec::new();
        let mut nodes = vec![self.root];
        let mut i = 0;
        while i < nodes.len() {
            let n = nodes[i];
            i += 1;
            if self.style(n).display == Display::None {
                continue;
            }
            nodes.extend(self.tree.children(n).unwrap_or_default());
            let fonts = self.fonts.get_mut();
            let Some(widget) = self.tree.get_node_context_mut(n) else { continue };
            let frame = widget.frame;
            if frame.size.x <= 0.0 || widget.opacity <= 0.0 {
                continue;
            }
            match &mut widget.kind {
                WidgetKind::Label(label) => {
                    if label.max_lines > 1 {
                        let family = self.theme.font_family();
                        let width = frame.size.x * scale + 1.0;
                        let (style, align) = (label.style, label.align);
                        let broken =
                            break_lines(&mut label.buffer, fonts, family, (&label.text, style, align), width);
                        if broken.len() > label.max_lines {
                            out.push((label.text.clone(), frame.size.x, frame.size.x * broken.len() as f32));
                        }
                        // Lines enough, but maybe a word wider than the box.
                        let ((w, _), _) = layout_wrapped(label, fonts, family, Some(width));
                        if w > width && broken.len() <= label.max_lines {
                            out.push((label.text.clone(), frame.size.x, w / scale));
                        }
                    } else {
                        label.buffer.set_ellipsize(Ellipsize::None);
                        label.buffer.set_size(None, None);
                        label.buffer.shape_until_scroll(fonts, false);
                        let (w, _) = measure(&label.buffer);
                        if w > frame.size.x * scale + 1.0 {
                            out.push((label.text.clone(), frame.size.x, w / scale));
                        }
                        label.buffer.set_ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)));
                        label.buffer.set_size(Some(frame.size.x * scale + 1.0), Some(frame.size.y * scale));
                        label.buffer.shape_until_scroll(fonts, false);
                    }
                }
                WidgetKind::TextField(field)
                    if field.editor.text().is_empty() && !field.placeholder_text.is_empty() =>
                {
                    let (w, _) = measure(&field.placeholder);
                    if w / scale > frame.size.x + 1.0 {
                        out.push((field.placeholder_text.clone(), frame.size.x, w / scale));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Taffy records the absolutely positioned boxes of a containing block while it lays that
    /// block out, but a hidden block (`display: none` itself or below a hidden node) is not laid
    /// out, so a box removed from it stays in its list and the next layout panics ("invalid
    /// SlotMap key"). Such blocks are shown for one layout pass so the list is rebuilt.
    /// A block left without children is laid out as a leaf, which keeps its old list too (even
    /// when shown): it gets a stand-in child for that pass.
    fn refresh_stale_hoists(&mut self) {
        let blocks = std::mem::take(&mut self.stale_hoists);
        let mut hidden = Vec::new();
        let mut stand_ins = Vec::new();
        for block in blocks {
            // Gone with the removed subtree (a shown block with children is refreshed by the
            // ordinary layout).
            if self.tree.get_node_context(block).is_none() {
                continue;
            }
            if self.tree.child_count(block) == 0 {
                stand_ins.push(self.add(block, Widget::container(), Style::default()));
            }
            let mut node = Some(block);
            while let Some(n) = node {
                if self.style(n).display == Display::None && !hidden.contains(&n) {
                    hidden.push(n);
                }
                node = self.tree.parent(n);
            }
        }
        // A shown block with children: the ordinary layout refreshes it.
        let shown_with_children = hidden.is_empty() && stand_ins.is_empty();
        if shown_with_children {
            return;
        }
        for &n in &hidden {
            let mut style = self.style(n);
            style.display = Display::Flex;
            self.set_style(n, style);
        }
        self.compute_layout();
        for &n in &hidden {
            let mut style = self.style(n);
            style.display = Display::None;
            self.set_style(n, style);
        }
        for n in stand_ins {
            self.remove(n);
        }
    }

    fn compute_layout(&mut self) {
        let scale = self.scale;
        let available = Size {
            width: AvailableSpace::Definite(self.size.x),
            height: AvailableSpace::Definite(self.size.y),
        };
        let family = self.theme.font_family().to_owned();
        let fonts = self.fonts.get_mut();
        let _ =
            self.tree.compute_layout_with_measure(self.root, available, |input, _node, widget, _style| {
                let known = input.known_dimensions;
                let size = match widget.map(|w| &mut w.kind) {
                    Some(WidgetKind::Label(label)) => {
                        // Measure the natural size; the last layout may have squeezed the buffer
                        // (e.g. to 0 while hidden).
                        label.buffer.set_size(None, None);
                        label.buffer.shape_until_scroll(fonts, false);
                        let (mut w, mut h) = measure(&label.buffer);
                        if label.max_lines > 1 {
                            // A wrapping label takes the width it is given (or that is left) and
                            // grows taller instead of wider.
                            let limit = known.width.or(match input.available_space.width {
                                AvailableSpace::Definite(a) => Some(a),
                                _ => None,
                            });
                            ((w, h), _) = layout_wrapped(label, fonts, &family, limit.map(|l| l * scale));
                        }
                        Size {
                            width: known.width.unwrap_or(w / scale),
                            height: known.height.unwrap_or(h / scale),
                        }
                    }
                    Some(WidgetKind::TextField(field)) => {
                        let (_, h) = measure(&field.buffer);
                        Size { width: known.width.unwrap_or(0.0), height: known.height.unwrap_or(h / scale) }
                    }
                    Some(WidgetKind::Toggle { .. }) => Size {
                        width: known.width.unwrap_or(TOGGLE_SIZE.x),
                        height: known.height.unwrap_or(TOGGLE_SIZE.y),
                    },
                    Some(WidgetKind::Slider { .. }) => Size {
                        width: known.width.unwrap_or(0.0),
                        height: known.height.unwrap_or(SLIDER_KNOB + 4.0),
                    },
                    Some(WidgetKind::Icon { size, .. }) => {
                        Size { width: known.width.unwrap_or(*size), height: known.height.unwrap_or(*size) }
                    }
                    _ => Size { width: known.width.unwrap_or(0.0), height: known.height.unwrap_or(0.0) },
                };
                taffy::LayoutOutput::from_outer_size(size)
            });
    }

    /// Stores absolute frames and sizes text buffers to their final widths.
    fn update_frames(&mut self, node: NodeId, parent_origin: Vec2) {
        let Ok(layout) = self.tree.layout(node).copied() else { return };
        let origin = parent_origin + Vec2::new(layout.location.x, layout.location.y);
        let frame = Rect { origin, size: Vec2::new(layout.size.width, layout.size.height) };
        let scale = self.scale;
        let focused = self.focus == Some(node);
        if let Some(widget) = self.tree.get_node_context_mut(node) {
            widget.frame = frame;
            match &mut widget.kind {
                WidgetKind::Label(label) => {
                    // A little slack so rounding never ellipsizes a label that fits.
                    if label.max_lines > 1 {
                        let family = self.theme.font_family();
                        layout_wrapped(label, self.fonts.get_mut(), family, Some(frame.size.x * scale + 1.0));
                    }
                    let height = (label.max_lines <= 1).then_some(frame.size.y * scale);
                    label.buffer.set_size(Some(frame.size.x * scale + 1.0), height);
                    label.buffer.shape_until_scroll(self.fonts.get_mut(), false);
                }
                WidgetKind::TextField(field) => {
                    field.buffer.set_size(None, Some(frame.size.y * scale));
                    field.buffer.shape_until_scroll(self.fonts.get_mut(), false);
                    // Keep the caret in view.
                    let caret = caret_x(&field.buffer, field.display_index(field.editor.cursor())) / scale;
                    let width = frame.size.x;
                    if !focused || caret < field.scroll_x {
                        field.scroll_x = 0.0f32.max(caret - width).min(caret);
                    }
                    if caret > field.scroll_x + width - CARET_WIDTH {
                        field.scroll_x = caret - width + CARET_WIDTH;
                    }
                    if !focused || caret + CARET_WIDTH <= width {
                        // Unfocused, or the field grew until the caret fits again (the compact
                        // address expands while it is edited): back to the start.
                        field.scroll_x = 0.0;
                    }
                }
                _ => {}
            }
        }
        for child in self.tree.children(node).unwrap_or_default() {
            self.update_frames(child, origin);
        }
    }

    /// Absolute frame of `node` in logical pixels (after [`Ui::layout`]).
    pub fn frame(&self, node: NodeId) -> Rect {
        self.widget(node).map(Widget::frame).unwrap_or_default()
    }

    fn descendants(&self, node: NodeId) -> Vec<NodeId> {
        let mut out = vec![node];
        let mut i = 0;
        while i < out.len() {
            out.extend(self.tree.children(out[i]).unwrap_or_default());
            i += 1;
        }
        out
    }

    // ---- hit testing & input ----

    /// The topmost visible node under `p` that paints something or reacts to input.
    pub fn hit(&self, p: Vec2) -> Option<NodeId> {
        self.hit_in(self.root, p)
    }

    fn hit_in(&self, node: NodeId, p: Vec2) -> Option<NodeId> {
        if !self.is_visible(node) {
            return None;
        }
        let widget = self.widget(node)?;
        if widget.clip && !widget.frame.contains(p) {
            return None;
        }
        // A segmented control takes the pointer itself (its segments may carry actions).
        let children = if matches!(widget.kind, WidgetKind::Segmented { .. }) {
            Vec::new()
        } else {
            self.tree.children(node).unwrap_or_default()
        };
        for child in children.into_iter().rev() {
            if let Some(hit) = self.hit_in(child, p) {
                return Some(hit);
            }
        }
        let solid = !widget.join_only && widget.background != Background::None
            || widget.action.is_some()
            || matches!(
                widget.kind,
                WidgetKind::TextField(_)
                    | WidgetKind::Toggle { .. }
                    | WidgetKind::Slider { .. }
                    | WidgetKind::Segmented { .. }
            );
        (solid && node != self.root && rounded_contains(widget.frame, widget.radius, p)).then_some(node)
    }

    /// The nearest node at or above `node` with an action.
    fn actionable(&self, mut node: NodeId) -> Option<NodeId> {
        loop {
            let widget = self.widget(node)?;
            if widget.action.is_some() && widget.enabled {
                return Some(node);
            }
            node = self.tree.parent(node)?;
        }
    }

    /// The button being held down, if any.
    pub fn pressed_action(&self) -> Option<NodeId> {
        self.pressed
    }

    /// Ends whatever the pointer holds without a click: a slider/toggle drag (it settles where
    /// it is, springing back), a held button (its glass springs back), a text selection.
    /// For a button-up that never reached the UI: pointer outside the window, focus lost, Esc,
    /// the held control hidden or removed. Returns the events of the settled drag.
    pub fn end_interaction(&mut self) -> Vec<UiEvent> {
        let mut events = Vec::new();
        self.field_select = None;
        if let Some(node) = self.dragging.take() {
            events.extend(self.release_knob(node));
        }
        self.pressed = None;
        if let Some((held, _)) = self.press_anchor.take() {
            self.release_press(held);
        }
        events
    }

    /// Whether the pointer holds something (a drag or a pressed button).
    pub fn is_holding(&self) -> bool {
        self.dragging.is_some() || self.pressed.is_some() || self.press_anchor.is_some()
    }

    /// Lets go of the held button without a click (the press became a drag of it).
    /// The held glass (or lens) springs back.
    pub fn cancel_press(&mut self) {
        self.pressed = None;
        if let Some((held, _)) = self.press_anchor.take() {
            self.release_press(held);
        }
    }

    /// The button under the pointer, if any.
    pub fn hovered(&self) -> Option<NodeId> {
        self.hover
    }

    /// The hovered button with a tooltip: its node, its frame and the tooltip.
    pub fn hovered_tooltip(&self) -> Option<(NodeId, Rect, &Tooltip)> {
        let node = self.hover?;
        let widget = self.widget(node)?;
        Some((node, widget.frame, widget.tooltip.as_ref()?))
    }

    /// The clickable node at `p`, if any.
    pub fn hit_action(&self, p: Vec2) -> Option<NodeId> {
        self.hit(p).and_then(|n| self.actionable(n))
    }

    /// Whether the UI covers `p` (so pointer input there must not reach the page).
    pub fn covers(&self, p: Vec2) -> bool {
        self.hit(p).is_some()
    }

    pub fn cursor(&self) -> Option<UiCursor> {
        let node = self.hit(self.pointer?)?;
        let widget = self.widget(node)?;
        Some(if matches!(widget.kind, WidgetKind::TextField(_)) {
            UiCursor::Text
        } else if self.actionable(node).is_some()
            || matches!(
                widget.kind,
                WidgetKind::Toggle { .. } | WidgetKind::Slider { .. } | WidgetKind::Segmented { .. }
            )
        {
            UiCursor::Pointer
        } else {
            UiCursor::Default
        })
    }

    /// Feeds input in. Returns whether the UI consumed it, and what happened.
    pub fn handle(&mut self, input: UiInput) -> (bool, Vec<UiEvent>) {
        let mut events = Vec::new();
        let consumed = match input {
            UiInput::PointerMove(p) => {
                self.pointer = Some(p);
                if let Some(node) = self.dragging {
                    if self.is_toggle(node) || self.is_segmented(node) {
                        let travel = if self.is_toggle(node) {
                            self.toggle_travel(node)
                        } else {
                            self.segment_travel(node)
                        };
                        if let Some(knob) = self.knobs.get_mut(&node) {
                            knob.drag_to(p.x, travel);
                        }
                    } else {
                        events.extend(self.slide_to(node, p));
                    }
                    return (true, events);
                }
                if let Some((node, word)) = self.field_select
                    && let Some(index) = self.field_index_at(node, p)
                {
                    match word {
                        Some((start, end)) => self.edit_field(node, |e| e.extend_by_words(start, end, index)),
                        None => self.edit_field(node, |e| e.move_to(index, true)),
                    }
                    return (true, events);
                }
                self.pull_glass(p);
                let hit = self.hit(p);
                self.hover = hit.and_then(|n| self.actionable(n));
                hit.is_some() || self.pressed.is_some()
            }
            UiInput::PointerLeave => {
                self.pointer = None;
                self.hover = None;
                false
            }
            UiInput::PointerDown => self.pointer_down(&mut events),
            UiInput::PointerUp => {
                self.field_select = None;
                if let Some(node) = self.dragging.take() {
                    events.extend(self.release_knob(node));
                    return (true, events);
                }
                let pressed = self.pressed.take();
                if let Some((held, _)) = self.press_anchor.take() {
                    self.release_press(held);
                }
                if let (Some(node), Some(p)) = (pressed, self.pointer)
                    && self.hit(p).and_then(|n| self.actionable(n)) == Some(node)
                    && let Some(action) = self.widget(node).and_then(|w| w.action)
                {
                    events.push(UiEvent::Clicked { action, node });
                }
                pressed.is_some() || self.pointer.is_some_and(|p| self.covers(p))
            }
            UiInput::Key { key, modifiers } => match self.focus {
                Some(node) => {
                    self.field_key(node, &key, modifiers, &mut events);
                    true
                }
                None => false,
            },
            UiInput::Text(text) => match self.focus {
                Some(node) => {
                    self.edit_field(node, |e| e.insert(&text));
                    true
                }
                None => false,
            },
        };
        (consumed, events)
    }

    fn pointer_down(&mut self, events: &mut Vec<UiEvent>) -> bool {
        let Some(p) = self.pointer else { return false };
        let Some(hit) = self.hit(p) else {
            // Clicking the page takes focus away from the UI.
            events.extend(self.blur());
            return false;
        };
        match self.widget(hit).map(|w| &w.kind) {
            Some(WidgetKind::Toggle { on }) => {
                // Decided on release: a tap flips it, a drag puts it where the knob is let go.
                let pos = if *on { 1.0 } else { 0.0 };
                events.extend(self.blur());
                self.knob(hit, pos).press(p.x);
                self.dragging = Some(hit);
                return true;
            }
            Some(WidgetKind::Segmented { selected }) => {
                let pos = self.segment_pos(hit, *selected);
                events.extend(self.blur());
                self.knob(hit, pos).press(p.x);
                self.dragging = Some(hit);
                return true;
            }
            Some(WidgetKind::Slider { value, .. }) => {
                let value = *value;
                events.extend(self.blur());
                self.knob(hit, value).press(p.x);
                self.dragging = Some(hit);
                events.extend(self.slide_to(hit, p));
                return true;
            }
            _ => {}
        }
        let is_field = self.widget(hit).is_some_and(Widget::is_focusable);
        if is_field {
            if self.focus == Some(hit) {
                // One click places the caret (and dragging selects), two select a word, three
                // everything.
                let now = Instant::now();
                let count = match self.last_click {
                    Some((at, q, n))
                        if now.duration_since(at) < MULTI_CLICK_TIME
                            && q.distance(p) < MULTI_CLICK_DISTANCE =>
                    {
                        n % 3 + 1
                    }
                    _ => 1,
                };
                self.last_click = Some((now, p, count));
                if let Some(index) = self.field_index_at(hit, p) {
                    match count {
                        1 => {
                            self.edit_field(hit, |e| e.move_to(index, false));
                            self.field_select = Some((hit, None));
                        }
                        2 => {
                            self.edit_field(hit, |e| e.select_word_at(index));
                            let word = self.field_editor(hit).map(|e| e.word_at(index));
                            self.field_select = word.map(|w| (hit, Some(w)));
                        }
                        _ => self.edit_field(hit, Editor::select_all),
                    }
                }
            } else {
                events.extend(self.focus(hit));
            }
        } else {
            events.extend(self.blur());
        }
        self.pressed = self.actionable(hit);
        // A field takes the press for its caret and selection: its glass stays put.
        if let Some((held, lens)) = self.pressed.filter(|_| !is_field).and_then(|n| self.press_target(n)) {
            self.press_glass(held, lens);
            self.press_anchor = Some((held, p));
        }
        true
    }

    fn field_editor(&self, node: NodeId) -> Option<&Editor> {
        match &self.widget(node)?.kind {
            WidgetKind::TextField(field) => Some(&field.editor),
            _ => None,
        }
    }

    /// The text index under `p` (logical px) in text field `node`.
    fn field_index_at(&self, node: NodeId, p: Vec2) -> Option<usize> {
        let Some(Widget { kind: WidgetKind::TextField(field), frame, .. }) = self.widget(node) else {
            return None;
        };
        let x = (p.x - frame.origin.x + field.scroll_x) * self.scale;
        Some(field.real_index(index_at(&field.buffer, x)))
    }

    fn is_toggle(&self, node: NodeId) -> bool {
        matches!(self.widget(node).map(|w| &w.kind), Some(WidgetKind::Toggle { .. }))
    }

    /// The knob state of a switch or slider, created at `pos` if it isn't animating yet.
    fn knob(&mut self, node: NodeId, pos: f32) -> &mut Knob {
        let spring = self.knob_spring();
        self.knobs.entry(node).or_insert_with(|| Knob::new(pos, spring))
    }

    fn knob_spring(&self) -> Spring {
        let motion = &self.theme.config().motion;
        if self.theme.config().glass.reduce_motion {
            Spring::new(0.15, 1.0)
        } else {
            Spring::new(
                motion.liquid.response * motion.speed,
                self.theme.config().glass.damping(motion.liquid.damping),
            )
        }
    }

    /// (track size, knob size) of a switch, logical px.
    fn toggle_geometry() -> (Vec2, Vec2) {
        let knob_h = TOGGLE_SIZE.y - 2.0 * TOGGLE_INSET;
        (TOGGLE_SIZE, Vec2::new(knob_h * TOGGLE_KNOB_ASPECT, knob_h))
    }

    fn toggle_travel(&self, _node: NodeId) -> f32 {
        let (track, knob) = Self::toggle_geometry();
        track.x - 2.0 * TOGGLE_INSET - knob.x
    }

    fn is_segmented(&self, node: NodeId) -> bool {
        matches!(self.widget(node).map(|w| &w.kind), Some(WidgetKind::Segmented { .. }))
    }

    /// The knob position (0–1) of segment `index` of a segmented control.
    fn segment_pos(&self, node: NodeId, index: usize) -> f32 {
        let count = self.tree.children(node).map_or(1, |c| c.len());
        if count < 2 { 0.0 } else { index.min(count - 1) as f32 / (count - 1) as f32 }
    }

    /// Distance between the first and the last segment's centres (logical px).
    fn segment_travel(&self, node: NodeId) -> f32 {
        let children = self.tree.children(node).unwrap_or_default();
        match (children.first(), children.last()) {
            (Some(&a), Some(&b)) => (self.frame(b).center().x - self.frame(a).center().x).abs(),
            _ => 1.0,
        }
    }

    /// The selection pill of a segmented control at knob position `pos` (logical px):
    /// between two segments it takes the in-between position and width.
    fn segment_pill(&self, node: NodeId, pos: f32) -> Option<Rect> {
        let children = self.tree.children(node).unwrap_or_default();
        let n = children.len();
        if n == 0 {
            return None;
        }
        let f = pos.clamp(0.0, 1.0) * (n - 1) as f32;
        let i = (f.floor() as usize).min(n - 1);
        let j = (i + 1).min(n - 1);
        let t = f - i as f32;
        let (a, b) = (self.frame(children[i]), self.frame(children[j]));
        let width = a.size.x + (b.size.x - a.size.x) * t;
        let mut centre = a.center() + (b.center() - a.center()) * t;
        // Pulled past an end: the pill follows a little (the knob is rubber-banded).
        if pos < 0.0 || pos > 1.0 {
            let over = if pos < 0.0 { pos } else { pos - 1.0 };
            centre.x += over * self.segment_travel(node).max(width);
        }
        Some(Rect { origin: centre - Vec2::new(width, a.size.y) / 2.0, size: Vec2::new(width, a.size.y) })
    }

    /// Lets go of a held segmented control: a drag snaps to the nearest segment, a tap selects
    /// the segment under the pointer.
    fn release_segmented(&mut self, node: NodeId) -> Option<UiEvent> {
        let selected = match self.widget(node).map(|w| &w.kind) {
            Some(WidgetKind::Segmented { selected }) => *selected,
            _ => return None,
        };
        let children = self.tree.children(node).unwrap_or_default();
        let n = children.len().max(1);
        let tapped = self.pointer.and_then(|p| children.iter().position(|&c| self.frame(c).contains(p)));
        let knob = self.knobs.get_mut(&node)?;
        let dragged_to = knob.pos.value;
        let moved = knob.release(0.0);
        let index = if moved {
            ((dragged_to.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize).min(n - 1)
        } else {
            tapped.unwrap_or(selected)
        };
        let target = if n < 2 { 0.0 } else { index as f32 / (n - 1) as f32 };
        if let Some(knob) = self.knobs.get_mut(&node) {
            knob.pos.target = target;
        }
        if index == selected {
            return None;
        }
        if let Some(WidgetKind::Segmented { selected }) =
            self.tree.get_node_context_mut(node).map(|w| &mut w.kind)
        {
            *selected = index;
        }
        // A segment with an action reports that (settings rows); else the new index.
        match children.get(index).and_then(|&c| self.widget(c).and_then(|w| w.action.map(|a| (a, c)))) {
            Some((action, child)) => Some(UiEvent::Clicked { action, node: child }),
            None => Some(UiEvent::ValueChanged { node, value: index as f32 }),
        }
    }

    /// Lets go of a held switch or slider.
    fn release_knob(&mut self, node: NodeId) -> Option<UiEvent> {
        if self.is_segmented(node) {
            return self.release_segmented(node);
        }
        let on = match self.widget(node).map(|w| &w.kind) {
            Some(WidgetKind::Toggle { on }) => Some(*on),
            Some(WidgetKind::Slider { value, .. }) => {
                let value = *value;
                if let Some(knob) = self.knobs.get_mut(&node) {
                    knob.release(value);
                }
                return None;
            }
            _ => None,
        }?;
        let knob = self.knobs.get_mut(&node)?;
        let dragged_to = knob.pos.value;
        let moved = knob.release(0.0);
        let now_on = if moved { dragged_to > 0.5 } else { !on };
        knob.pos.target = if now_on { 1.0 } else { 0.0 };
        if now_on == on {
            return None;
        }
        let value = if now_on { 1.0 } else { 0.0 };
        if let Some(WidgetKind::Toggle { on }) = self.tree.get_node_context_mut(node).map(|w| &mut w.kind) {
            *on = now_on;
        }
        Some(UiEvent::ValueChanged { node, value })
    }

    /// Moves a slider's knob under the pointer.
    fn slide_to(&mut self, slider: NodeId, p: Vec2) -> Option<UiEvent> {
        let frame = self.frame(slider);
        let line =
            matches!(self.widget(slider).map(|w| &w.kind), Some(WidgetKind::Slider { line: true, .. }));
        let inset = if line { 0.0 } else { SLIDER_KNOB / 2.0 };
        let span = (frame.size.x - 2.0 * inset).max(1.0);
        let value = ((p.x - frame.origin.x - inset) / span).clamp(0.0, 1.0);
        match self.widget(slider).map(|w| &w.kind) {
            Some(WidgetKind::Slider { value: v, .. }) if (*v - value).abs() > f32::EPSILON => {
                self.set_value(slider, value);
                Some(UiEvent::ValueChanged { node: slider, value })
            }
            _ => None,
        }
    }

    /// Sets a toggle (> 0.5 is on) or slider (0–1) without emitting an event.
    /// Swaps an icon widget's glyph.
    pub fn set_icon(&mut self, node: NodeId, icon: axion_compositor::Icon) {
        if let Some(WidgetKind::Icon { icon: i, .. }) =
            self.tree.get_node_context_mut(node).map(|w| &mut w.kind)
        {
            *i = icon;
        }
    }

    /// Resizes an icon widget's glyph (logical px).
    pub fn set_icon_size(&mut self, node: NodeId, side: f32) {
        if let Some(WidgetKind::Icon { size, .. }) = self.tree.get_node_context_mut(node).map(|w| &mut w.kind)
            && (*size - side).abs() > f32::EPSILON
        {
            *size = side;
            let _ = self.tree.mark_dirty(node);
            self.needs_layout = true;
        }
    }

    /// The slider being dragged, if any.
    pub fn dragging(&self) -> Option<NodeId> {
        self.dragging
    }

    pub fn set_value(&mut self, node: NodeId, value: f32) {
        let target = if self.is_toggle(node) {
            (value > 0.5) as u8 as f32
        } else if self.is_segmented(node) {
            self.segment_pos(node, value.max(0.0).round() as usize)
        } else {
            value.clamp(0.0, 1.0)
        };
        if let Some(knob) = self.knobs.get_mut(&node) {
            knob.set(target);
        }
        match self.tree.get_node_context_mut(node).map(|w| &mut w.kind) {
            Some(WidgetKind::Toggle { on }) => *on = value > 0.5,
            Some(WidgetKind::Segmented { selected }) => *selected = value.max(0.0).round() as usize,
            Some(WidgetKind::Slider { value: v, .. }) => *v = value.clamp(0.0, 1.0),
            _ => {}
        }
    }

    fn field_key(&mut self, node: NodeId, key: &Key, m: Modifiers, events: &mut Vec<UiEvent>) {
        let word = m.ctrl;
        match key {
            Key::Named(name) => match name.as_str() {
                "Left" => self.edit_field(node, |e| e.move_left(m.shift, word)),
                "Right" => self.edit_field(node, |e| e.move_right(m.shift, word)),
                "Home" => self.edit_field(node, |e| e.home(m.shift)),
                "End" => self.edit_field(node, |e| e.end(m.shift)),
                "Backspace" if word => self.edit_field(node, Editor::backspace_word),
                "Backspace" => self.edit_field(node, Editor::backspace),
                "Delete" => self.edit_field(node, Editor::delete),
                "Enter" => {
                    let text = self.field_text(node).unwrap_or_default().to_owned();
                    events.push(UiEvent::Submitted { node, text });
                }
                "Escape" => {
                    events.push(UiEvent::Cancelled { node });
                    events.extend(self.blur());
                }
                _ => {}
            },
            Key::Char(c) if m.ctrl && !m.alt => {
                let selected = |ui: &Self| match ui.widget(node).map(|w| &w.kind) {
                    // Passwords never leave the field.
                    Some(WidgetKind::TextField(f)) if !f.secure => {
                        f.editor.selected_text().map(str::to_owned)
                    }
                    _ => None,
                };
                match c {
                    'a' => self.edit_field(node, Editor::select_all),
                    'c' => events.extend(selected(self).map(UiEvent::Copy)),
                    'x' => {
                        if let Some(text) = selected(self) {
                            events.push(UiEvent::Copy(text));
                            self.edit_field(node, Editor::backspace);
                        }
                    }
                    'v' => events.push(UiEvent::PasteRequested { node }),
                    _ => {}
                }
            }
            Key::Char(_) => {}
        }
    }

    /// The nearest glass at or above `node` (the surface a button sits on).
    fn glass_of(&self, mut node: NodeId) -> Option<NodeId> {
        loop {
            if matches!(self.widget(node)?.background, Background::Glass(_) | Background::TintedGlass(..)) {
                return Some(node);
            }
            node = self.tree.parent(node)?;
        }
    }

    /// What swells while `button` is held, and whether it is a lens: a button without a
    /// background lying on glass, or a small glass button, becomes a lens (its content rides
    /// along with it); anything else swells the glass it lies on as a group.
    /// Glass that is one element (`Widget::press_whole`) becomes one lens as a whole,
    /// whichever of its buttons is pressed.
    fn press_target(&self, button: NodeId) -> Option<(NodeId, bool)> {
        // Flat mode: nothing swells or becomes a lens; the button's own tint is the feedback.
        if self.theme.flat() {
            return None;
        }
        let widget = self.widget(button)?;
        if widget.feedback
            && let Some(whole) =
                self.glass_of(button).filter(|&g| self.widget(g).is_some_and(|w| w.press_whole))
        {
            return Some((whole, true));
        }
        match widget.background {
            Background::None if widget.feedback => self.glass_of(button).map(|_| (button, true)),
            Background::Glass(_) | Background::TintedGlass(..)
                if widget.feedback && widget.frame.size.max_element() <= LENS_BUTTON_MAX =>
            {
                Some((button, true))
            }
            _ => self.glass_of(button).map(|glass| (glass, false)),
        }
    }

    /// Starts the swell of held glass, or of a pressed button's lens.
    fn press_glass(&mut self, node: NodeId, lens: bool) {
        let frame = self.frame(node);
        let size = frame.size;
        let mut pull = Vec2::splat(PULL_LIMIT);
        let swell = if lens {
            let mut grow = LENS_PRESS_GROW_PX;
            // A row nearly as long as the glass it lies on (a menu row) stays inside it along
            // its length, where it neither swells past nor follows the pointer out of it.
            let long = if size.x >= size.y { 0 } else { 1 };
            if size[long] >= LENS_ROW_ASPECT * size[1 - long]
                && let Some(glass) = self.tree.parent(node).and_then(|p| self.glass_of(p))
            {
                let g = self.frame(glass);
                let (lo, hi) = (
                    frame.origin[long] - g.origin[long],
                    (g.origin + g.size)[long] - (frame.origin + frame.size)[long],
                );
                grow = grow.min(lo.min(hi).max(0.0));
                pull[long] = 0.0;
            }
            (grow / (size.max_element() / 2.0).max(1.0)).min(LENS_PRESS_SCALE - 1.0)
        } else {
            // Only surfaces big in both directions (sheets, the sidebar) stay still; bars swell.
            if size.min_element() > PRESS_MAX_SIZE {
                return;
            }
            (PRESS_GROW_PX / (size.max_element() / 2.0).max(1.0)).min(PRESS_SCALE - 1.0)
        };
        let spring = self.press_spring();
        let press = self.press_scale.entry(node).or_insert_with(|| Press {
            scale: Animated::new(1.0, spring),
            offset: Animated::new(Vec2::ZERO, spring),
            lens,
            held: Animated::new(0.0, spring),
            pull,
            distance: Animated::new(0.0, spring),
            dir: Vec2::X,
        });
        press.scale.spring = spring;
        press.offset.spring = spring;
        press.distance.spring = spring;
        press.lens = lens;
        press.held.spring = spring;
        press.held.target = 1.0;
        press.pull = pull;
        press.scale.target = 1.0 + swell;
    }

    /// Lets held glass (or a lens) spring back.
    fn release_press(&mut self, node: NodeId) {
        if let Some(press) = self.press_scale.get_mut(&node) {
            press.scale.target = 1.0;
            press.offset.target = Vec2::ZERO;
            press.held.target = 0.0;
            press.distance.target = 0.0;
        }
    }

    /// While glass is held, it follows the pointer a little (rubber band) when dragged away.
    fn pull_glass(&mut self, p: Vec2) {
        let Some((glass, anchor)) = self.press_anchor else { return };
        // Reduce Motion: it swells in place.
        let still = self.theme.config().glass.reduce_motion;
        if let Some(press) = self.press_scale.get_mut(&glass) {
            let d = p - anchor;
            // The stretch follows how far the pointer is from where it went down (only along
            // the axes the glass may be pulled on), however little the glass itself follows.
            let free = Vec2::new(f32::from(press.pull.x > 0.0), f32::from(press.pull.y > 0.0));
            let reach = d * free;
            if reach.length() > 0.0 {
                press.dir = reach / reach.length();
            }
            press.distance.target = if still { 0.0 } else { reach.length() };
            press.offset.target = if still {
                Vec2::ZERO
            } else if press.pull.x == press.pull.y {
                axion_physics::rubber_band_vec(d, press.pull.x)
            } else {
                Vec2::new(
                    axion_physics::rubber_band(d.x, press.pull.x),
                    axion_physics::rubber_band(d.y, press.pull.y),
                )
            };
        }
    }

    /// The innermost visible glass under `p` in `within`'s subtree, leaving out `skip` (and
    /// what lies inside them): its frame as drawn (logical px), corner radius (clamped to the
    /// frame) and tint density. Popovers use it to find the glass they flow out of.
    pub fn glass_at(
        &self,
        within: NodeId,
        p: Vec2,
        skip: &[NodeId],
    ) -> Option<(Rect, f32, Option<GlassLevel>)> {
        if skip.contains(&within) || !self.is_visible(within) {
            return None;
        }
        let widget = self.widget(within)?;
        for child in self.tree.children(within).unwrap_or_default().into_iter().rev() {
            if let Some(found) = self.glass_at(child, p, skip) {
                return Some(found);
            }
        }
        // Glass that only joins other glass (a popover's bridge) is not a surface of its own.
        if widget.join_only
            || !matches!(widget.background, Background::Glass(_) | Background::TintedGlass(..))
        {
            return None;
        }
        let frame = widget.visual_frame.unwrap_or(widget.frame);
        let radius = widget.radius.min(frame.size.min_element() / 2.0);
        rounded_contains(frame, radius, p).then_some((frame, radius, widget.glass_density))
    }

    fn press_spring(&self) -> Spring {
        let motion = &self.theme.config().motion;
        if self.theme.config().glass.reduce_motion {
            Spring::new(0.15, 1.0)
        } else {
            Spring::new(
                motion.snappy.response * motion.speed,
                self.theme.config().glass.damping(motion.liquid.damping),
            )
        }
    }

    /// Advances UI animations (glass press feedback). Call once per frame.
    pub fn animate(&mut self, dt: f32) {
        self.press_scale.retain(|_, press| {
            press.scale.step(dt);
            press.offset.step(dt);
            press.held.step(dt);
            press.distance.step(dt);
            !(press.scale.target == 1.0
                && press.scale.is_settled(PRESS_SETTLED)
                && press.held.target == 0.0
                && press.held.is_settled(PRESS_SETTLED)
                && press.offset.target == Vec2::ZERO
                && press.offset.is_settled(0.05)
                && press.distance.target == 0.0
                && press.distance.is_settled(0.05))
        });
        // A slider's knob sits where its value is; only the lens animates.
        let sliders: Vec<(NodeId, f32)> = self
            .knobs
            .keys()
            .filter_map(|&n| match self.widget(n).map(|w| &w.kind) {
                Some(WidgetKind::Slider { value, .. }) => Some((n, *value)),
                _ => None,
            })
            .collect();
        for (node, value) in sliders {
            if let Some(knob) = self.knobs.get_mut(&node) {
                knob.pos.snap(value);
            }
        }
        let tree = &self.tree;
        self.knobs.retain(|node, knob| {
            knob.step(dt);
            tree.get_node_context(*node).is_some() && !knob.is_settled()
        });
    }

    // ---- painting ----

    /// Appends this UI's layers (content and glass, in stacking order) to `layers`.
    pub fn paint<'a>(&'a self, layers: &mut Vec<Layer<'a>>) {
        // Each child of the root is its own plane (sidebar, bar, overlays…), painted completely
        // before the next one, so later planes cover earlier ones — including their text.
        for plane in self.tree.children(self.root).unwrap_or_default() {
            let mut stacks: Vec<Stack<'a>> = Vec::new();
            self.painting_dark.set(self.dark_planes.contains(&plane));
            self.paint_node(plane, 0, 1.0, 1.0, &mut stacks);
            self.flush(stacks, layers);
            self.painting_dark.set(false);
        }
    }

    fn flush<'a>(&'a self, stacks: Vec<Stack<'a>>, layers: &mut Vec<Layer<'a>>) {
        let s = self.scale;
        // Stack k holds the glass that has k-1 glass ancestors, and the content sitting on it.
        // Glass is drawn in one pass per (thickness, opacity) group; only shapes in the same
        // pass flow into each other.
        for stack in stacks {
            let mut groups: Vec<(GlassLevel, f32, Option<[f32; 4]>)> = Vec::new();
            for item in &stack.glass {
                let key = (item.level, item.fade, item.tint);
                if !groups.contains(&key) {
                    groups.push(key);
                }
            }
            groups.sort_by_key(|(level, fade, tint)| {
                (*level as u8, (fade * GLASS_FADE_STEPS) as u32, tint.is_some())
            });
            for (level, fade, tint) in groups {
                let shapes: Vec<GlassShape> = stack
                    .glass
                    .iter()
                    .filter(|g| g.level == level && g.fade == fade && g.tint == tint)
                    .map(|g| {
                        let mut shape = g.shape;
                        if let Some(opacity) = &mut shape.tint_opacity {
                            *opacity *= fade;
                        }
                        shape
                    })
                    .collect();
                let mut params = self.paint_theme().glass(level, s);
                if let Some(tint) = tint {
                    let theme = self.paint_theme();
                    let glass = theme.tinted_glass(tint).0;
                    params.tint = [glass[0], glass[1], glass[2], theme.tinted_opacity()];
                    // Tinted glass is a button: neighbours stay separate drops (no blob between two).
                    params.merge_smoothness = 0.0;
                    params.rim *= theme.tinted_rim();
                }
                if fade < 1.0 {
                    // Fading glass loses blur, tint, refraction and light together.
                    params.blur_radius *= fade;
                    params.tint[3] *= fade;
                    params.refraction *= fade;
                    params.dispersion *= fade;
                    params.rim *= fade;
                    params.shadow *= fade;
                }
                layers.push(Layer::Glass { shapes, params });
            }
            if !stack.content.is_empty() {
                layers.push(Layer::Content(stack.content));
            }
            layers.extend(stack.after);
        }
    }

    /// `depth` = number of glass ancestors. A glass node goes to stack `depth + 1`, where its
    /// content is drawn on top of it.
    fn paint_node<'a>(
        &'a self,
        node: NodeId,
        depth: usize,
        inherited_alpha: f32,
        inherited_fade: f32,
        stacks: &mut Vec<Stack<'a>>,
    ) {
        if !self.is_visible(node) {
            return;
        }
        let Some(widget) = self.widget(node) else { return };
        // `fade` (opacity) applies to everything; `alpha` additionally dims disabled content.
        let fade = inherited_fade * widget.opacity;
        if fade <= MIN_VISIBLE {
            return;
        }
        while stacks.len() <= depth + 1 {
            stacks.push(Stack::default());
        }
        let s = self.scale;
        let theme = self.paint_theme();
        let frame = widget.frame;
        let px = Rect { origin: frame.origin * s, size: frame.size * s };
        let radius = widget.radius.min(frame.size.min_element() / 2.0) * s;
        // The background may be animated separately from the layout (see `visual_frame`).
        let (bg_px, bg_radius) = match widget.visual_frame {
            Some(v) => (
                Rect { origin: v.origin * s, size: v.size * s },
                widget.radius.min(v.size.min_element() / 2.0) * s,
            ),
            None => (px, radius),
        };
        // Disabled widgets dim themselves and everything inside them.
        let dim = inherited_alpha * if widget.enabled { 1.0 } else { DISABLED_ALPHA };
        let alpha = dim * fade;
        let faded = |mut c: [f32; 4]| {
            c[3] *= fade;
            c
        };

        // Held glass swells and leans toward the pointer; its content moves with it.
        let flat = theme.flat();
        let press_xf = self.press_scale.get(&node).filter(|_| !flat).map(|press| {
            let offset = press.offset.value * s;
            // The stretch grows with the pointer's distance but resists more and more; its
            // limit is what a pull used to reach at most (and never more than the glass's
            // size allows: wide bars barely stretch).
            let glass = &self.theme.config().glass;
            let limit = glass
                .drag_stretch_max()
                .min(PULL_STRETCH * PULL_LIMIT * s / bg_px.size.max_element().max(1.0));
            let e = axion_physics::drag_stretch(press.distance.value, limit, glass.drag_resistance());
            let (angle, content) = stretch_content(press.dir, e);
            // The icons and labels stay at the glass's rest centre and are deformed with the
            // same stretch as the glass (never carried off with the pointer): `carry` maps
            // the centre onto itself and `content_scale` is the shared transform.
            let carry = press.lens.then(|| Carry { from: px.center(), to: px.center() });
            PressXf {
                scale: press.scale.value,
                offset,
                stretch: Vec2::new(1.0 + e, 1.0 / (1.0 + e)),
                angle,
                content_scale: press.scale.value * content,
                carry,
            }
        });
        // How far a lens has formed (0 at rest, 1 held): it lightens with it.
        // Reduce Transparency: opaque glass has no clearer lens to show, so a pressed button
        // keeps its plain highlight (the lens still swells).
        let opaque = self.theme.config().glass.reduce_transparency || flat;
        let lens_glow = self.press_scale.get(&node).filter(|p| p.lens).map(|press| {
            let t = press.held.value.clamp(0.0, 1.0);
            if opaque { 0.0 } else { LENS_PRESS_GLOW[usize::from(theme.dark)] * t }
        });
        // A lens of a button without glass of its own is drawn in the pass of the glass it
        // lies on, so it flows out of it; inside a clipped list it would be a pass of its own,
        // so there the button keeps its plain highlight.
        let lens_on = match widget.background {
            Background::None if lens_glow.is_some() && self.painting_clipped.get() == 0 => {
                self.tree.parent(node).and_then(|p| self.glass_of(p)).and_then(|g| self.widget(g))
            }
            _ => None,
        };
        let lensed = lens_glow.is_some()
            && (lens_on.is_some()
                || matches!(widget.background, Background::Glass(_) | Background::TintedGlass(..)));
        let press_xf = press_xf.filter(|_| lensed || lens_glow.is_none());
        let mark = press_xf.as_ref().map(|_| marks(stacks));
        // An animated background carries its content along (mapped from the layout frame onto
        // it), and the content fades with the background's growth: both share one value.
        let (frame_xf, reveal) = match widget.visual_frame {
            Some(_) if px.size.min_element() > 0.0 => {
                let ratio = bg_px.size / px.size;
                let xf = PressXf {
                    scale: 1.0,
                    offset: Vec2::ZERO,
                    stretch: Vec2::ONE,
                    angle: 0.0,
                    content_scale: ratio,
                    carry: Some(Carry { from: px.center(), to: bg_px.center() }),
                };
                let t = ((ratio.min_element() - CONTENT_REVEAL_FROM)
                    / (CONTENT_REVEAL_TO - CONTENT_REVEAL_FROM))
                    .clamp(0.0, 1.0);
                (Some(xf), t * t * (3.0 - 2.0 * t))
            }
            _ => (None, 1.0),
        };
        let frame_mark = frame_xf.as_ref().map(|_| marks(stacks));
        let child_fade = fade * reveal;
        let mut child_depth = depth;
        let content = &mut stacks[depth].content;
        // Glass in a clipped subtree (a scrolled settings page) cannot be cut to the clip: where
        // it is not wholly inside it, it is a flat card (clipped like any content) or, wholly
        // outside, nothing.
        let glass_clipped = self.clip_stack.borrow().last().is_some_and(|c| !contains_rect(*c, bg_px));
        let glass_unseen = self.clip_stack.borrow().last().is_some_and(|c| {
            let hit = clip(bg_px, *c);
            hit.size.x <= 0.0 || hit.size.y <= 0.0
        });
        match widget.background {
            Background::Glass(_) | Background::TintedGlass(..) if glass_unseen => {
                child_depth = depth + 1;
            }
            Background::Glass(_) | Background::TintedGlass(..) if flat || glass_clipped => {
                // No glass: the same surface as a plain card (see `Theme::flat_paint`).
                // A bare group draws nothing and is no surface for what lies in it.
                if widget.flat_role == FlatRole::Bare {
                    child_depth = depth;
                } else {
                    self.paint_flat_card(widget, theme, bg_px, bg_radius, fade, depth, stacks);
                    child_depth = depth + 1;
                }
            }
            Background::Glass(_) | Background::TintedGlass(..) => {
                let (level, tint) = match widget.background {
                    Background::TintedGlass(level, paint) => (level, Some(theme.color(paint))),
                    Background::Glass(level) => (level, None),
                    _ => unreachable!("matched above"),
                };
                let mut shape = GlassShape::rounded_rect(bg_px.center(), bg_px.size * 0.5, bg_radius);
                shape.tint_opacity = glass_tint_opacity(widget, theme);
                shape.merge = widget.glass_merge.map(|m| m * s);
                shape.join_only = widget.join_only;
                if let Some(xf) = &press_xf {
                    shape.center += xf.offset;
                    shape.half_size *= xf.scale;
                    shape.corner_radius *= xf.scale;
                    shape.stretch = xf.stretch;
                    shape.stretch_angle = xf.angle;
                }
                shape.glow = lens_glow.unwrap_or(0.0);
                stacks[depth + 1].glass.push(GlassItem { level, fade: quantize_fade(fade), tint, shape });
                child_depth = depth + 1;
            }
            // A soft plate only helps text over glass; a flat card is solid already.
            Background::Soft(..) if flat => {}
            Background::Soft(paint, opacity, blur) => {
                let mut color = theme.color(paint);
                color[3] *= opacity * fade;
                content.quads.push(Quad { softness: blur * s, ..Quad::fill(bg_px, bg_radius, color) });
            }
            Background::Fill(paint, opacity) => {
                if let Some(shadow) = widget.shadow {
                    let (dy, blur, opacity) =
                        theme.flat_shadow_if_flat(shadow.offset_y, shadow.blur, shadow.opacity);
                    content.quads.push(Quad::shadow(
                        bg_px,
                        bg_radius,
                        dy * s,
                        blur * s,
                        [0.0, 0.0, 0.0, opacity * fade],
                    ));
                }
                let mut color = theme.color(paint);
                color[3] *= opacity * fade;
                content.quads.push(Quad::fill(bg_px, bg_radius, color));
            }
            Background::None => {
                if let (Some(glass), Some(xf)) = (lens_on, &press_xf)
                    && depth > 0
                {
                    let (level, tint) = match glass.background {
                        Background::TintedGlass(level, paint) => (level, Some(theme.color(paint))),
                        Background::Glass(level) => (level, None),
                        _ => unreachable!("glass_of returns glass"),
                    };
                    let mut shape = GlassShape::rounded_rect(
                        bg_px.center() + xf.offset,
                        bg_px.size * 0.5 * xf.scale,
                        bg_radius * xf.scale,
                    );
                    shape.stretch = xf.stretch;
                    shape.stretch_angle = xf.angle;
                    shape.tint_opacity = glass_tint_opacity(glass, theme);
                    shape.glow = lens_glow.unwrap_or(0.0);
                    stacks[depth].glass.push(GlassItem { level, fade: quantize_fade(fade), tint, shape });
                }
            }
        }

        // Buttons: selected / hover / pressed feedback.
        let content = &mut stacks[child_depth].content;
        if widget.action.is_some() && widget.enabled {
            let label = theme.color(Paint::Label);
            if widget.selected {
                let surface = theme.color(Paint::Surface);
                content.quads.push(Quad::shadow(px, radius, 1.0 * s, 4.0 * s, [0.0, 0.0, 0.0, 0.08 * fade]));
                content.quads.push(Quad::fill(px, radius, faded([surface[0], surface[1], surface[2], 0.7])));
            }
            let feedback = if !widget.feedback {
                0.0
            } else if lensed && !opaque {
                // The button is glass: its lens is the feedback.
                0.0
            } else if !opaque
                && self.pressed == Some(node)
                && self.press_target(node).is_some_and(|(glass, lens)| lens && glass != node)
            {
                // A button in glass that is pressed as a whole: the whole lens is the feedback.
                0.0
            } else if self.pressed == Some(node) {
                PRESS_ALPHA
            } else if self.hover == Some(node) && self.pressed.is_none() && widget.hover_fill {
                // While a button is held, others under the pointer don't light up.
                HOVER_ALPHA
            } else {
                0.0
            };
            if feedback > 0.0 {
                content.quads.push(Quad::fill(px, radius, faded([label[0], label[1], label[2], feedback])));
            }
        }

        match &widget.kind {
            WidgetKind::Container => {}
            WidgetKind::Label(label) => {
                let mut color = theme.color(label.paint);
                color[3] *= alpha;
                let (_, h) = measure(&label.buffer);
                let top = px.origin.y + ((px.size.y - h) / 2.0).max(0.0);
                content.text.push(text_area(&label.buffer, px.origin.x, top, px, color));
            }
            WidgetKind::Icon { icon, weight, size, paint } => {
                let mut color = theme.color(*paint);
                color[3] *= alpha;
                let side = size * s;
                let origin = (px.center() - Vec2::splat(side / 2.0)).round();
                content.icons.push(IconDraw {
                    icon: *icon,
                    weight: *weight,
                    stroke: theme.icon_stroke(),
                    rect: Rect { origin, size: Vec2::splat(side) },
                    color,
                    rotation: widget.rotation,
                });
            }
            WidgetKind::Progress { value, paint } => {
                if *value > 0.0 && *value < 1.0 {
                    let bar = Rect { origin: px.origin, size: Vec2::new(px.size.x * value, px.size.y) };
                    content.quads.push(Quad::fill(bar, px.size.y / 2.0, faded(theme.color(*paint))));
                }
            }
            WidgetKind::TextField(field) => {
                self.paint_field(node, field, px, &mut stacks[child_depth].content);
            }
            WidgetKind::Image { view: Some(view), size, crop } => {
                // Cover the frame, cropping the texture's bottom (or sides) if aspects differ.
                let tex = Vec2::new(size.0 as f32, size.1 as f32);
                let scale = (px.size / tex).max_element();
                let visible = (px.size / (tex * scale)).min(Vec2::ONE);
                let uv =
                    crop.unwrap_or(Rect { origin: Vec2::new((1.0 - visible.x) / 2.0, 0.0), size: visible });
                content.images.push(ImageDraw { texture: view, rect: px, uv, radius, opacity: alpha });
            }
            WidgetKind::Image { view: None, .. } => {}
            WidgetKind::Segmented { selected } => {
                let held = self.knobs.get(&node);
                let pos = held.map_or_else(|| self.segment_pos(node, *selected), |k| k.pos.value);
                let lens = held.map_or(0.0, |k| k.lens.value.clamp(0.0, 1.0));
                if let Some(pill) = self.segment_pill(node, pos) {
                    let surface = theme.color(Paint::Surface);
                    let rest = [surface[0], surface[1], surface[2], SEGMENT_PILL_OPACITY];
                    let rect = Rect { origin: pill.origin * s, size: pill.size * s };
                    let stretch = self.knob_stretch(held, px.size.x);
                    self.paint_pill(rect, rest, lens, stretch, fade, depth, stacks);
                }
            }
            WidgetKind::Toggle { on } => {
                let held = self.knobs.get(&node);
                let pos = held.map_or(if *on { 1.0 } else { 0.0 }, |k| k.pos.value);
                let lens = held.map_or(0.0, |k| k.lens.value.clamp(0.0, 1.0));
                // The track blends from grey to the accent with the knob's position.
                let t = pos.clamp(0.0, 1.0);
                let off = theme.color(Paint::Label);
                let accent = SWITCH_ON;
                let mix = |a: f32, b: f32| a + (b - a) * t;
                let track = faded([
                    mix(off[0], accent[0]),
                    mix(off[1], accent[1]),
                    mix(off[2], accent[2]),
                    mix(0.16, 1.0),
                ]);
                let (track_size, knob_size) = Self::toggle_geometry();
                let size = track_size * s;
                let origin = px.origin + (px.size - size) / 2.0;
                content.quads.push(Quad::fill(Rect { origin, size }, size.y / 2.0, track));
                let knob = knob_size * s;
                let travel = size.x - 2.0 * TOGGLE_INSET * s - knob.x;
                let center = Vec2::new(
                    origin.x + TOGGLE_INSET * s + knob.x / 2.0 + travel * pos,
                    origin.y + size.y / 2.0,
                );
                let stretch = self.knob_stretch(held, travel);
                self.paint_knob(center, knob, lens, stretch, fade, depth, true, stacks);
            }
            WidgetKind::Slider { value, line: true } => {
                let held = self.knobs.get(&node);
                let lens = held.map_or(0.0, |k| k.lens.value.clamp(0.0, 1.0));
                let rest = widget.track.unwrap_or(LINE_SLIDER);
                let swollen = widget.track.map_or(LINE_SLIDER_HELD, |t| t * LINE_SLIDER_SWELL);
                let h = (rest + (swollen - rest) * lens) * s;
                let y = px.origin.y + px.size.y / 2.0;
                let track = Rect::new(px.origin.x, y - h / 2.0, px.size.x, h);
                let label = theme.color(Paint::Label);
                content.quads.push(Quad::fill(track, h / 2.0, faded([label[0], label[1], label[2], 0.28])));
                let filled = Rect { size: Vec2::new(px.size.x * value, h), ..track };
                content.quads.push(Quad::fill(filled, h / 2.0, faded([label[0], label[1], label[2], 0.95])));
                // A dragged slider's fill end is a glass lens (no knob at rest).
                if lens > MIN_VISIBLE {
                    let stretch = self.knob_stretch(held, px.size.x);
                    let center = Vec2::new(px.origin.x + px.size.x * value, y);
                    let side = rest.max(LINE_SLIDER) * LINE_LENS * s;
                    self.paint_knob(center, Vec2::splat(side), lens, stretch, fade, depth, false, stacks);
                }
            }
            WidgetKind::Slider { value, line: false } => {
                let knob = SLIDER_KNOB * s;
                let inset = knob / 2.0;
                let y = px.origin.y + px.size.y / 2.0;
                let track_h = SLIDER_TRACK * s;
                let width = (px.size.x - 2.0 * inset).max(0.0);
                let track = Rect::new(px.origin.x + inset, y - track_h / 2.0, width, track_h);
                let label = theme.color(Paint::Label);
                content.quads.push(Quad::fill(
                    track,
                    track_h / 2.0,
                    faded([label[0], label[1], label[2], 0.15]),
                ));
                let filled = Rect { size: Vec2::new(width * value, track_h), ..track };
                content.quads.push(Quad::fill(filled, track_h / 2.0, faded(theme.color(Paint::Accent))));
                let held = self.knobs.get(&node);
                let lens = held.map_or(0.0, |k| k.lens.value.clamp(0.0, 1.0));
                let stretch = self.knob_stretch(held, width);
                let center = Vec2::new(px.origin.x + inset + width * value, y);
                self.paint_knob(center, Vec2::splat(knob), lens, stretch, fade, depth, true, stacks);
            }
        }

        if widget.clip {
            // The subtree as its own layers, each cut to this widget's frame.
            let mut sub: Vec<Stack<'a>> = Vec::new();
            self.painting_clipped.set(self.painting_clipped.get() + 1);
            {
                let mut stack = self.clip_stack.borrow_mut();
                let cut = stack.last().map_or(px, |outer| clip(px, *outer));
                stack.push(cut);
            }
            for child in self.tree.children(node).unwrap_or_default() {
                self.paint_node(child, child_depth, dim, child_fade, &mut sub);
            }
            self.painting_clipped.set(self.painting_clipped.get() - 1);
            self.clip_stack.borrow_mut().pop();
            let mut layers = Vec::new();
            self.flush(sub, &mut layers);
            for layer in &mut layers {
                if let Layer::Content(content) = layer {
                    // A clipped box inside a clipped page (release notes in Settings) keeps its
                    // own clip, cut to this one's.
                    content.clip = Some(content.clip.map_or(px, |inner| clip(inner, px)));
                }
            }
            stacks[child_depth].after.extend(layers);
            return;
        }
        for child in self.tree.children(node).unwrap_or_default() {
            self.paint_node(child, child_depth, dim, child_fade, stacks);
        }
        if let (Some(xf), Some(mark)) = (frame_xf, frame_mark) {
            transform_since(stacks, &mark, &xf, child_depth);
        }
        if let (Some(xf), Some(mark)) = (press_xf, mark) {
            transform_since(stacks, &mark, &xf, child_depth);
        }
    }

    /// A glass background painted without glass (flat mode): a solid card with a hairline border
    /// and a soft shadow, in the plane's content at the glass's stacking position (glass on glass
    /// is an inset instead, see `Theme::flat_paint`). Bridges between shapes aren't drawn.
    #[allow(clippy::too_many_arguments)]
    fn paint_flat_card(
        &self,
        widget: &Widget,
        theme: &Theme,
        rect: Rect,
        radius: f32,
        fade: f32,
        depth: usize,
        stacks: &mut [Stack<'_>],
    ) {
        if widget.join_only {
            return;
        }
        let nested = depth > 0;
        let (level, tint) = match widget.background {
            Background::TintedGlass(level, paint) => (level, Some(theme.color(paint))),
            Background::Glass(level) => (level, None),
            _ => return,
        };
        // A panel that carries text (a popover on the bar's thin glass) is as solid as its
        // density says: with no blur behind it, anything less would show the page through.
        let level = if level == GlassLevel::Lens { level } else { widget.glass_density.unwrap_or(level) };
        let s = self.scale;
        let paint = match (tint, widget.flat_role) {
            (Some(tint), _) => theme.flat_tinted(tint),
            (None, FlatRole::Field) => theme.flat_paint(level, true),
            (None, FlatRole::Raised) => theme.flat_raised(),
            (None, _) => theme.flat_paint(level, nested),
        };
        let faded = |mut c: [f32; 4]| {
            c[3] *= fade;
            c
        };
        let content = &mut stacks[depth + 1].content;
        if paint.shadow[3] > 0.0 && paint.shadow_blur > 0.0 {
            // A plain shadow, not `card_shadow`: a knockout trims the shadow where its own
            // (offset) shape lies, which leaves a bright crescent of the backdrop under a card
            // (very visible over video). A flat card is >= 94% opaque: nothing shows through it.
            content.quads.push(Quad::shadow(
                rect,
                radius,
                paint.shadow_dy * s,
                paint.shadow_blur * s,
                faded(paint.shadow),
            ));
        }
        content.quads.push(Quad::fill(rect, radius, faded(paint.fill)));
        if paint.border[3] > 0.0 {
            content.quads.push(Quad::ring(rect, radius, (FLAT_BORDER * s).max(1.0), faded(paint.border)));
        }
    }

    /// How far a held knob's lens stretches along its travel (`travel_px`, physical px): by
    /// its speed (as before, up to `glass.squash`), and by how far the pointer is pulled past
    /// the ends of the travel, on the saturating drag curve (up to `glass.drag_stretch_max`,
    /// a little more for a long lens).
    fn knob_stretch(&self, knob: Option<&knob::Knob>, travel_px: f32) -> f32 {
        let Some(k) = knob else { return 0.0 };
        let glass = &self.theme.config().glass;
        let by_speed = (k.speed.abs() * LENS_SQUASH).min(self.squash() * LENS_SQUASH_RATIO);
        let over_px = k.over * travel_px / self.scale;
        let by_pull = axion_physics::drag_stretch(
            over_px,
            glass.drag_stretch_max() * LENS_SQUASH_RATIO,
            glass.drag_resistance(),
        );
        by_speed.max(by_pull)
    }

    /// A switch or slider knob (physical px): opaque white at rest (a line slider has
    /// none: `rest_knob` off), a clear glass lens while held (`lens` 0→1), stretched along fast
    /// motion.
    #[allow(clippy::too_many_arguments)]
    fn paint_knob(
        &self,
        center: Vec2,
        knob: Vec2,
        lens: f32,
        stretch: f32,
        fade: f32,
        depth: usize,
        rest_knob: bool,
        stacks: &mut [Stack<'_>],
    ) {
        let s = self.scale;
        // Flat mode: a plain knob that never swells into a lens.
        let lens = if self.theme.flat() { 0.0 } else { lens };
        let grow = Vec2::ONE + (LENS_GROWTH - Vec2::ONE) * lens;
        let stretch = stretch * lens;
        let size = knob * grow * Vec2::new(1.0 + stretch, 1.0 / (1.0 + stretch).sqrt());
        let rect = Rect { origin: center - size / 2.0, size };
        let radius = size.y / 2.0;
        // The opaque knob fades out as the lens forms, and back in as it frosts over.
        let opaque = if rest_knob { (1.0 - lens) * fade } else { 0.0 };
        if opaque > MIN_VISIBLE {
            let content = &mut stacks[depth].content;
            let (dy, blur, alpha) = self.paint_theme().flat_shadow_if_flat(2.0, 6.0, 0.18);
            content.quads.push(Quad::shadow(rect, radius, dy * s, blur * s, [0.0, 0.0, 0.0, alpha * opaque]));
            content.quads.push(Quad::fill(rect, radius, [1.0, 1.0, 1.0, opaque]));
        }
        let glass = lens * fade;
        if glass > MIN_VISIBLE {
            let shape = GlassShape::rounded_rect(center, size / 2.0, radius);
            stacks[depth + 1].glass.push(GlassItem {
                level: GlassLevel::Lens,
                fade: quantize_fade(glass),
                tint: None,
                shape,
            });
        }
    }

    /// A segmented control's selection pill (physical px): `rest` colour at rest, a glass lens
    /// (grown like a held switch knob) while held.
    #[allow(clippy::too_many_arguments)]
    fn paint_pill(
        &self,
        rect: Rect,
        rest: [f32; 4],
        lens: f32,
        stretch: f32,
        fade: f32,
        depth: usize,
        stacks: &mut [Stack<'_>],
    ) {
        let s = self.scale;
        let lens = if self.theme.flat() { 0.0 } else { lens };
        let grow = Vec2::ONE + (Vec2::new(1.12, 1.25) - Vec2::ONE) * lens;
        let size = rect.size * grow * Vec2::new(1.0 + stretch * lens, 1.0 / (1.0 + stretch * lens).sqrt());
        let center = rect.center();
        let radius = size.y / 2.0;
        let r = Rect { origin: center - size / 2.0, size };
        let opaque = (1.0 - lens) * fade;
        if opaque > MIN_VISIBLE {
            let content = &mut stacks[depth].content;
            let (dy, blur, alpha) = self.paint_theme().flat_shadow_if_flat(1.0, 4.0, 0.10);
            content.quads.push(Quad::shadow(r, radius, dy * s, blur * s, [0.0, 0.0, 0.0, alpha * opaque]));
            content.quads.push(Quad::fill(r, radius, [rest[0], rest[1], rest[2], rest[3] * opaque]));
        }
        let glass = lens * fade;
        if glass > MIN_VISIBLE {
            let shape = GlassShape::rounded_rect(center, size / 2.0, radius);
            stacks[depth + 1].glass.push(GlassItem {
                level: GlassLevel::Lens,
                fade: quantize_fade(glass),
                tint: None,
                shape,
            });
        }
    }

    fn paint_field<'a>(
        &'a self,
        node: NodeId,
        field: &'a widget::TextField,
        px: Rect,
        content: &mut Content<'a>,
    ) {
        let s = self.scale;
        let theme = self.paint_theme();
        let focused = self.focus == Some(node);
        let empty = field.editor.text().is_empty();
        let (_, h) = measure(&field.buffer);
        let top = px.origin.y + ((px.size.y - h) / 2.0).max(0.0);
        let left = px.origin.x - field.scroll_x * s;

        if focused && let Some(range) = field.editor.selection() {
            let x0 = caret_x(&field.buffer, field.display_index(range.start));
            let x1 = caret_x(&field.buffer, field.display_index(range.end));
            let mut color = theme.color(Paint::Accent);
            color[3] = 0.28;
            let rect = Rect::new(left + x0, top, x1 - x0, h);
            content.quads.push(Quad::fill(clip(rect, px), 3.0 * s, color));
        }
        if empty {
            let mut color = theme.color(Paint::Secondary);
            if field.paint != Paint::Label {
                // A recoloured field (e.g. light text on dark) gets a matching placeholder.
                color = theme.color(field.paint);
                color[3] *= 0.6;
            }
            content.text.push(text_area(&field.placeholder, left, top, px, color));
        } else {
            content.text.push(text_area(&field.buffer, left, top, px, theme.color(field.paint)));
        }
        let blink_on = (self.caret_epoch.elapsed().as_millis() / CARET_BLINK.as_millis()).is_multiple_of(2);
        if focused && field.editor.selection().is_none() && blink_on {
            let x = left + caret_x(&field.buffer, field.display_index(field.editor.cursor()));
            let caret = Rect::new(x.round(), top, CARET_WIDTH * s, h);
            content.quads.push(Quad::fill(caret, 0.0, theme.color(Paint::Accent)));
        }
    }
}

#[derive(Default)]
struct Stack<'a> {
    content: Content<'a>,
    glass: Vec<GlassItem>,
    /// Layers of clipped subtrees on this stack, drawn after its content.
    after: Vec<Layer<'a>>,
}

/// A held (or springing back) glass: its swell and its pull toward the pointer.
struct Press {
    scale: Animated<f32>,
    offset: Animated<Vec2>,
    /// A button's own lens (see `Ui::press_target`), not group glass.
    lens: bool,
    /// 1 while held, 0 at rest (on the same spring): how far a lens has formed.
    held: Animated<f32>,
    /// How far it follows the pointer along each axis (rubber band limits, logical px).
    pull: Vec2,
    /// How far the pointer is from where it went down (logical px, sprung): what the stretch
    /// grows with, along `dir`, on a saturating curve (`axion_physics::drag_stretch`).
    distance: Animated<f32>,
    /// Unit direction of the pull (kept while the stretch springs back).
    dir: Vec2,
}

struct GlassItem {
    level: GlassLevel,
    fade: f32,
    /// A strong tint colour (tinted glass), instead of the level's own.
    tint: Option<[f32; 4]>,
    shape: GlassShape,
}

/// The press transform of held glass (physical px).
struct PressXf {
    scale: f32,
    offset: Vec2,
    stretch: Vec2,
    angle: f32,
    /// The content's own deformation: every icon, label and fill stays where it is and
    /// stretches about its own centre with the glass (per axis), so it looks distorted by
    /// the glass rather than moved.
    content_scale: Vec2,
    /// Set for an animated frame (popover growing): the whole subtree is mapped from the
    /// layout frame onto the visual one, so items move with it instead of staying put.
    carry: Option<Carry>,
}

/// The shared stretch transform of glass and its content: `e` along the unit direction `dir`
/// (the glass's own shader: `(1 + e, 1 / (1 + e))` along/across, area preserving). Returns the
/// stretch angle and that scaling as seen on the screen axes (exact for pulls along an axis,
/// the nearest axis-aligned scaling for diagonal ones), for quads, icons and text boxes.
fn stretch_content(dir: Vec2, e: f32) -> (f32, Vec2) {
    let (along, across) = (1.0 + e, 1.0 / (1.0 + e));
    let (c2, s2) = (dir.x * dir.x, dir.y * dir.y);
    (dir.y.atan2(dir.x), Vec2::new(along * c2 + across * s2, along * s2 + across * c2))
}

/// Maps the layout frame's centre onto the visual frame's.
struct Carry {
    from: Vec2,
    to: Vec2,
}

impl PressXf {
    /// Where an item centred at `c` ends up.
    fn place(&self, c: Vec2) -> Vec2 {
        match &self.carry {
            Some(carry) => carry.to + (c - carry.from) * self.content_scale,
            None => c,
        }
    }

    fn rect(&self, r: Rect) -> Rect {
        let size = r.size * self.content_scale;
        Rect { origin: self.place(r.center()) - size / 2.0, size }
    }

    /// For text, which only scales uniformly.
    fn uniform(&self) -> f32 {
        self.content_scale.min_element()
    }
}

/// How much each stack held before a subtree was painted.
struct Mark {
    quads: usize,
    images: usize,
    icons: usize,
    text: usize,
    /// Layers of clipped subtrees.
    after: usize,
}

impl Mark {
    const NONE: Mark = Mark { quads: 0, images: 0, icons: 0, text: 0, after: 0 };
}

fn marks(stacks: &[Stack<'_>]) -> Vec<Mark> {
    stacks
        .iter()
        .map(|s| Mark {
            quads: s.content.quads.len(),
            images: s.content.images.len(),
            icons: s.content.icons.len(),
            text: s.content.text.len(),
            after: s.after.len(),
        })
        .collect()
}

/// Moves what a held glass's subtree painted (from `from_depth` on) along with the glass,
/// clipped lists (a sheet's scrolled page) and their clip included.
fn transform_since(stacks: &mut [Stack<'_>], mark: &[Mark], xf: &PressXf, from_depth: usize) {
    for (d, stack) in stacks.iter_mut().enumerate().skip(from_depth) {
        let m = mark.get(d).unwrap_or(&Mark::NONE);
        transform_content(&mut stack.content, m, xf);
        for layer in stack.after.iter_mut().skip(m.after) {
            if let Layer::Content(c) = layer {
                transform_content(c, &Mark::NONE, xf);
                c.clip = c.clip.map(|r| xf.rect(r));
            }
        }
    }
}

fn transform_content(c: &mut Content<'_>, m: &Mark, xf: &PressXf) {
    for q in c.quads.iter_mut().skip(m.quads) {
        q.rect = xf.rect(q.rect);
        q.radius *= xf.uniform();
    }
    for i in c.images.iter_mut().skip(m.images) {
        i.rect = xf.rect(i.rect);
    }
    for i in c.icons.iter_mut().skip(m.icons) {
        i.rect = xf.rect(i.rect);
    }
    for t in c.text.iter_mut().skip(m.text) {
        // Text scales uniformly about the centre of its box.
        let b = &mut t.bounds;
        let center = Vec2::new(b.left as f32 + b.right as f32, b.top as f32 + b.bottom as f32) / 2.0;
        let u = xf.uniform();
        let placed = xf.place(center);
        let about = |p: Vec2| placed + (p - center) * u;
        let p = about(Vec2::new(t.left, t.top));
        t.left = p.x;
        t.top = p.y;
        t.scale *= u;
        let tl = about(Vec2::new(b.left as f32, b.top as f32));
        let br = about(Vec2::new(b.right as f32, b.bottom as f32));
        *b = glyphon::TextBounds {
            left: tl.x.floor() as i32,
            top: tl.y.floor() as i32,
            right: br.x.ceil() as i32,
            bottom: br.y.ceil() as i32,
        };
    }
}

/// The tint strength of a glass widget's shape: its own density if it has one, else the
/// button strength for a button (a widget with an action) so a glass button is pure refractive
/// glass without a grey plate (tinted primary buttons excepted), else `None` (the pass's level strength).
fn glass_tint_opacity(widget: &Widget, theme: &Theme) -> Option<f32> {
    widget.glass_density.map(|l| theme.glass_opacity(l)).or_else(|| {
        // A tinted (primary) button keeps its level's strength: the colour is its point.
        (widget.action.is_some() && matches!(widget.background, Background::Glass(_)))
            .then(|| theme.glass_button_opacity())
    })
}

/// Flat mode audit: what in a painted frame is glass or imitates it. Flat mode draws solid
/// cards with a hairline border and a subtle shadow, nothing else: no glass pass, no ring
/// thicker than the border (a rim), no knocked-out shadow (it leaves a bright crescent) and no
/// shadow heavier than the heaviest flat card. One line per violation (empty: clean).
pub fn flat_violations(layers: &[Layer<'_>], scale: f32) -> Vec<String> {
    let (_, max_blur, strength) = theme::FLAT_SHADOW[2];
    // The darkest chrome (the player's, dark in every scheme) has the strongest shadows.
    let max_alpha = strength[0].max(strength[1]);
    let mut out = Vec::new();
    for (i, layer) in layers.iter().enumerate() {
        match layer {
            Layer::Glass { shapes, .. } => {
                out.push(format!("layer {i}: a glass pass of {} shapes", shapes.len()));
            }
            Layer::Content(content) => {
                for q in &content.quads {
                    let at = format!(
                        "layer {i} quad at ({:.0}, {:.0}) {:.0}x{:.0}",
                        q.rect.origin.x, q.rect.origin.y, q.rect.size.x, q.rect.size.y
                    );
                    if q.ring > FLAT_BORDER * scale * 1.5 + 0.5 {
                        out.push(format!("{at}: a {:.1}px ring (rim)", q.ring));
                    }
                    if q.knockout {
                        out.push(format!("{at}: a knockout shadow"));
                    }
                    if q.softness > max_blur * scale + 0.5
                        || (q.softness > 0.0 && q.color[3] > max_alpha + 0.01)
                    {
                        out.push(format!(
                            "{at}: a heavy shadow (blur {:.0}, alpha {:.2})",
                            q.softness, q.color[3]
                        ));
                    }
                }
            }
            Layer::Image(_) => {}
        }
    }
    out
}

fn quantize_fade(fade: f32) -> f32 {
    ((fade * GLASS_FADE_STEPS).round() / GLASS_FADE_STEPS).clamp(0.0, 1.0)
}

fn text_area(
    buffer: &glyphon::Buffer,
    left: f32,
    top: f32,
    clip: Rect,
    color: [f32; 4],
) -> glyphon::TextArea<'_> {
    let [r, g, b, a] = color.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
    glyphon::TextArea {
        buffer,
        left,
        top,
        scale: 1.0,
        bounds: glyphon::TextBounds {
            left: clip.origin.x.floor() as i32,
            top: clip.origin.y.floor() as i32,
            right: (clip.origin.x + clip.size.x).ceil() as i32,
            bottom: (clip.origin.y + clip.size.y).ceil() as i32,
        },
        default_color: glyphon::Color::rgba(r, g, b, a),
        custom_glyphs: &[],
    }
}

fn clip(rect: Rect, to: Rect) -> Rect {
    let min = rect.origin.max(to.origin);
    let max = (rect.origin + rect.size).min(to.origin + to.size);
    Rect { origin: min, size: (max - min).max(Vec2::ZERO) }
}

/// `inner` lies wholly inside `outer` (half a pixel of slack).
fn contains_rect(outer: Rect, inner: Rect) -> bool {
    let slack = 0.5;
    inner.origin.x >= outer.origin.x - slack
        && inner.origin.y >= outer.origin.y - slack
        && inner.origin.x + inner.size.x <= outer.origin.x + outer.size.x + slack
        && inner.origin.y + inner.size.y <= outer.origin.y + outer.size.y + slack
}

fn rounded_contains(frame: Rect, radius: f32, p: Vec2) -> bool {
    if !frame.contains(p) {
        return false;
    }
    let r = radius.min(frame.size.min_element() / 2.0);
    if r <= 0.0 {
        return true;
    }
    let half = frame.size / 2.0;
    let q = (p - frame.center()).abs() - half + Vec2::splat(r);
    q.max(Vec2::ZERO).length() + q.max_element().min(0.0) - r <= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui() -> Ui {
        let mut ui = Ui::new(Theme::new(&axion_config::Config::default(), false), 1.0);
        ui.set_viewport(Vec2::new(800.0, 600.0), 1.0);
        ui
    }

    #[test]
    fn long_labels_wrap_and_clipped_ones_are_reported() {
        let mut ui = ui();
        let root = ui.root();
        let column = ui.add(
            root,
            Widget::container(),
            Style {
                flex_direction: FlexDirection::Column,
                size: Size { width: length(120.0), height: auto() },
                ..Default::default()
            },
        );
        let text = "Zwischenablage nach dem Kopieren leeren";
        let wrapping = ui.add_label(column, text, TextStyle::Body, Style::default());
        ui.set_label_lines(wrapping, 3);
        let _single = ui.add_label(column, text, TextStyle::Body, Style::default());
        ui.layout();
        let line = ui.widget(wrapping).unwrap().frame().size.y;
        assert!(line > 30.0, "the wrapping label grows taller: {line}");
        let WidgetKind::Label(l) = &ui.widget(wrapping).unwrap().kind else { unreachable!() };
        let lines: Vec<String> = l
            .buffer
            .layout_runs()
            .map(|r| r.glyphs.iter().map(|g| &r.text[g.start..g.end]).collect())
            .collect();
        assert_eq!(lines, ["Zwischenablage", "nach dem", "Kopieren leeren"]);
        assert!(ui.clipped_texts().iter().any(|(t, ..)| t == text), "the one-line copy is clipped");
        assert_eq!(ui.clipped_texts().len(), 1, "the wrapping one is not");
    }

    /// Linear-light relative luminance of an sRGB colour (WCAG).
    fn luminance(c: [f32; 3]) -> f32 {
        let lin = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
    }

    fn contrast(a: [f32; 3], b: [f32; 3]) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    fn over(top: [f32; 4], alpha: f32, below: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|i| top[i] * alpha + below[i] * (1.0 - alpha))
    }

    /// Text and icons stay legible on every hover / selected / pressed surface, in both themes
    /// and with Reduce Transparency on or off (text 4.5:1, icons 3:1).
    #[test]
    fn feedback_surfaces_keep_contrast() {
        for dark in [false, true] {
            for reduce in [false, true] {
                let mut config = axion_config::Config::default();
                config.glass.reduce_transparency = reduce;
                let theme = Theme::new(&config, dark);
                let label = theme.color(Paint::Label);
                let secondary = theme.color(Paint::Secondary);
                let surface = theme.color(Paint::Surface);
                let ground = theme.color(Paint::Ground);
                // What a row lies on: the sidebar/bar glass (near-opaque tint over the ground).
                let glass = theme.glass(GlassLevel::Regular, 1.0);
                let panel = over(glass.tint, glass.tint[3].max(0.94), [ground[0], ground[1], ground[2]]);
                let mut surfaces = vec![
                    ("rest", panel),
                    ("hover", over(label, HOVER_ALPHA, panel)),
                    ("pressed", over(label, PRESS_ALPHA, panel)),
                    ("selected", over(surface, 0.7, panel)),
                    ("selected+hover", over(label, HOVER_ALPHA, over(surface, 0.7, panel))),
                ];
                if reduce {
                    let lens = theme.glass(GlassLevel::Lens, 1.0).tint;
                    assert!((lens[3] - 1.0).abs() < 1e-6, "an opaque lens");
                    surfaces.push(("lens", [lens[0], lens[1], lens[2]]));
                }
                for (name, bg) in surfaces {
                    for (what, fg, min) in
                        [("label", label, 4.5), ("secondary", secondary, 4.5), ("icon", label, 3.0)]
                    {
                        let c = contrast([fg[0], fg[1], fg[2]], bg);
                        assert!(c >= min, "{what} on {name} (dark {dark}, reduce {reduce}): {c:.2} < {min}");
                    }
                }
            }
        }
    }

    #[test]
    fn glass_buttons_are_clear_glass_not_a_plate() {
        let theme = Theme::new(&axion_config::Config::default(), false);
        let panel = Widget::container().with_background(Background::Glass(GlassLevel::Thin));
        let button =
            Widget::container().with_action(ActionId(1)).with_background(Background::Glass(GlassLevel::Thin));
        let primary = Widget::container()
            .with_action(ActionId(1))
            .with_background(Background::TintedGlass(GlassLevel::Thin, Paint::Accent));
        // A panel keeps its level's strength, a glass button is far clearer, a primary button
        // keeps its colour; none of them has a fill of its own.
        assert_eq!(glass_tint_opacity(&panel, &theme), None);
        let clear = glass_tint_opacity(&button, &theme).expect("button strength");
        assert!(clear <= 0.1 && clear < theme.glass_opacity(GlassLevel::Thin) / 3.0);
        assert_eq!(glass_tint_opacity(&primary, &theme), None);
        for w in [&button, &primary] {
            assert!(matches!(w.background, Background::Glass(_) | Background::TintedGlass(..)));
        }
    }

    fn row(ui: &mut Ui) -> (NodeId, NodeId, NodeId) {
        let root = ui.root();
        let bar = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
            Style {
                size: Size { width: length(400.0), height: length(56.0) },
                padding: taffy::Rect {
                    left: length(6.0),
                    right: length(6.0),
                    top: length(6.0),
                    bottom: length(6.0),
                },
                gap: Size { width: length(4.0), height: length(0.0) },
                ..Default::default()
            },
        );
        let button = ui.add(
            bar,
            Widget::container().with_action(ActionId(7)).with_radius(CAPSULE),
            Style { size: Size { width: length(44.0), height: length(44.0) }, ..Default::default() },
        );
        let field = ui.add_text_field(
            bar,
            "Search or enter website",
            TextStyle::Body,
            Style { flex_grow: 1.0, ..Default::default() },
        );
        ui.layout();
        (bar, button, field)
    }

    #[test]
    fn lays_out_and_hits() {
        let mut ui = ui();
        let (bar, button, field) = row(&mut ui);
        assert_eq!(ui.frame(bar).size, Vec2::new(400.0, 56.0));
        assert_eq!(ui.frame(button).origin, Vec2::new(6.0, 6.0));
        assert_eq!(ui.frame(field).origin.x, 54.0);
        assert_eq!(ui.hit(Vec2::new(28.0, 28.0)), Some(button));
        assert_eq!(ui.hit(Vec2::new(200.0, 28.0)), Some(field));
        assert_eq!(ui.hit(Vec2::new(600.0, 300.0)), None);
        // Rounded corners don't catch clicks.
        assert_eq!(ui.hit(Vec2::new(1.0, 1.0)), None);
    }

    #[test]
    fn click_and_type() {
        let mut ui = ui();
        let (_, button, field) = row(&mut ui);
        ui.handle(UiInput::PointerMove(Vec2::new(28.0, 28.0)));
        ui.handle(UiInput::PointerDown);
        let (_, events) = ui.handle(UiInput::PointerUp);
        assert_eq!(events, vec![UiEvent::Clicked { action: ActionId(7), node: button }]);

        ui.handle(UiInput::PointerMove(Vec2::new(200.0, 28.0)));
        let (_, events) = ui.handle(UiInput::PointerDown);
        assert_eq!(events, vec![UiEvent::FocusChanged { node: Some(field) }]);
        ui.handle(UiInput::Text("rust".into()));
        let (consumed, events) =
            ui.handle(UiInput::Key { key: Key::Named("Enter".into()), modifiers: Modifiers::default() });
        assert!(consumed);
        assert_eq!(events, vec![UiEvent::Submitted { node: field, text: "rust".into() }]);
    }

    #[test]
    fn typing_dirties_the_layout_and_the_next_frame_has_every_letter() {
        let mut ui = ui();
        let (_, _, field) = row(&mut ui);
        ui.set_secure(field, true);
        ui.focus(field);
        ui.layout();
        assert!(!ui.needs_layout);
        for (i, letter) in "hunter2".chars().enumerate() {
            ui.handle(UiInput::Text(letter.to_string()));
            // Each keystroke asks for a layout, and the text shown after it is complete.
            assert!(ui.needs_layout, "typing {letter:?} must dirty the layout");
            ui.layout();
            let Some(Widget { kind: WidgetKind::TextField(f), .. }) = ui.widget(field) else {
                panic!("no field");
            };
            assert_eq!(f.display_text().chars().count(), i + 1);
        }
        assert_eq!(ui.field_text(field), Some("hunter2"));
    }

    #[test]
    fn secure_fields_hide_and_keep_their_text() {
        let mut ui = ui();
        let (_, _, field) = row(&mut ui);
        ui.set_secure(field, true);
        ui.focus(field);
        ui.handle(UiInput::Text("pässwort".into()));
        assert_eq!(ui.field_text(field), Some("pässwort"));
        let ctrl = Modifiers { ctrl: true, ..Default::default() };
        ui.handle(UiInput::Key { key: Key::Char('a'), modifiers: ctrl });
        let (_, events) = ui.handle(UiInput::Key { key: Key::Char('c'), modifiers: ctrl });
        assert!(events.is_empty(), "copying a password must do nothing");
        // Caret positions map between "ä" (2 bytes) and "•" (3 bytes).
        if let Some(Widget { kind: WidgetKind::TextField(f), .. }) = ui.widget(field) {
            assert_eq!(f.display_text().chars().count(), 8);
            // "pä" is 3 bytes; shown as two bullets, 6 bytes.
            assert_eq!(f.display_index(3), 6);
            assert_eq!(f.real_index(6), 3);
        }
        ui.layout();
        let mut layers = Vec::new();
        ui.paint(&mut layers);
    }

    #[test]
    fn segmented_taps_and_slides() {
        let mut ui = ui();
        let root = ui.root();
        let seg = ui.add(
            root,
            Widget::segmented(1),
            Style { flex_direction: FlexDirection::Row, ..Default::default() },
        );
        let cell =
            || Style { size: Size { width: length(100.0), height: length(30.0) }, ..Default::default() };
        let a = ui.add(seg, Widget::container(), cell());
        let b = ui.add(seg, Widget::container(), cell());
        ui.layout();
        // A tap on the other segment selects it when let go.
        ui.handle(UiInput::PointerMove(ui.frame(a).center()));
        ui.handle(UiInput::PointerDown);
        let (_, events) = ui.handle(UiInput::PointerUp);
        assert_eq!(events, vec![UiEvent::ValueChanged { node: seg, value: 0.0 }]);
        // Holding the pill and sliding it most of the way across snaps to that segment.
        ui.handle(UiInput::PointerMove(ui.frame(a).center()));
        ui.handle(UiInput::PointerDown);
        ui.handle(UiInput::PointerMove(ui.frame(a).center() + Vec2::new(70.0, 0.0)));
        let (_, events) = ui.handle(UiInput::PointerUp);
        assert_eq!(events, vec![UiEvent::ValueChanged { node: seg, value: 1.0 }]);
        // A short slide snaps back: nothing changes.
        ui.handle(UiInput::PointerMove(ui.frame(b).center()));
        ui.handle(UiInput::PointerDown);
        ui.handle(UiInput::PointerMove(ui.frame(b).center() - Vec2::new(20.0, 0.0)));
        let (_, events) = ui.handle(UiInput::PointerUp);
        assert!(events.is_empty(), "{events:?}");
    }

    /// The selection pill is the selected segment's real frame, whatever the segments' widths
    /// (translated labels make them unequal), and in between it moves and grows from one
    /// segment's frame to the next one's.
    #[test]
    fn segmented_pill_matches_unequal_segments() {
        let mut ui = ui();
        let root = ui.root();
        let seg = ui.add(
            root,
            Widget::segmented(0),
            Style { flex_direction: FlexDirection::Row, ..Default::default() },
        );
        let widths = [60.0_f32, 140.0, 90.0];
        let cells: Vec<NodeId> = widths
            .iter()
            .map(|&w| {
                ui.add(
                    seg,
                    Widget::container(),
                    Style { size: Size { width: length(w), height: length(30.0) }, ..Default::default() },
                )
            })
            .collect();
        ui.layout();
        for (i, &cell) in cells.iter().enumerate() {
            let pill = ui.segment_pill(seg, ui.segment_pos(seg, i)).expect("pill");
            let frame = ui.frame(cell);
            assert!(
                (pill.origin - frame.origin).abs().max_element() < 0.01
                    && (pill.size - frame.size).abs().max_element() < 0.01,
                "segment {i}: pill {pill:?} vs frame {frame:?}"
            );
        }
        // Half way between segments 0 and 1: the mean of both frames.
        let half = ui.segment_pill(seg, 0.25).expect("pill");
        let (a, b) = (ui.frame(cells[0]), ui.frame(cells[1]));
        assert!((half.size.x - (a.size.x + b.size.x) / 2.0).abs() < 0.01);
        assert!((half.center().x - (a.center().x + b.center().x) / 2.0).abs() < 0.01);
        // After the labels change width the pill follows the new frames at once.
        ui.set_style(
            cells[1],
            Style { size: Size { width: length(40.0), height: length(30.0) }, ..Default::default() },
        );
        ui.layout();
        let pill = ui.segment_pill(seg, ui.segment_pos(seg, 1)).expect("pill");
        assert!(
            (pill.size.x - 40.0).abs() < 0.01 && (pill.origin.x - ui.frame(cells[1]).origin.x).abs() < 0.01
        );
    }

    #[test]
    fn toggles_and_sliders() {
        let mut ui = ui();
        let root = ui.root();
        let toggle = ui.add(root, Widget::toggle(false), Style::default());
        let slider = ui.add(
            root,
            Widget::slider(0.0),
            Style { size: Size { width: length(224.0), height: auto() }, ..Default::default() },
        );
        ui.layout();
        let t = ui.frame(toggle);
        ui.handle(UiInput::PointerMove(t.center()));
        // A switch flips when let go (a held knob is a lens that can still be dragged back).
        let (_, events) = ui.handle(UiInput::PointerDown);
        assert!(events.iter().all(|e| !matches!(e, UiEvent::ValueChanged { .. })));
        let (_, events) = ui.handle(UiInput::PointerUp);
        assert!(events.contains(&UiEvent::ValueChanged { node: toggle, value: 1.0 }));
        // Dragging the knob back to the start and letting go turns it off again.
        ui.handle(UiInput::PointerMove(t.center()));
        ui.handle(UiInput::PointerDown);
        ui.handle(UiInput::PointerMove(t.center() - Vec2::new(40.0, 0.0)));
        let (_, events) = ui.handle(UiInput::PointerUp);
        assert!(events.contains(&UiEvent::ValueChanged { node: toggle, value: 0.0 }));

        let f = ui.frame(slider);
        ui.handle(UiInput::PointerMove(Vec2::new(f.origin.x + 12.0, f.center().y)));
        ui.handle(UiInput::PointerDown);
        // Drag to the middle of the track (the knob is inset 12 px on each side).
        let (_, events) = ui.handle(UiInput::PointerMove(Vec2::new(f.origin.x + 112.0, f.center().y)));
        assert_eq!(events, vec![UiEvent::ValueChanged { node: slider, value: 0.5 }]);
        ui.handle(UiInput::PointerUp);
        let (_, events) = ui.handle(UiInput::PointerMove(Vec2::new(f.origin.x + 200.0, f.center().y)));
        assert!(events.is_empty(), "no drag after release");
    }

    #[test]
    fn drag_ends_on_release_outside_focus_loss_or_hiding() {
        let mut ui = ui();
        let root = ui.root();
        let slider = ui.add(
            root,
            Widget::slider(0.0),
            Style { size: Size { width: length(224.0), height: auto() }, ..Default::default() },
        );
        let button = ui.add(
            root,
            Widget::container().with_action(ActionId(3)),
            Style { size: Size { width: length(44.0), height: length(44.0) }, ..Default::default() },
        );
        ui.layout();
        let f = ui.frame(slider);
        let grab = |ui: &mut Ui| {
            ui.handle(UiInput::PointerMove(Vec2::new(f.origin.x + 12.0, f.center().y)));
            ui.handle(UiInput::PointerDown);
            assert_eq!(ui.dragging(), Some(slider));
        };
        // Press, move far outside the control, release there: the drag ends.
        grab(&mut ui);
        ui.handle(UiInput::PointerMove(Vec2::new(700.0, 500.0)));
        assert_eq!(ui.dragging(), Some(slider));
        ui.handle(UiInput::PointerUp);
        assert!(!ui.is_holding());
        // The button-up never arrives (focus lost, Esc, pointer outside): explicit end.
        grab(&mut ui);
        assert!(ui.is_holding());
        ui.end_interaction();
        assert!(!ui.is_holding());
        let (_, events) = ui.handle(UiInput::PointerMove(Vec2::new(f.origin.x + 200.0, f.center().y)));
        assert!(events.is_empty(), "no drag after the forced end");
        // A held button lets go without a click.
        let b = ui.frame(button);
        ui.handle(UiInput::PointerMove(b.center()));
        ui.handle(UiInput::PointerDown);
        assert!(ui.is_holding());
        let events = ui.end_interaction();
        assert!(events.is_empty() && !ui.is_holding());
        let (_, events) = ui.handle(UiInput::PointerUp);
        assert!(events.iter().all(|e| !matches!(e, UiEvent::Clicked { .. })));
        // Hiding or removing the held control ends the drag too.
        grab(&mut ui);
        ui.set_visible(slider, false);
        assert!(!ui.is_holding());
        ui.set_visible(slider, true);
        grab(&mut ui);
        ui.remove(slider);
        assert!(!ui.is_holding());
    }

    #[test]
    fn removing_the_last_child_of_a_block_with_absolute_boxes_does_not_break_the_next_layout() {
        // The separate tab strip: every tab has an absolute hairline; switching to the compact
        // bar removes all tabs, and the empty strip (laid out as a leaf) kept the hairlines in
        // its list of absolute boxes, shown or hidden.
        for hide in [false, true] {
            let mut ui = ui();
            let root = ui.root();
            let strip = ui.add(
                root,
                Widget::container(),
                Style {
                    position: Position::Absolute,
                    size: Size { width: length(400.0), height: length(40.0) },
                    ..Default::default()
                },
            );
            let mut tabs = Vec::new();
            for _ in 0..3 {
                let tab = ui.add(
                    strip,
                    Widget::container(),
                    Style { size: Size { width: length(100.0), height: length(30.0) }, ..Default::default() },
                );
                ui.add(
                    tab,
                    Widget::container(),
                    Style {
                        position: Position::Absolute,
                        size: Size { width: length(1.0), height: length(16.0) },
                        ..Default::default()
                    },
                );
                tabs.push(tab);
            }
            ui.layout();
            ui.set_visible(strip, !hide);
            for tab in tabs {
                ui.remove(tab);
            }
            ui.layout();
            assert!(ui.children(strip).is_empty(), "the stand-in child is gone again");
            ui.set_visible(strip, true);
            ui.layout();
            assert_eq!(ui.frame(strip).size.x, 400.0);
        }
    }

    #[test]
    fn removing_an_absolute_box_below_a_hidden_block_does_not_break_the_next_layout() {
        // The Playlist page: an absolute badge in a list row; the page is hidden (fullscreen
        // player) and its rows are rebuilt meanwhile. Taffy kept the removed badge in the
        // page's list of absolute boxes and panicked in the next layout.
        let mut ui = ui();
        let root = ui.root();
        let page = ui.add(
            root,
            Widget::container(),
            Style {
                position: Position::Absolute,
                size: Size { width: length(400.0), height: length(300.0) },
                ..Default::default()
            },
        );
        let list = ui.add(page, Widget::container(), Style::default());
        let row = |ui: &mut Ui, list: NodeId| {
            let thumb = ui.add(
                list,
                Widget::container(),
                Style { size: Size { width: length(40.0), height: length(30.0) }, ..Default::default() },
            );
            ui.add(
                thumb,
                Widget::container(),
                Style {
                    position: Position::Absolute,
                    size: Size { width: length(10.0), height: length(8.0) },
                    ..Default::default()
                },
            )
        };
        row(&mut ui, list);
        ui.layout();
        for hidden_ancestor in [page, root] {
            // Hidden (itself or higher up), rows rebuilt, laid out again.
            let was = ui.is_visible(hidden_ancestor);
            ui.set_visible(hidden_ancestor, false);
            ui.clear_children(list);
            row(&mut ui, list);
            ui.layout();
            ui.set_visible(hidden_ancestor, was);
            ui.layout();
            // Still consistent when shown: the new badge is laid out.
            let badge = *ui.children(*ui.children(list).last().unwrap()).first().unwrap();
            assert!(ui.frame(badge).size.x > 0.0);
        }
    }

    #[test]
    fn a_line_slider_covers_the_pointer_and_maps_x_to_its_value() {
        let mut ui = ui();
        let root = ui.root();
        let bar = ui.add(
            root,
            Widget::container(),
            Style { size: Size { width: length(400.0), height: length(40.0) }, ..Default::default() },
        );
        let slider = ui.add(
            bar,
            Widget::line_slider(0.0).with_track(8.0),
            Style {
                flex_grow: 1.0,
                min_size: Size { width: length(40.0), height: length(28.0) },
                ..Default::default()
            },
        );
        ui.layout();
        let f = ui.frame(slider);
        assert!(f.size.y >= 24.0, "the grab area is tall: {f:?}");
        let at = |t: f32| Vec2::new(f.origin.x + f.size.x * t, f.center().y);
        // It takes the pointer but is no button: `hit_action` misses it, `covers` doesn't (the
        // fullscreen Playlist player let sliders' presses fall through because of that).
        assert!(ui.covers(at(0.5)));
        assert!(ui.hit_action(at(0.5)).is_none());
        // A click seeks to where it lands, a drag follows.
        ui.handle(UiInput::PointerMove(at(0.25)));
        let (consumed, events) = ui.handle(UiInput::PointerDown);
        assert!(consumed);
        let value = |events: &[UiEvent]| match events.last() {
            Some(UiEvent::ValueChanged { value, .. }) => *value,
            other => panic!("no value change: {other:?}"),
        };
        assert!((value(&events) - 0.25).abs() < 1e-3);
        let (_, events) = ui.handle(UiInput::PointerMove(at(0.8)));
        assert!((value(&events) - 0.8).abs() < 1e-3);
        ui.handle(UiInput::PointerUp);
        assert!(!ui.is_holding());
    }

    /// Glass in a clipped (scrolling) box cannot be cut to it: wholly outside it is not drawn
    /// (a scrolled-out tinted button used to float below its page), straddling the edge it is a
    /// clipped flat card, wholly inside it stays glass.
    #[test]
    fn glass_in_a_clipped_box_never_leaves_it() {
        let mut ui = ui();
        let root = ui.root();
        let boxed = ui.add(
            root,
            Widget::container().with_clip(),
            Style { size: Size { width: length(200.0), height: length(100.0) }, ..Default::default() },
        );
        let tall = ui.add(
            boxed,
            Widget::container(),
            Style {
                size: Size { width: length(200.0), height: length(400.0) },
                flex_direction: FlexDirection::Column,
                justify_content: taffy::JustifyContent::SPACE_BETWEEN,
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        let button = |ui: &mut Ui| {
            ui.add(
                tall,
                Widget::container()
                    .with_radius(10.0)
                    .with_background(Background::TintedGlass(GlassLevel::Thin, Paint::Accent)),
                Style { size: Size { width: length(80.0), height: length(30.0) }, ..Default::default() },
            )
        };
        let inside = button(&mut ui);
        let outside = button(&mut ui);
        ui.layout();
        assert!(ui.frame(inside).origin.y + 30.0 <= 100.0 && ui.frame(outside).origin.y >= 100.0);
        let glass_shapes = |ui: &Ui| {
            let mut layers = Vec::new();
            ui.paint(&mut layers);
            layers
                .iter()
                .map(|l| if let Layer::Glass { shapes, .. } = l { shapes.len() } else { 0 })
                .sum::<usize>()
        };
        assert_eq!(glass_shapes(&ui), 1, "the visible button is glass, the scrolled-out one is not drawn");
    }

    fn flat_ui() -> Ui {
        let mut config = axion_config::Config::default();
        config.glass.style = axion_config::GlassStyle::Flat;
        let mut ui = Ui::new(Theme::new(&config, false), 1.0);
        ui.set_viewport(Vec2::new(800.0, 600.0), 1.0);
        ui
    }

    #[test]
    fn flat_mode_paints_no_glass_layers_at_all() {
        let mut ui = flat_ui();
        row(&mut ui);
        ui.layout();
        let mut layers = Vec::new();
        ui.paint(&mut layers);
        assert!(layers.iter().all(|l| !matches!(l, Layer::Glass { .. })), "flat mode skips glass passes");
        // The bar became a card: a shadow, a fill and a one-px ring.
        let quads: Vec<&Quad> = layers
            .iter()
            .filter_map(|l| if let Layer::Content(c) = l { Some(c.quads.iter()) } else { None })
            .flatten()
            .collect();
        assert!(quads.iter().any(|q| q.softness > 0.0 && !q.knockout), "a shadow");
        assert!(quads.iter().any(|q| q.ring >= 1.0), "a border");
        assert!(quads.iter().any(|q| q.ring == 0.0 && q.softness == 0.0 && q.color[3] > 0.9), "a solid fill");
    }

    /// Every kind of glass surface the app uses, painted in flat mode in both schemes, at rest,
    /// hovered, held and with a held knob: nothing is glass (no pass) and nothing imitates it (no
    /// rim, no knocked-out or heavy shadow, no lens). `flat_violations` is also the audit the
    /// `flat-audit` capture aid runs on the real windows.
    #[test]
    fn flat_mode_draws_no_glass_and_no_glass_lookalikes() {
        for dark in [false, true] {
            let mut config = axion_config::Config::default();
            config.glass.style = axion_config::GlassStyle::Flat;
            let mut ui = Ui::new(Theme::new(&config, dark), 2.0);
            ui.set_viewport(Vec2::new(900.0, 700.0), 2.0);
            let (_, button, _) = row(&mut ui);
            let root = ui.root();
            let cell = |w: f32, h: f32| Style {
                size: Size { width: length(w), height: length(h) },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            };
            let mut buttons = Vec::new();
            // A text button of each tint (accent, destructive) and of each level, a player disc
            // over video (thick glass with an icon), a hover highlight (lens level) and a card
            // with a nested card on it.
            let backgrounds = [
                Background::TintedGlass(GlassLevel::Thin, Paint::Accent),
                Background::TintedGlass(GlassLevel::Thin, Paint::Fixed(axion_config::Color::rgb(0xD0362B))),
                Background::Glass(GlassLevel::Thin),
                Background::Glass(GlassLevel::Regular),
                Background::Glass(GlassLevel::Thick),
                Background::Glass(GlassLevel::Lens),
                Background::Soft(Paint::Surface, 0.6, 8.0),
            ];
            for background in backgrounds {
                let b = ui.add(
                    root,
                    Widget::container()
                        .with_action(ActionId(9))
                        .with_radius(CAPSULE)
                        .with_background(background),
                    cell(120.0, 40.0),
                );
                ui.add(b, Widget::icon(axion_compositor::Icon::Close, 14.0), Style::default());
                ui.add_label(b, "Import", TextStyle::Callout, Style::default());
                buttons.push(b);
            }
            let card = ui.add(
                root,
                Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(18.0),
                cell(200.0, 80.0),
            );
            ui.add(
                card,
                Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
                cell(80.0, 30.0),
            );
            let seg = ui.add(root, Widget::segmented(0), Style::default());
            ui.add(seg, Widget::container(), cell(100.0, 30.0));
            ui.add(seg, Widget::container(), cell(100.0, 30.0));
            let toggle = ui.add(root, Widget::toggle(true), Style::default());
            let slider = ui.add(
                root,
                Widget::slider(0.4),
                Style { size: Size { width: length(224.0), height: auto() }, ..Default::default() },
            );
            ui.layout();

            let audit = |ui: &mut Ui, when: &str| {
                let mut layers = Vec::new();
                ui.paint(&mut layers);
                let violations = flat_violations(&layers, 2.0);
                assert!(violations.is_empty(), "flat, dark {dark}, {when}: {violations:#?}");
            };
            audit(&mut ui, "at rest");
            for target in buttons.iter().copied().chain([button, toggle, slider, seg]) {
                ui.handle(UiInput::PointerMove(ui.frame(target).center()));
                audit(&mut ui, "hovered");
                ui.handle(UiInput::PointerDown);
                audit(&mut ui, "held");
                ui.handle(UiInput::PointerMove(ui.frame(target).center() + Vec2::new(30.0, 6.0)));
                audit(&mut ui, "dragged");
                ui.handle(UiInput::PointerUp);
            }
        }
    }

    /// The audit does catch what it is about (so the test above means something).
    #[test]
    fn flat_audit_flags_glass_rims_and_knockouts() {
        let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
        let mut content = Content::default();
        content.quads.push(Quad::ring(rect, 20.0, 6.0, [1.0; 4]));
        content.quads.push(Quad::card_shadow(rect, 20.0, 2.0, 10.0, [0.0, 0.0, 0.0, 0.1]));
        content.quads.push(Quad::shadow(rect, 20.0, 8.0, 60.0, [0.0, 0.0, 0.0, 0.5]));
        content.quads.push(Quad::ring(rect, 20.0, 2.0, [1.0; 4]));
        let layers = vec![
            Layer::Glass { shapes: vec![GlassShape::circle(Vec2::ZERO, 10.0)], params: Default::default() },
            Layer::Content(content),
        ];
        assert_eq!(flat_violations(&layers, 2.0).len(), 4);
    }

    #[test]
    fn liquid_mode_still_paints_glass() {
        let mut ui = ui();
        row(&mut ui);
        ui.layout();
        let mut layers = Vec::new();
        ui.paint(&mut layers);
        assert!(layers.iter().any(|l| matches!(l, Layer::Glass { .. })));
    }

    #[test]
    fn flat_mode_stops_presses_stretch_and_merging() {
        let mut ui = flat_ui();
        let (_, button, _) = row(&mut ui);
        ui.layout();
        assert!(ui.press_target(button).is_none(), "no lens, no swell");
        assert_eq!(ui.squash(), 0.0);
        assert_eq!(ui.theme().config().glass.merge_distance(), 0.0);
    }

    #[test]
    fn paint_orders_glass_before_its_content() {
        let mut ui = ui();
        row(&mut ui);
        let mut layers = Vec::new();
        ui.paint(&mut layers);
        let kinds: Vec<&str> = layers
            .iter()
            .map(|l| match l {
                Layer::Glass { .. } => "glass",
                Layer::Content(_) => "content",
                Layer::Image(_) => "image",
            })
            .collect();
        assert_eq!(kinds, vec!["glass", "content"]);
    }

    /// Every icon is drawn after all the glass that could cover it: in the same stack's content
    /// layer, which follows its glass, and no later glass layer reaches over it. So the glass'
    /// blur, refraction and dispersion never touch an icon's pixels.
    fn assert_icons_above_glass(layers: &[Layer<'_>]) {
        let mut icon_layers = 0;
        for (i, layer) in layers.iter().enumerate() {
            let Layer::Content(c) = layer else { continue };
            for icon in &c.icons {
                icon_layers += 1;
                for later in &layers[i + 1..] {
                    let Layer::Glass { shapes, .. } = later else { continue };
                    for shape in shapes {
                        let reach = shape.half_size * shape.stretch.max_element().max(1.0) + Vec2::splat(1.0);
                        let covers =
                            (icon.rect.center() - shape.center).abs().cmple(reach + icon.rect.size / 2.0);
                        assert!(
                            !covers.all(),
                            "glass {:?} lies over the icon {:?} drawn before it",
                            (shape.center, shape.half_size),
                            icon.rect
                        );
                    }
                }
            }
        }
        assert!(icon_layers > 0, "the scene has icons");
    }

    #[test]
    fn icons_are_drawn_above_every_glass_that_could_cover_them() {
        // A bar (glass) holding a glass button with an icon, a glass card with its own button
        // and icon (glass nested in glass), and a plain icon button, at rest and held.
        let mut ui = ui();
        let bar = ui.add(
            ui.root(),
            Widget::container().with_background(Background::Glass(GlassLevel::Regular)).with_radius(CAPSULE),
            Style {
                size: Size { width: length(300.0), height: length(56.0) },
                align_items: AlignItems::CENTER,
                gap: Size { width: length(12.0), height: length(0.0) },
                padding: taffy::Rect {
                    left: length(6.0),
                    right: length(6.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        );
        let button = |ui: &mut Ui, parent: NodeId, id: u32, glass: bool| {
            let mut widget = Widget::container().with_action(ActionId(id)).with_radius(CAPSULE);
            if glass {
                widget = widget.with_background(Background::Glass(GlassLevel::Thin));
            }
            let b = ui.add(
                parent,
                widget,
                Style {
                    size: Size { width: length(44.0), height: length(44.0) },
                    justify_content: JustifyContent::CENTER,
                    align_items: AlignItems::CENTER,
                    ..Default::default()
                },
            );
            ui.add(b, Widget::icon(axion_compositor::Icon::Plus, 20.0), Style::default());
        };
        button(&mut ui, bar, 1, false);
        button(&mut ui, bar, 2, true);
        let card = ui.add(
            bar,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
            Style {
                size: Size { width: length(100.0), height: length(48.0) },
                align_items: AlignItems::CENTER,
                justify_content: JustifyContent::CENTER,
                ..Default::default()
            },
        );
        button(&mut ui, card, 3, false);
        ui.layout();
        for held in [false, true] {
            if held {
                ui.handle(UiInput::PointerMove(Vec2::new(28.0, 28.0)));
                ui.handle(UiInput::PointerDown);
                ui.handle(UiInput::PointerMove(Vec2::new(40.0, 36.0)));
                for _ in 0..30 {
                    ui.animate(1.0 / 60.0);
                }
            }
            let mut layers = Vec::new();
            ui.paint(&mut layers);
            assert_icons_above_glass(&layers);
        }
    }

    #[test]
    fn a_held_button_stretches_its_icon_with_its_glass() {
        let mut ui = ui();
        let (_, button, _) = row(&mut ui);
        ui.add(button, Widget::icon(axion_compositor::Icon::Back, 20.0), Style::default());
        let mut style = ui.style(button);
        style.justify_content = JustifyContent::CENTER;
        style.align_items = AlignItems::CENTER;
        ui.set_style(button, style);
        ui.layout();
        ui.handle(UiInput::PointerMove(Vec2::new(28.0, 28.0)));
        ui.handle(UiInput::PointerDown);
        ui.handle(UiInput::PointerMove(Vec2::new(60.0, 28.0)));
        for _ in 0..40 {
            ui.animate(1.0 / 60.0);
        }
        let mut layers = Vec::new();
        ui.paint(&mut layers);
        assert_icons_above_glass(&layers);
        let lens = layers
            .iter()
            .filter_map(|l| if let Layer::Glass { shapes, .. } = l { shapes.last().copied() } else { None })
            .next_back()
            .expect("the lens");
        let icon = layers
            .iter()
            .filter_map(|l| if let Layer::Content(c) = l { c.icons.first().copied() } else { None })
            .next_back()
            .expect("the icon");
        // Same transform as the glass: scaled by the same amount, but it stays at the rest
        // centre while the lens follows the pointer a little.
        assert!((icon.rect.center() - Vec2::splat(28.0)).length() < 0.01);
        assert!(lens.center.x > 28.5);
        assert!(icon.rect.size.x > 20.0 * 1.2, "scaled with the lens: {:?}", icon.rect.size);
        // Stretched along the pull (x) and narrower across it, like the glass.
        assert!(icon.rect.size.x / icon.rect.size.y > 1.0 + 1e-3, "{:?}", icon.rect.size);
        assert!(lens.stretch.x > 1.0 && lens.stretch.y < 1.0);
    }

    fn glass_and_icons(ui: &Ui) -> (Vec<GlassShape>, Vec<Rect>, usize) {
        let mut layers = Vec::new();
        ui.paint(&mut layers);
        let mut shapes = Vec::new();
        let mut icons = Vec::new();
        let mut quads = 0;
        for layer in &layers {
            match layer {
                Layer::Glass { shapes: s, .. } => shapes.extend(s.iter().copied()),
                Layer::Content(c) => {
                    icons.extend(c.icons.iter().map(|i| i.rect));
                    quads += c.quads.len();
                }
                Layer::Image(_) => {}
            }
        }
        (shapes, icons, quads)
    }

    /// The stretch of a held button at a pointer distance: (icon size ratio x/y, glass stretch).
    fn pulled(distance: f32) -> (f32, f32) {
        let mut ui = ui();
        let (_, button, _) = row(&mut ui);
        ui.add(button, Widget::icon(axion_compositor::Icon::Back, 20.0), Style::default());
        let mut style = ui.style(button);
        style.justify_content = JustifyContent::CENTER;
        style.align_items = AlignItems::CENTER;
        ui.set_style(button, style);
        ui.layout();
        ui.handle(UiInput::PointerMove(Vec2::new(28.0, 28.0)));
        ui.handle(UiInput::PointerDown);
        ui.handle(UiInput::PointerMove(Vec2::new(28.0 + distance, 28.0)));
        for _ in 0..200 {
            ui.animate(1.0 / 60.0);
        }
        let (shapes, icons, _) = glass_and_icons(&ui);
        (icons[0].size.x / icons[0].size.y, shapes[1].stretch.x)
    }

    #[test]
    fn the_drag_stretch_grows_with_distance_but_saturates_at_the_old_maximum() {
        let max = 1.0 + axion_config::Config::default().glass.drag_stretch_max;
        let (_, near) = pulled(5.0);
        let (_, mid) = pulled(30.0);
        let (_, far) = pulled(150.0);
        let (icon_far, huge) = pulled(5000.0);
        assert!(near > 1.0 && near < mid && mid < far && far <= huge, "{near} {mid} {far} {huge}");
        // Resistance: the second 25 px add less than the first 25.
        assert!((mid - near) / 25.0 < (near - 1.0) / 5.0, "per-px gain falls");
        assert!((huge - max).abs() < 0.002, "asymptote {huge} vs {max}");
        // The icon is deformed by the same amount (its aspect, not just its place).
        assert!(icon_far > 1.0 + (max - 1.0) * 0.8, "{icon_far}");
    }

    #[test]
    fn no_drag_stretch_under_reduce_motion_or_flat() {
        let mut config = axion_config::Config::default();
        config.glass.reduce_motion = true;
        let mut ui = Ui::new(Theme::new(&config, false), 1.0);
        ui.set_viewport(Vec2::new(800.0, 600.0), 1.0);
        let (_, button, _) = row(&mut ui);
        ui.add(button, Widget::icon(axion_compositor::Icon::Back, 20.0), Style::default());
        ui.layout();
        ui.handle(UiInput::PointerMove(Vec2::new(28.0, 28.0)));
        ui.handle(UiInput::PointerDown);
        ui.handle(UiInput::PointerMove(Vec2::new(280.0, 28.0)));
        for _ in 0..120 {
            ui.animate(1.0 / 60.0);
        }
        let (shapes, _, _) = glass_and_icons(&ui);
        assert!(shapes.iter().all(|s| (s.stretch.x - 1.0).abs() < 1e-5), "{:?}", shapes);
    }

    #[test]
    fn a_pressed_button_is_a_lens_carrying_its_icon() {
        let mut ui = ui();
        let (_, button, _) = row(&mut ui);
        ui.add(button, Widget::icon(axion_compositor::Icon::Back, 20.0), Style::default());
        let mut style = ui.style(button);
        style.justify_content = JustifyContent::CENTER;
        style.align_items = AlignItems::CENTER;
        ui.set_style(button, style);
        ui.layout();
        let (rest_shapes, rest_icons, _) = glass_and_icons(&ui);
        assert_eq!(rest_shapes.len(), 1);
        ui.handle(UiInput::PointerMove(Vec2::new(28.0, 28.0)));
        ui.handle(UiInput::PointerDown);
        ui.handle(UiInput::PointerMove(Vec2::new(48.0, 28.0)));
        for _ in 0..60 {
            ui.animate(1.0 / 60.0);
        }
        let (shapes, icons, quads) = glass_and_icons(&ui);
        // The bar's glass and the button's lens, in one pass, so they flow together.
        assert_eq!(shapes.len(), 2);
        let lens = shapes[1];
        assert!(lens.half_size.x > 22.0 * 1.3, "it swells");
        assert!(lens.center.x > 28.5, "it follows the pointer");
        assert!(lens.glow > 0.0, "it is lighter");
        // No plain highlight: the lens is the feedback.
        assert_eq!(quads, 0);
        // The icon grows and stretches with the glass but stays at its rest place.
        let (rest, held) = (rest_icons[0], icons[0]);
        assert!((rest.center() - Vec2::splat(28.0)).length() < 1.0);
        assert!((held.center() - rest.center()).length() < 0.01, "stays: {:?}", held.center());
        assert!(held.size.x > rest.size.x * 1.3);
        // Released, it springs back into the bar.
        ui.handle(UiInput::PointerUp);
        for _ in 0..120 {
            ui.animate(1.0 / 60.0);
        }
        assert_eq!(glass_and_icons(&ui).0.len(), 1);
    }

    #[test]
    fn glass_pressed_whole_is_one_lens_carrying_all_its_buttons() {
        let mut ui = ui();
        // A capsule of three buttons (the toolbar's nav group), one element.
        let capsule = ui.add(
            ui.root(),
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Thin))
                .with_radius(CAPSULE)
                .pressed_whole(),
            Style {
                size: Size { width: length(140.0), height: length(44.0) },
                padding: taffy::Rect {
                    left: length(4.0),
                    right: length(4.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        );
        for id in 1..=3 {
            let button = ui.add(
                capsule,
                Widget::container().with_action(ActionId(id)).with_radius(CAPSULE),
                Style {
                    size: Size { width: length(44.0), height: length(44.0) },
                    justify_content: JustifyContent::CENTER,
                    align_items: AlignItems::CENTER,
                    ..Default::default()
                },
            );
            ui.add(button, Widget::icon(axion_compositor::Icon::Back, 20.0), Style::default());
        }
        ui.layout();
        let (_, rest_icons, _) = glass_and_icons(&ui);
        ui.handle(UiInput::PointerMove(Vec2::new(26.0, 22.0)));
        ui.handle(UiInput::PointerDown);
        ui.handle(UiInput::PointerMove(Vec2::new(46.0, 22.0)));
        for _ in 0..60 {
            ui.animate(1.0 / 60.0);
        }
        let (shapes, icons, quads) = glass_and_icons(&ui);
        // One shape: the whole capsule is the lens, no lens of the button inside it.
        assert_eq!(shapes.len(), 1);
        let lens = shapes[0];
        assert!(lens.half_size.x > 70.0, "it swells");
        assert!(lens.center.x > 70.5, "it follows the pointer");
        assert!(lens.glow > 0.0, "it is lighter");
        assert_eq!(quads, 0, "no plain highlight on the pressed button");
        // Every icon stays at its rest place (about the capsule's rest centre): it swells and
        // stretches with the glass but is never carried along with the pointer.
        for (rest, held) in rest_icons.iter().zip(&icons) {
            let expected = 70.0 + (rest.center().x - 70.0) * (lens.half_size.x / 70.0);
            assert!((held.center().x - expected).abs() < 3.0, "{rest:?} → {held:?}");
            assert!((held.center().y - rest.center().y).abs() < 0.5);
        }
        ui.handle(UiInput::PointerUp);
        for _ in 0..120 {
            ui.animate(1.0 / 60.0);
        }
        let (shapes, icons, _) = glass_and_icons(&ui);
        assert!((shapes[0].half_size.x - 70.0).abs() < 0.5);
        assert!((icons[0].center() - rest_icons[0].center()).length() < 0.5);
    }

    #[test]
    fn joining_glass_is_neither_hit_nor_a_surface() {
        let mut ui = ui();
        let (bar, _, _) = row(&mut ui);
        let join = ui.add(
            ui.root(),
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).as_join(),
            Style {
                position: Position::Absolute,
                size: Size { width: length(100.0), height: length(100.0) },
                ..Default::default()
            },
        );
        ui.layout();
        let p = Vec2::new(200.0, 28.0);
        assert_ne!(ui.hit(Vec2::new(50.0, 80.0)), Some(join));
        assert_eq!(ui.glass_at(ui.root(), p, &[]).map(|g| g.0), Some(ui.frame(bar)));
        assert!(ui.glass_at(ui.root(), Vec2::new(50.0, 80.0), &[]).is_none());
    }
}
