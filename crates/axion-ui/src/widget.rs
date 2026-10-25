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
    /// A solid fill with an opacity multiplier.
    Fill(Paint, f32),
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
    /// Draws the widget in its "on" state (e.g. the sidebar toggle while the sidebar is open).
    pub selected: bool,
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
            selected: false,
            frame: Rect::default(),
        }
    }

    pub fn container() -> Self {
        Self::new(WidgetKind::Container)
    }

    pub fn icon(icon: Icon, size: f32) -> Self {
        Self::new(WidgetKind::Icon { icon, weight: IconWeight::Regular, size, paint: Paint::Label })
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
        set_buffer_text(&mut buffer, fonts, family, text, style, Align::Start);
        Self::new(WidgetKind::Label(Label {
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
        }))
    }

    pub fn with_background(mut self, background: Background) -> Self {
        self.background = background;
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
            WidgetKind::Container => {}
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
    let attrs = Attrs::new().family(Family::Name(family)).weight(Weight(weight));
    let align = match align {
        Align::Start => None,
        Align::Center => Some(glyphon::cosmic_text::Align::Center),
    };
    buffer.set_text(&text, &attrs, Shaping::Advanced, align);
    buffer.shape_until_scroll(fonts, false);
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
