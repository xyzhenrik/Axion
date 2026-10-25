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
pub use theme::{GlassLevel, Paint, TextStyle, Theme};
pub use widget::{ActionId, Align, Background, CAPSULE, Shadow, Widget, WidgetKind};
use widget::{caret_x, index_at, measure, new_buffer, set_buffer_text};

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
/// Content on glass whose background is animated (`Widget::visual_frame`: popovers dripping
/// out of a button, alerts scaling in) is mapped onto that background and shows only once it
/// has grown to this share of its layout size …
const CONTENT_REVEAL_FROM: f32 = 0.55;
/// … and fully from this share on, so glass and content appear and vanish together.
const CONTENT_REVEAL_TO: f32 = 0.9;
/// Fading glass is grouped into this many opacity steps (each step is one glass pass).
const GLASS_FADE_STEPS: f32 = 24.0;
/// How strongly tinted glass takes its colour (it still refracts, blurs and catches light).
const TINTED_GLASS_STRENGTH: f32 = 0.78;
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
    /// Glass that swells while a button on it is held, springing back on release.
    press_scale: HashMap<NodeId, Press>,
    /// The glass being held and where the pointer went down (for the pull toward the pointer).
    press_anchor: Option<(NodeId, Vec2)>,
    /// Switch and slider knobs that are held or still animating.
    knobs: HashMap<NodeId, Knob>,
    focus: Option<NodeId>,
    /// A slider being dragged.
    dragging: Option<NodeId>,
    caret_epoch: Instant,
    needs_layout: bool,
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
            focus: None,
            dragging: None,
            press_scale: HashMap::new(),
            press_anchor: None,
            knobs: HashMap::new(),
            caret_epoch: Instant::now(),
            needs_layout: true,
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

    /// How much moving glass may squash (`glass.squash`; none with Reduce Motion).
    fn squash(&self) -> f32 {
        let glass = &self.theme.config().glass;
        if glass.reduce_motion { 0.0 } else { glass.squash.max(0.0) }
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
        for n in self.descendants(node).into_iter().rev() {
            if self.focus == Some(n) {
                self.focus = None;
            }
            if self.hover == Some(n) {
                self.hover = None;
            }
            if self.pressed == Some(n) {
                self.pressed = None;
            }
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
        let scale = self.scale;
        let available = Size {
            width: AvailableSpace::Definite(self.size.x),
            height: AvailableSpace::Definite(self.size.y),
        };
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
                        let (w, h) = measure(&label.buffer);
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
        self.update_frames(self.root, Vec2::ZERO);
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
                    label.buffer.set_size(Some(frame.size.x * scale), Some(frame.size.y * scale));
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
                    if !focused {
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
        let children = self.tree.children(node).unwrap_or_default();
        for child in children.into_iter().rev() {
            if let Some(hit) = self.hit_in(child, p) {
                return Some(hit);
            }
        }
        let solid = widget.background != Background::None
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

    /// The button under the pointer, if any.
    pub fn hovered(&self) -> Option<NodeId> {
        self.hover
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
                if let Some(node) = self.dragging.take() {
                    events.extend(self.release_knob(node));
                    return (true, events);
                }
                let pressed = self.pressed.take();
                self.press_anchor = None;
                if let Some(glass) = pressed.and_then(|n| self.glass_of(n)) {
                    self.press_glass(glass, false);
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
                // Place the caret where clicked.
                let scale = self.scale;
                if let Some(Widget { kind: WidgetKind::TextField(field), frame, .. }) = self.widget(hit) {
                    let x = (p.x - frame.origin.x + field.scroll_x) * scale;
                    let index = field.real_index(index_at(&field.buffer, x));
                    self.edit_field(hit, |e| e.move_to(index, false));
                }
            } else {
                events.extend(self.focus(hit));
            }
        } else {
            events.extend(self.blur());
        }
        self.pressed = self.actionable(hit);
        if let Some(glass) = self.pressed.and_then(|n| self.glass_of(n)) {
            self.press_glass(glass, true);
            self.press_anchor = Some((glass, p));
        }
        true
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
            Spring::new(motion.liquid.response * motion.speed, motion.liquid.damping)
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
        Some(UiEvent::ValueChanged { node, value: index as f32 })
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

    fn press_glass(&mut self, glass: NodeId, pressed: bool) {
        let size = self.frame(glass).size;
        // Only surfaces big in both directions (sheets, the sidebar) stay still; bars swell.
        if size.min_element() > PRESS_MAX_SIZE {
            return;
        }
        let swell = (PRESS_GROW_PX / (size.max_element() / 2.0).max(1.0)).min(PRESS_SCALE - 1.0);
        let spring = self.press_spring();
        let press = self.press_scale.entry(glass).or_insert_with(|| Press {
            scale: Animated::new(1.0, spring),
            offset: Animated::new(Vec2::ZERO, spring),
        });
        press.scale.spring = spring;
        press.offset.spring = spring;
        press.scale.target = if pressed { 1.0 + swell } else { 1.0 };
        if !pressed {
            press.offset.target = Vec2::ZERO;
        }
    }

    /// While glass is held, it follows the pointer a little (rubber band) when dragged away.
    fn pull_glass(&mut self, p: Vec2) {
        let Some((glass, anchor)) = self.press_anchor else { return };
        if let Some(press) = self.press_scale.get_mut(&glass) {
            press.offset.target = axion_physics::rubber_band_vec(p - anchor, PULL_LIMIT);
        }
    }

    fn press_spring(&self) -> Spring {
        let motion = &self.theme.config().motion;
        if self.theme.config().glass.reduce_motion {
            Spring::new(0.15, 1.0)
        } else {
            Spring::new(motion.snappy.response * motion.speed, motion.liquid.damping)
        }
    }

    /// Advances UI animations (glass press feedback). Call once per frame.
    pub fn animate(&mut self, dt: f32) {
        self.press_scale.retain(|_, press| {
            press.scale.step(dt);
            press.offset.step(dt);
            !(press.scale.target == 1.0
                && press.scale.is_settled(PRESS_SETTLED)
                && press.offset.target == Vec2::ZERO
                && press.offset.is_settled(0.05))
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
                    params.tint = [tint[0], tint[1], tint[2], TINTED_GLASS_STRENGTH];
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
        let press_xf = self.press_scale.get(&node).map(|press| {
            let offset = press.offset.value * s;
            let pull = offset.length();
            let e = (pull * PULL_STRETCH / bg_px.size.max_element().max(1.0)).min(self.squash());
            let dir = if pull > 0.0 { (offset / pull).abs() } else { Vec2::ZERO };
            // Along the pull it stretches, across it it narrows a little (like the glass).
            let along = Vec2::ONE + dir * e;
            let across = Vec2::ONE - Vec2::new(dir.y, dir.x) * e * 0.5;
            PressXf {
                scale: press.scale.value,
                offset,
                stretch: Vec2::new(1.0 + e, 1.0 / (1.0 + e)),
                angle: offset.y.atan2(offset.x),
                content_scale: press.scale.value * along * across,
            }
        });
        let mark = press_xf.as_ref().map(|_| marks(stacks));
        // An animated background carries its content along (mapped from the layout frame onto
        // it), and the content fades with the background's growth: both share one value.
        let (frame_xf, reveal) = match widget.visual_frame {
            Some(_) if px.size.min_element() > 0.0 => {
                let ratio = bg_px.size / px.size;
                let xf = PressXf {
                    center: px.center(),
                    scale: 1.0,
                    offset: Vec2::ZERO,
                    stretch: Vec2::ONE,
                    angle: 0.0,
                    content_scale: ratio,
                    content_offset: bg_px.center() - px.center(),
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
        match widget.background {
            Background::Glass(_) | Background::TintedGlass(..) => {
                let (level, tint) = match widget.background {
                    Background::TintedGlass(level, paint) => (level, Some(theme.color(paint))),
                    Background::Glass(level) => (level, None),
                    _ => unreachable!("matched above"),
                };
                let mut shape = GlassShape::rounded_rect(bg_px.center(), bg_px.size * 0.5, bg_radius);
                shape.tint_opacity = widget.glass_density.map(|l| theme.glass_opacity(l));
                if let Some(xf) = &press_xf {
                    shape.center += xf.offset;
                    shape.half_size *= xf.scale;
                    shape.corner_radius *= xf.scale;
                    shape.stretch = xf.stretch;
                    shape.stretch_angle = xf.angle;
                }
                stacks[depth + 1].glass.push(GlassItem { level, fade: quantize_fade(fade), tint, shape });
                child_depth = depth + 1;
            }
            Background::Fill(paint, opacity) => {
                if let Some(shadow) = widget.shadow {
                    content.quads.push(Quad::shadow(
                        bg_px,
                        bg_radius,
                        shadow.offset_y * s,
                        shadow.blur * s,
                        [0.0, 0.0, 0.0, shadow.opacity * fade],
                    ));
                }
                let mut color = theme.color(paint);
                color[3] *= opacity * fade;
                content.quads.push(Quad::fill(bg_px, bg_radius, color));
            }
            Background::None => {}
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
            let feedback = if self.pressed == Some(node) {
                PRESS_ALPHA
            } else if self.hover == Some(node) {
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
                    rect: Rect { origin, size: Vec2::splat(side) },
                    color,
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
                    self.paint_pill(rect, rest, lens, fade, depth, stacks);
                }
            }
            WidgetKind::Toggle { on } => {
                let held = self.knobs.get(&node);
                let pos = held.map_or(if *on { 1.0 } else { 0.0 }, |k| k.pos.value);
                let lens = held.map_or(0.0, |k| k.lens.value.clamp(0.0, 1.0));
                let speed = held.map_or(0.0, |k| k.speed);
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
                self.paint_knob(center, knob, lens, speed, fade, depth, stacks);
            }
            WidgetKind::Slider { value, line: true } => {
                let held = self.dragging == Some(node);
                let h = if held { LINE_SLIDER_HELD } else { LINE_SLIDER } * s;
                let y = px.origin.y + px.size.y / 2.0;
                let track = Rect::new(px.origin.x, y - h / 2.0, px.size.x, h);
                let label = theme.color(Paint::Label);
                content.quads.push(Quad::fill(track, h / 2.0, faded([label[0], label[1], label[2], 0.28])));
                let filled = Rect { size: Vec2::new(px.size.x * value, h), ..track };
                content.quads.push(Quad::fill(filled, h / 2.0, faded([label[0], label[1], label[2], 0.95])));
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
                let speed = held.map_or(0.0, |k| k.speed);
                let center = Vec2::new(px.origin.x + inset + width * value, y);
                self.paint_knob(center, Vec2::splat(knob), lens, speed, fade, depth, stacks);
            }
        }

        if widget.clip {
            // The subtree as its own layers, each cut to this widget's frame.
            let mut sub: Vec<Stack<'a>> = Vec::new();
            for child in self.tree.children(node).unwrap_or_default() {
                self.paint_node(child, child_depth, dim, child_fade, &mut sub);
            }
            let mut layers = Vec::new();
            self.flush(sub, &mut layers);
            for layer in &mut layers {
                if let Layer::Content(content) = layer {
                    content.clip = Some(px);
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

    /// A switch or slider knob (physical px): opaque white at rest, a clear glass lens while
    /// held (`lens` 0→1), stretched along fast motion.
    #[allow(clippy::too_many_arguments)]
    fn paint_knob(
        &self,
        center: Vec2,
        knob: Vec2,
        lens: f32,
        speed: f32,
        fade: f32,
        depth: usize,
        stacks: &mut [Stack<'_>],
    ) {
        let s = self.scale;
        let grow = Vec2::ONE + (LENS_GROWTH - Vec2::ONE) * lens;
        let stretch = (speed.abs() * LENS_SQUASH).min(self.squash() * LENS_SQUASH_RATIO) * lens;
        let size = knob * grow * Vec2::new(1.0 + stretch, 1.0 / (1.0 + stretch).sqrt());
        let rect = Rect { origin: center - size / 2.0, size };
        let radius = size.y / 2.0;
        // The opaque knob fades out as the lens forms, and back in as it frosts over.
        let opaque = (1.0 - lens) * fade;
        if opaque > MIN_VISIBLE {
            let content = &mut stacks[depth].content;
            content.quads.push(Quad::shadow(rect, radius, 2.0 * s, 6.0 * s, [0.0, 0.0, 0.0, 0.18 * opaque]));
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
    fn paint_pill(
        &self,
        rect: Rect,
        rest: [f32; 4],
        lens: f32,
        fade: f32,
        depth: usize,
        stacks: &mut [Stack<'_>],
    ) {
        let s = self.scale;
        let grow = Vec2::ONE + (Vec2::new(1.12, 1.25) - Vec2::ONE) * lens;
        let size = rect.size * grow;
        let center = rect.center();
        let radius = size.y / 2.0;
        let r = Rect { origin: center - size / 2.0, size };
        let opaque = (1.0 - lens) * fade;
        if opaque > MIN_VISIBLE {
            let content = &mut stacks[depth].content;
            content.quads.push(Quad::shadow(r, radius, 1.0 * s, 4.0 * s, [0.0, 0.0, 0.0, 0.10 * opaque]));
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
}

impl PressXf {
    fn rect(&self, r: Rect) -> Rect {
        let size = r.size * self.content_scale;
        Rect { origin: r.center() - size / 2.0, size }
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
}

fn marks(stacks: &[Stack<'_>]) -> Vec<Mark> {
    stacks
        .iter()
        .map(|s| Mark {
            quads: s.content.quads.len(),
            images: s.content.images.len(),
            icons: s.content.icons.len(),
            text: s.content.text.len(),
        })
        .collect()
}

/// Moves what a held glass's subtree painted (from `from_depth` on) along with the glass.
fn transform_since(stacks: &mut [Stack<'_>], mark: &[Mark], xf: &PressXf, from_depth: usize) {
    for (d, stack) in stacks.iter_mut().enumerate().skip(from_depth) {
        let m = mark.get(d).unwrap_or(&Mark { quads: 0, images: 0, icons: 0, text: 0 });
        let c = &mut stack.content;
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
            let about = |p: Vec2| center + (p - center) * u;
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
}
