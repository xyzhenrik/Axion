//! Axion's retained UI: a tree of widgets laid out with taffy (flexbox), drawn as Liquid Glass,
//! shapes, text and icons through the compositor.
//!
//! The app builds the tree once and updates it in place (`set_text`, `set_visible`, …).
//! Input goes through [`Ui::handle`], which hit-tests, tracks hover/press/focus and edits
//! text fields, and reports [`UiEvent`]s. Layout is in logical pixels; painting converts to
//! physical pixels with the window's scale factor.

mod editor;
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
const TOGGLE_SIZE: Vec2 = Vec2::new(51.0, 31.0);
const SLIDER_KNOB: f32 = 24.0;
const SLIDER_TRACK: f32 = 4.0;
/// Below this opacity a widget (and its subtree) isn't drawn.
const MIN_VISIBLE: f32 = 0.01;
/// How much glass swells while a button on it is held.
const PRESS_SCALE: f32 = 1.06;
const PRESS_SETTLED: f32 = 0.001;
/// Fading glass is grouped into this many opacity steps (each step is one glass pass).
const GLASS_FADE_STEPS: f32 = 24.0;

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
    press_scale: HashMap<NodeId, Animated<f32>>,
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
        let family_changed = theme.font_family() != self.theme.font_family();
        self.dark_theme = Theme::new(theme.config(), true);
        self.theme = theme;
        if family_changed {
            self.reshape_all();
        }
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
        let nodes = self.descendants(self.root);
        for node in nodes {
            let Some(widget) = self.tree.get_node_context_mut(node) else { continue };
            match &mut widget.kind {
                WidgetKind::Label(label) => {
                    label.buffer = new_buffer(self.fonts.get_mut(), label.style, self.scale);
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
                    field.buffer = new_buffer(self.fonts.get_mut(), field.style, self.scale);
                    field.placeholder = new_buffer(self.fonts.get_mut(), field.style, self.scale);
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
        let widget = Widget::label(self.fonts.get_mut(), &family, self.scale, text, style);
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
        let widget = Widget::text_field(self.fonts.get_mut(), &family, self.scale, placeholder, style);
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
        if let Some(Widget { kind: WidgetKind::Image { view, size }, .. }) =
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
                WidgetKind::TextField(_) | WidgetKind::Toggle { .. } | WidgetKind::Slider { .. }
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
            || matches!(widget.kind, WidgetKind::Toggle { .. } | WidgetKind::Slider { .. })
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
                if let Some(slider) = self.dragging {
                    events.extend(self.slide_to(slider, p));
                    return (true, events);
                }
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
                if self.dragging.take().is_some() {
                    return (true, events);
                }
                let pressed = self.pressed.take();
                if let Some(glass) = pressed.and_then(|n| self.glass_of(n)) {
                    self.press_glass(glass, 1.0);
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
                let value = if *on { 0.0 } else { 1.0 };
                self.set_value(hit, value);
                events.extend(self.blur());
                events.push(UiEvent::ValueChanged { node: hit, value });
                return true;
            }
            Some(WidgetKind::Slider { .. }) => {
                events.extend(self.blur());
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
            self.press_glass(glass, PRESS_SCALE);
        }
        true
    }

    /// Moves a slider's knob under the pointer.
    fn slide_to(&mut self, slider: NodeId, p: Vec2) -> Option<UiEvent> {
        let frame = self.frame(slider);
        let inset = SLIDER_KNOB / 2.0;
        let span = (frame.size.x - 2.0 * inset).max(1.0);
        let value = ((p.x - frame.origin.x - inset) / span).clamp(0.0, 1.0);
        match self.widget(slider).map(|w| &w.kind) {
            Some(WidgetKind::Slider { value: v }) if (*v - value).abs() > f32::EPSILON => {
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
        match self.tree.get_node_context_mut(node).map(|w| &mut w.kind) {
            Some(WidgetKind::Toggle { on }) => *on = value > 0.5,
            Some(WidgetKind::Slider { value: v }) => *v = value.clamp(0.0, 1.0),
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
            if matches!(self.widget(node)?.background, Background::Glass(_)) {
                return Some(node);
            }
            node = self.tree.parent(node)?;
        }
    }

    fn press_glass(&mut self, glass: NodeId, scale: f32) {
        let spring = self.press_spring();
        let anim = self.press_scale.entry(glass).or_insert_with(|| Animated::new(1.0, spring));
        anim.spring = spring;
        anim.target = scale;
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
        self.press_scale.retain(|_, anim| {
            anim.step(dt);
            !(anim.target == 1.0 && anim.is_settled(PRESS_SETTLED))
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
            let mut groups: Vec<(GlassLevel, f32)> = Vec::new();
            for item in &stack.glass {
                if !groups.contains(&(item.level, item.fade)) {
                    groups.push((item.level, item.fade));
                }
            }
            groups.sort_by_key(|(level, fade)| (*level as u8, (fade * GLASS_FADE_STEPS) as u32));
            for (level, fade) in groups {
                let shapes: Vec<GlassShape> = stack
                    .glass
                    .iter()
                    .filter(|g| g.level == level && g.fade == fade)
                    .map(|g| g.shape)
                    .collect();
                let mut params = self.paint_theme().glass(level, s);
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

        let mut child_depth = depth;
        let content = &mut stacks[depth].content;
        match widget.background {
            Background::Glass(level) => {
                let k = self.press_scale.get(&node).map_or(1.0, |a| a.value);
                let shape = GlassShape::rounded_rect(bg_px.center(), bg_px.size * 0.5 * k, bg_radius * k);
                stacks[depth + 1].glass.push(GlassItem { level, fade: quantize_fade(fade), shape });
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
            WidgetKind::Image { view: Some(view), size } => {
                // Cover the frame, cropping the texture's bottom (or sides) if aspects differ.
                let tex = Vec2::new(size.0 as f32, size.1 as f32);
                let scale = (px.size / tex).max_element();
                let visible = (px.size / (tex * scale)).min(Vec2::ONE);
                let uv = Rect { origin: Vec2::new((1.0 - visible.x) / 2.0, 0.0), size: visible };
                content.images.push(ImageDraw { texture: view, rect: px, uv, radius, opacity: alpha });
            }
            WidgetKind::Image { view: None, .. } => {}
            WidgetKind::Toggle { on } => {
                let base = if *on { theme.color(Paint::Accent) } else { theme.color(Paint::Label) };
                let track = faded([base[0], base[1], base[2], if *on { 1.0 } else { 0.16 }]);
                let size = TOGGLE_SIZE * s;
                let origin = px.origin + (px.size - size) / 2.0;
                content.quads.push(Quad::fill(Rect { origin, size }, size.y / 2.0, track));
                let knob = size.y - 4.0 * s;
                let x = if *on { origin.x + size.x - 2.0 * s - knob } else { origin.x + 2.0 * s };
                let knob_rect = Rect::new(x, origin.y + 2.0 * s, knob, knob);
                content.quads.push(Quad::shadow(
                    knob_rect,
                    knob / 2.0,
                    2.0 * s,
                    6.0 * s,
                    [0.0, 0.0, 0.0, 0.18 * fade],
                ));
                content.quads.push(Quad::fill(knob_rect, knob / 2.0, faded([1.0, 1.0, 1.0, 1.0])));
            }
            WidgetKind::Slider { value } => {
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
                let knob_rect = Rect::new(px.origin.x + width * value, y - knob / 2.0, knob, knob);
                content.quads.push(Quad::shadow(
                    knob_rect,
                    knob / 2.0,
                    2.0 * s,
                    6.0 * s,
                    [0.0, 0.0, 0.0, 0.2 * fade],
                ));
                content.quads.push(Quad::fill(knob_rect, knob / 2.0, faded([1.0, 1.0, 1.0, 1.0])));
            }
        }

        for child in self.tree.children(node).unwrap_or_default() {
            self.paint_node(child, child_depth, dim, fade, stacks);
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
}

struct GlassItem {
    level: GlassLevel,
    fade: f32,
    shape: GlassShape,
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
        let (_, events) = ui.handle(UiInput::PointerDown);
        assert!(events.contains(&UiEvent::ValueChanged { node: toggle, value: 1.0 }));

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
