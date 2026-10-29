//! Widget data stored in each layout node.

use axion_compositor::glyphon::{self, Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Weight};
use axion_compositor::{Icon, IconWeight, Rect};

use crate::editor::Editor;
use crate::theme::{GlassLevel, Paint, TextStyle};

/// Identifies what a clickable widget does; the app decides what the ids mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActionId(pub u32);

/// Corner radius that makes a capsule or circle, whatever the size.
pub const CAPSULE: f32 = f32::INFINITY;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Background {
    None,
    Glass(GlassLevel),
    /// Glass tinted strongly with a colour (a primary button: still refracts and catches light).
    TintedGlass(GlassLevel, Paint),
    /// A solid fill with an opacity multiplier.
    Fill(Paint, f32),
    /// A fill with soft edges (blur radius in logical px) at an opacity: it is full over its
    /// inside and fades out over its edge. Menus use it as a plate on their glass, so the rows
    /// read cleanly while the glass at the edges still refracts.
    Soft(Paint, f32, f32),
}

/// A drop shadow below a filled widget (black, CSS-like offset and blur).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub offset_y: f32,
    pub blur: f32,
    pub opacity: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    Center,
}

pub struct Label {
    pub(crate) text: String,
    pub(crate) buffer: Buffer,
    pub style: TextStyle,
    pub paint: Paint,
    pub align: Align,
    /// How many lines the text may wrap onto (1 = never wraps; what does not fit ends in an
    /// ellipsis).
    pub(crate) max_lines: usize,
}

pub struct TextField {
    pub(crate) editor: Editor,
    pub(crate) buffer: Buffer,
    pub(crate) placeholder: Buffer,
    pub(crate) placeholder_text: String,
    pub style: TextStyle,
    pub paint: Paint,
    /// Horizontal scroll so the caret stays visible (logical px).
    pub(crate) scroll_x: f32,
    /// Shows bullets instead of the text and refuses copy/cut (passwords).
    pub(crate) secure: bool,
}

/// The bullet shown for each character of a secure field.
pub(crate) const BULLET: char = '•';

impl TextField {
    /// The text as displayed (bullets for secure fields).
    pub(crate) fn display_text(&self) -> String {
        if self.secure {
            BULLET.to_string().repeat(self.editor.text().chars().count())
        } else {
            self.editor.text().to_owned()
        }
    }

    /// Byte index in the displayed text for a byte index in the real text.
    pub(crate) fn display_index(&self, index: usize) -> usize {
        if self.secure { self.editor.text()[..index].chars().count() * BULLET.len_utf8() } else { index }
    }

    /// Byte index in the real text for a byte index in the displayed text.
    pub(crate) fn real_index(&self, index: usize) -> usize {
        if !self.secure {
            return index;
        }
        let chars = index / BULLET.len_utf8();
        self.editor.text().char_indices().nth(chars).map_or(self.editor.text().len(), |(i, _)| i)
    }
}

pub enum WidgetKind {
    Container,
    Label(Label),
    Icon {
        icon: Icon,
        weight: IconWeight,
        size: f32,
        paint: Paint,
    },
    TextField(TextField),
    /// A thin bar filled to `value` (0–1), e.g. page load progress.
    Progress {
        value: f32,
        paint: Paint,
    },
    /// An on/off switch (51×31, like iOS).
    Toggle {
        on: bool,
    },
    /// A horizontal slider with a value in 0–1. A `line` slider has no knob: a thin track
    /// that fills with the label colour and thickens while dragged (media progress, volume).
    Slider {
        value: f32,
        line: bool,
    },
    /// A segmented control: its children are the segments; the `selected` one sits on a pill
    /// that turns into a glass lens while held and can be slid to another segment.
    Segmented {
        selected: usize,
    },
    /// A texture scaled to cover the frame, cropped from the top (tab thumbnails, favicons).
    Image {
        view: Option<wgpu::TextureView>,
        size: (u32, u32),
        /// Only this part of the texture (0–1 coordinates), stretched over the frame.
        crop: Option<axion_compositor::Rect>,
    },
}

/// A button's hover label: what it does, plus the keymap action whose shortcut is appended
/// (resolved when the tooltip is shown, so remapped keys show up).
#[derive(Clone, Debug, PartialEq)]
pub struct Tooltip {
    pub label: String,
    pub action: Option<axion_config::Action>,
}

/// How a glass background is drawn without glass (flat mode). By default a card at the top
/// level and an inset inside another card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlatRole {
    #[default]
    Auto,
    /// Nothing: the widget only groups content that lies on the toolbar's surface.
    Bare,
    /// A field sunk into the surface it lies on (the address field on the toolbar).
    Field,
    /// A pill raised from a tinted strip (the open tab on the tab strip).
    Raised,
}

pub struct Widget {
    pub kind: WidgetKind,
    pub background: Background,
    /// Logical pixels; [`CAPSULE`] for fully rounded ends.
    pub radius: f32,
    pub shadow: Option<Shadow>,
    /// Makes the widget clickable.
    pub action: Option<ActionId>,
    pub enabled: bool,
    /// Shows the hover/press highlight over the widget. Off for backdrops (a modal sheet's
    /// dimmed window): their highlight would cover the whole window and flash it whenever the
    /// pointer crosses a gap between the sheet's rows.
    pub feedback: bool,
    /// Draws the widget in its "on" state (e.g. the sidebar toggle while the sidebar is open).
    pub selected: bool,
    /// Fades the widget and everything inside it (0–1); glass loses blur, tint and light.
    pub opacity: f32,
    /// An icon's clockwise rotation in degrees (a chevron turning as a section folds).
    pub rotation: f32,
    /// Draws the widget's own background (glass, fill, shadow) in this rectangle (absolute,
    /// logical px) instead of its layout frame — for animating glass without relayout.
    pub visual_frame: Option<Rect>,
    /// Glass drawn in its own level's pass (so it flows into that level's glass) but tinted
    /// as densely as this level: popovers that grow out of thin bar glass and carry text.
    pub glass_density: Option<GlassLevel>,
    /// A glass control without an action of its own (the pill around a slider) that is tinted
    /// like the glass buttons beside it, so a row of controls over a video reads as one.
    pub button_tint: bool,
    /// This glass's own smooth-union radius (logical px) where it flows into the glass drawn
    /// before it, when wider than `glass.merge_distance` (a popover's bridge to its bar).
    pub glass_merge: Option<f32>,
    /// Glass that only joins other glass (a popover's bridge to its bar): it lies over their
    /// buttons, so the pointer passes through it (its children are still hit); it is no
    /// surface of its own (see `Ui::glass_at`); and it has no tint of its own, so across it
    /// the tint blends from the glass it joins.
    pub join_only: bool,
    /// How a glass background is drawn in flat mode (see [`FlatRole`]); ignored with glass.
    pub flat_role: FlatRole,
    /// Glass that is one element (the toolbar's nav capsule, the address field): pressing any
    /// button in it turns the whole glass into one lens that swells, follows the pointer and
    /// stretches, carrying all its content along, instead of the button becoming a lens of
    /// its own.
    pub press_whole: bool,
    /// Lights up under the pointer. Off for glass buttons that are pure glass (the media
    /// player's circles): their only feedback is their lens when pressed.
    pub hover_fill: bool,
    /// Children are drawn only inside this widget's frame (scrolling lists).
    pub clip: bool,
    /// A line slider's track thickness at rest (logical px); `None` is the default thin line.
    /// The widget's own frame (its height) is what grabs the pointer, not the track.
    pub track: Option<f32>,
    /// Shown in a glass label after the pointer rests on the widget (see `Ui::hovered_tooltip`).
    pub tooltip: Option<Tooltip>,
    /// The pointer passes through this widget and everything inside it (a tab that is closing).
    pub passthrough: bool,
    /// Absolute frame in logical pixels, updated by layout.
    pub(crate) frame: Rect,
}

impl Widget {
    fn new(kind: WidgetKind) -> Self {
        Self {
            kind,
            background: Background::None,
            radius: 0.0,
            shadow: None,
            action: None,
            enabled: true,
            feedback: true,
            selected: false,
            opacity: 1.0,
            rotation: 0.0,
            visual_frame: None,
            glass_density: None,
            button_tint: false,
            glass_merge: None,
            join_only: false,
            flat_role: FlatRole::Auto,
            press_whole: false,
            hover_fill: true,
            clip: false,
            track: None,
            tooltip: None,
            passthrough: false,
            frame: Rect::default(),
        }
    }

    pub fn container() -> Self {
        Self::new(WidgetKind::Container)
    }

    pub fn icon(icon: Icon, size: f32) -> Self {
        Self::new(WidgetKind::Icon { icon, weight: IconWeight::Regular, size, paint: Paint::Label })
    }

    pub fn toggle(on: bool) -> Self {
        Self::new(WidgetKind::Toggle { on })
    }

    /// A segmented control (see [`WidgetKind::Segmented`]); add the segments as children.
    pub fn segmented(selected: usize) -> Self {
        Self::new(WidgetKind::Segmented { selected })
    }

    pub fn slider(value: f32) -> Self {
        Self::new(WidgetKind::Slider { value: value.clamp(0.0, 1.0), line: false })
    }

    /// A knobless line slider (see [`WidgetKind::Slider`]).
    pub fn line_slider(value: f32) -> Self {
        Self::new(WidgetKind::Slider { value: value.clamp(0.0, 1.0), line: true })
    }

    pub fn image() -> Self {
        Self::new(WidgetKind::Image { view: None, size: (1, 1), crop: None })
    }

    pub fn progress(paint: Paint) -> Self {
        Self::new(WidgetKind::Progress { value: 0.0, paint })
    }

    pub(crate) fn label(
        fonts: &mut FontSystem,
        family: &str,
        scale: f32,
        text: &str,
        style: TextStyle,
    ) -> Self {
        let mut buffer = new_buffer(fonts, style, scale);
        buffer.set_ellipsize(glyphon::cosmic_text::Ellipsize::End(
            glyphon::cosmic_text::EllipsizeHeightLimit::Lines(1),
        ));
        set_buffer_text(&mut buffer, fonts, family, text, style, Align::Start);
        Self::new(WidgetKind::Label(Label {
            max_lines: 1,
            text: text.to_owned(),
            buffer,
            style,
            paint: Paint::Label,
            align: Align::Start,
        }))
    }

    pub(crate) fn text_field(
        fonts: &mut FontSystem,
        family: &str,
        scale: f32,
        placeholder: &str,
        style: TextStyle,
    ) -> Self {
        let buffer = new_buffer(fonts, style, scale);
        let mut placeholder_buffer = new_buffer(fonts, style, scale);
        set_buffer_text(&mut placeholder_buffer, fonts, family, placeholder, style, Align::Start);
        Self::new(WidgetKind::TextField(TextField {
            editor: Editor::default(),
            buffer,
            placeholder: placeholder_buffer,
            placeholder_text: placeholder.to_owned(),
            style,
            paint: Paint::Label,
            scroll_x: 0.0,
            secure: false,
        }))
    }

    pub fn with_background(mut self, background: Background) -> Self {
        self.background = background;
        self
    }

    /// See [`Widget::button_tint`].
    pub fn with_button_tint(mut self) -> Self {
        self.button_tint = true;
        self
    }

    /// See [`Widget::glass_density`].
    pub fn with_glass_density(mut self, level: GlassLevel) -> Self {
        self.glass_density = Some(level);
        self
    }

    /// See [`Widget::glass_merge`].
    pub fn with_glass_merge(mut self, merge: f32) -> Self {
        self.glass_merge = Some(merge);
        self
    }

    /// See [`Widget::flat_role`].
    pub fn with_flat_role(mut self, role: FlatRole) -> Self {
        self.flat_role = role;
        self
    }

    /// See [`Widget::join_only`].
    pub fn as_join(mut self) -> Self {
        self.join_only = true;
        self
    }

    /// See [`Widget::press_whole`].
    pub fn pressed_whole(mut self) -> Self {
        self.press_whole = true;
        self
    }

    /// See [`Widget::tooltip`].
    pub fn with_tooltip(mut self, label: &str, action: Option<axion_config::Action>) -> Self {
        self.tooltip = Some(Tooltip { label: label.to_owned(), action });
        self
    }

    /// See [`Widget::hover_fill`].
    pub fn without_hover_fill(mut self) -> Self {
        self.hover_fill = false;
        self
    }

    /// A line slider's track thickness at rest (logical px); it swells while dragged.
    pub fn with_track(mut self, thickness: f32) -> Self {
        self.track = Some(thickness.max(1.0));
        self
    }

    pub fn with_clip(mut self) -> Self {
        self.clip = true;
        self
    }

    pub fn with_radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    pub fn with_shadow(mut self, shadow: Shadow) -> Self {
        self.shadow = Some(shadow);
        self
    }

    pub fn with_action(mut self, action: ActionId) -> Self {
        self.action = Some(action);
        self
    }

    /// Clickable without a hover/press highlight (see [`Widget::feedback`]).
    pub fn without_feedback(mut self) -> Self {
        self.feedback = false;
        self
    }

    pub fn with_paint(mut self, paint: Paint) -> Self {
        self.set_paint(paint);
        self
    }

    /// Recolours text, icons and progress bars.
    pub fn set_paint(&mut self, paint: Paint) {
        match &mut self.kind {
            WidgetKind::Label(l) => l.paint = paint,
            WidgetKind::Icon { paint: p, .. } | WidgetKind::Progress { paint: p, .. } => *p = paint,
            WidgetKind::TextField(f) => f.paint = paint,
            WidgetKind::Container
            | WidgetKind::Image { .. }
            | WidgetKind::Toggle { .. }
            | WidgetKind::Segmented { .. }
            | WidgetKind::Slider { .. } => {}
        }
    }

    /// Swaps the icon of an icon widget (e.g. reload ↔ stop).
    pub fn set_icon(&mut self, new: Icon) {
        if let WidgetKind::Icon { icon, .. } = &mut self.kind {
            *icon = new;
        }
    }

    pub fn with_icon_weight(mut self, weight: IconWeight) -> Self {
        if let WidgetKind::Icon { weight: w, .. } = &mut self.kind {
            *w = weight;
        }
        self
    }

    /// The widget's absolute frame in logical pixels, as of the last layout.
    pub fn frame(&self) -> Rect {
        self.frame
    }

    pub(crate) fn is_focusable(&self) -> bool {
        self.enabled && matches!(self.kind, WidgetKind::TextField(_))
    }
}

pub(crate) fn new_buffer(fonts: &mut FontSystem, style: TextStyle, scale: f32) -> Buffer {
    let (size, line, _) = style.metrics();
    let mut buffer = Buffer::new(fonts, Metrics::new(size * scale, line * scale));
    buffer.set_wrap(glyphon::Wrap::None);
    buffer
}

pub(crate) fn set_buffer_text(
    buffer: &mut Buffer,
    fonts: &mut FontSystem,
    family: &str,
    text: &str,
    style: TextStyle,
    align: Align,
) {
    let (_, _, weight) = style.metrics();
    let text = if style == TextStyle::Caption { text.to_uppercase() } else { text.to_owned() };
    let family = if style == TextStyle::Mono { Family::Monospace } else { Family::Name(family) };
    let attrs = Attrs::new().family(family).weight(Weight(weight));
    let align = match align {
        Align::Start => None,
        Align::Center => Some(glyphon::cosmic_text::Align::Center),
    };
    buffer.set_text(&text, &attrs, Shaping::Advanced, align);
    buffer.shape_until_scroll(fonts, false);
}

/// The lines a label's text breaks into at `width` (physical px): greedy, at spaces. A word
/// wider than the width stays on its own line (and is clipped). `buffer` is used for measuring
/// and left holding some line.
pub(crate) fn break_lines(
    buffer: &mut Buffer,
    fonts: &mut FontSystem,
    family: &str,
    label: (&str, TextStyle, Align),
    width: f32,
) -> Vec<String> {
    let (text, style, align) = label;
    let mut fits = |candidate: &str| {
        set_buffer_text(buffer, fonts, family, candidate, style, align);
        measure(buffer).0 <= width
    };
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current = word.to_owned();
            continue;
        }
        let candidate = format!("{current} {word}");
        if fits(&candidate) {
            current = candidate;
        } else {
            lines.push(std::mem::replace(&mut current, word.to_owned()));
        }
    }
    lines.push(current);
    lines
}

/// Shows a wrapping label's text broken to `width` (physical px) over at most its `max_lines`
/// (the last ends in an ellipsis when text is left over). Returns the text's size and whether
/// it all fit.
pub(crate) fn layout_wrapped(
    label: &mut Label,
    fonts: &mut FontSystem,
    family: &str,
    width: Option<f32>,
) -> ((f32, f32), bool) {
    let (style, align) = (label.style, label.align);
    set_buffer_text(&mut label.buffer, fonts, family, &label.text, style, align);
    let natural = measure(&label.buffer);
    let Some(width) = width.filter(|w| natural.0 > *w) else { return (natural, true) };
    let mut lines = break_lines(&mut label.buffer, fonts, family, (&label.text, style, align), width);
    let fit = lines.len() <= label.max_lines;
    if !fit {
        let mut rest = lines.split_off(label.max_lines - 1).join(" ");
        loop {
            set_buffer_text(&mut label.buffer, fonts, family, &format!("{rest}…"), style, align);
            if measure(&label.buffer).0 <= width || rest.is_empty() {
                break;
            }
            rest.pop();
            rest = rest.trim_end().to_owned();
        }
        lines.push(format!("{rest}…"));
    }
    set_buffer_text(&mut label.buffer, fonts, family, &lines.join("\n"), style, align);
    (measure(&label.buffer), fit)
}

/// Width and height of shaped text, in the buffer's (physical) units.
pub(crate) fn measure(buffer: &Buffer) -> (f32, f32) {
    let mut width: f32 = 0.0;
    let mut height: f32 = 0.0;
    for run in buffer.layout_runs() {
        width = width.max(run.line_w);
        height = height.max(run.line_top + run.line_height);
    }
    (width, height.max(buffer.metrics().line_height))
}

/// x position (buffer units) of the caret before byte `index`.
pub(crate) fn caret_x(buffer: &Buffer, index: usize) -> f32 {
    let Some(run) = buffer.layout_runs().next() else { return 0.0 };
    for glyph in run.glyphs {
        if glyph.start >= index {
            return glyph.x;
        }
    }
    run.line_w
}

/// Byte index closest to x (buffer units).
pub(crate) fn index_at(buffer: &Buffer, x: f32) -> usize {
    let Some(run) = buffer.layout_runs().next() else { return 0 };
    for glyph in run.glyphs {
        if x < glyph.x + glyph.w / 2.0 {
            return glyph.start;
        }
    }
    run.text.len()
}
