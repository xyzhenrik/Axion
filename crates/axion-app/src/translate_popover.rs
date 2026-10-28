//! The Translate popover: drips out of the Translate button in the address field. It offers
//! the translation, asks before anything is downloaded, shows progress, and restores the
//! original.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_i18n::tr;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::popover::Popover;

const ACTION_BASE: u32 = 6900;
const WIDTH: f32 = 340.0;
const PADDING: f32 = 6.0;
const INSET: f32 = 16.0;
const GAP: f32 = 10.0;
const GAP_BELOW_ANCHOR: f32 = 14.0;
const RADIUS: f32 = 24.0;
const HEADER: f32 = 36.0;
const NOTE: f32 = 18.0;
const BUTTONS: f32 = 36.0;
const PROGRESS: f32 = 6.0;
/// Characters of a note line that fit the popover.
const LINE_CHARS: usize = 46;

/// What the popover shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    /// "Translate this page from German to English?"
    Offer {
        from: String,
        to: String,
    },
    /// Looking up what the translation needs.
    Planning,
    /// Asking for the download of `bytes`.
    Consent {
        from: String,
        to: String,
        bytes: u64,
    },
    Downloading {
        done: u64,
        total: u64,
    },
    Translating {
        from: String,
        percent: u32,
        /// Pieces of text translated of those requested so far (the visible part and what
        /// scrolling has brought within reach).
        done: u32,
        total: u32,
    },
    Translated {
        from: String,
        to: String,
    },
    Failed {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Translate,
    Download,
    Cancel,
    ShowOriginal,
    NeverThisSite,
    OpenSettings,
}

impl Action {
    const ALL: [Action; 6] = [
        Action::Translate,
        Action::Download,
        Action::Cancel,
        Action::ShowOriginal,
        Action::NeverThisSite,
        Action::OpenSettings,
    ];

    fn id(self) -> ActionId {
        ActionId(ACTION_BASE + Self::ALL.iter().position(|a| *a == self).unwrap_or(0) as u32)
    }
}

/// A popover's title, note paragraphs, progress bar and buttons (action, icon, label, primary).
type Sections = (String, Vec<String>, Option<f32>, Vec<(Action, Icon, &'static str, bool)>);

pub struct TranslatePopover {
    popover: Popover,
    plate: NodeId,
    bar: Option<NodeId>,
    percent: Option<NodeId>,
    shown: Option<Content>,
}

impl TranslatePopover {
    /// `parent` is the toolbar, so the popover's glass merges with it.
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
        let popover = Popover::build(
            ui,
            parent,
            RADIUS,
            Style {
                size: Size { width: length(WIDTH), height: auto() },
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(GAP) },
                padding: uniform(PADDING + INSET),
                ..Default::default()
            },
        );
        let plate = popover.root();
        Self { popover, plate, bar: None, percent: None, shown: None }
    }

    pub fn is_open(&self) -> bool {
        self.popover.is_open()
    }

    pub fn action(&self, id: ActionId) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.id() == id)
    }

    /// Opens below `anchor`, or shows `content` in the open popover.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        content: &Content,
    ) {
        let height = self.fill(ui, content);
        let left = (anchor.origin.x - PADDING - 8.0).clamp(8.0, (window_width - WIDTH - 8.0).max(8.0));
        let target = Frame::new(left, anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR, WIDTH, height);
        self.popover.open(ui, config, anchor, target);
        self.shown = Some(content.clone());
    }

    /// Follows progress without rebuilding; anything else rebuilds.
    pub fn update(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        content: &Content,
    ) {
        if !self.is_open() || self.shown.as_ref() == Some(content) {
            return;
        }
        let same_kind = |a: &Content, b: &Content| std::mem::discriminant(a) == std::mem::discriminant(b);
        if let (Some(shown), Some(bar)) = (&self.shown, self.bar)
            && same_kind(shown, content)
        {
            match content {
                Content::Downloading { done, total } => {
                    ui.set_progress(bar, fraction(*done, *total));
                    if let Some(p) = self.percent {
                        ui.set_text(p, &megabytes(*done, *total));
                    }
                    self.shown = Some(content.clone());
                    return;
                }
                Content::Translating { percent, .. } => {
                    ui.set_progress(bar, *percent as f32 / 100.0);
                    if let Some(p) = self.percent
                        && let Content::Translating { from, done, total, .. } = content
                    {
                        ui.set_text(p, &translating_note(from, *done, *total));
                    }
                    self.shown = Some(content.clone());
                    return;
                }
                _ => {}
            }
        }
        self.open(ui, config, anchor, window_width, content);
    }

    /// Fills the popover and returns its height.
    fn fill(&mut self, ui: &mut Ui, content: &Content) -> f32 {
        ui.clear_children(self.plate);
        self.bar = None;
        self.percent = None;
        let mut height = 2.0 * (PADDING + INSET);
        let (title, notes, progress, buttons): Sections = match content {
            Content::Offer { from, to } => (
                tr("Translate this page?").to_owned(),
                vec![axion_i18n::tr_fmt!("From {from} to {to}.", from = from, to = to)],
                None,
                vec![
                    (Action::Translate, Icon::Translate, tr("Translate"), true),
                    (Action::NeverThisSite, Icon::Close, tr("Never for this site"), false),
                ],
            ),
            Content::Planning => (
                tr("Getting ready…").to_owned(),
                vec![tr("Looking up the language data.").to_owned()],
                Some(0.0),
                vec![(Action::Cancel, Icon::Close, tr("Cancel"), false)],
            ),
            Content::Consent { from, to, bytes } => (
                tr("Download language data?").to_owned(),
                vec![
                    axion_i18n::tr_fmt!(
                        "Translating {from} to {to} needs {size} MB, downloaded once from Mozilla and kept on this computer.",
                        from = from,
                        to = to,
                        size = (bytes / 1_000_000).max(1)
                    ),
                    tr("The pages you translate never leave it.").to_owned(),
                ],
                None,
                vec![
                    (Action::Download, Icon::Download, tr("Download and Translate"), true),
                    (Action::Cancel, Icon::Close, tr("Not Now"), false),
                ],
            ),
            Content::Downloading { done, total } => (
                tr("Downloading language data…").to_owned(),
                vec![megabytes(*done, *total)],
                Some(fraction(*done, *total)),
                vec![(Action::Cancel, Icon::Close, tr("Cancel"), false)],
            ),
            Content::Translating { from, percent, done, total } => (
                tr("Translating…").to_owned(),
                vec![translating_note(from, *done, *total)],
                Some(*percent as f32 / 100.0),
                vec![(Action::ShowOriginal, Icon::Reload, tr("Show Original"), false)],
            ),
            Content::Translated { from, to } => (
                tr("Page translated").to_owned(),
                vec![axion_i18n::tr_fmt!("From {from} to {to}.", from = from, to = to)],
                None,
                vec![
                    (Action::ShowOriginal, Icon::Reload, tr("Show Original"), true),
                    (Action::NeverThisSite, Icon::Close, tr("Never for this site"), false),
                ],
            ),
            Content::Failed { reason } => (
                tr("Couldn’t translate").to_owned(),
                crate::playlist::wrap_title(reason, LINE_CHARS, 3),
                None,
                vec![
                    (Action::Translate, Icon::Translate, tr("Try Again"), true),
                    (Action::OpenSettings, Icon::Settings, tr("Settings"), false),
                ],
            ),
        };

        let header = ui.add(self.plate, Widget::container(), fixed_row(HEADER, 10.0));
        ui.add(header, Widget::icon(Icon::Translate, 24.0).with_paint(Paint::Accent), Style::default());
        ui.add_label(header, &title, TextStyle::BodyEmphasis, shrinking());
        height += HEADER;

        for note in &notes {
            // A note is a block of lines of the popover's width, close together.
            let lines = crate::playlist::wrap_title(note, LINE_CHARS, 3);
            let block = ui.add(
                self.plate,
                Widget::container(),
                Style {
                    size: Size { width: auto(), height: length(NOTE * lines.len() as f32) },
                    flex_shrink: 0.0,
                    flex_direction: FlexDirection::Column,
                    ..Default::default()
                },
            );
            for line in &lines {
                let label = ui.add_label(block, line, TextStyle::Footnote, shrinking());
                if let Some(w) = ui.widget_mut(label) {
                    w.set_paint(Paint::Secondary);
                }
                if progress.is_some() {
                    self.percent = Some(label);
                }
            }
            height += GAP + NOTE * lines.len() as f32;
        }
        if let Some(value) = progress {
            let bar = ui.add(
                self.plate,
                Widget::progress(Paint::Accent),
                Style {
                    size: Size { width: auto(), height: length(PROGRESS) },
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            ui.set_progress(bar, value);
            self.bar = Some(bar);
            height += GAP + PROGRESS;
        }
        // Buttons share a row while they fit (by an estimate of their width), else wrap.
        let room = WIDTH - 2.0 * (PADDING + INSET);
        let mut used = f32::MAX;
        let mut row_node = self.plate;
        for (action, icon, text, primary) in buttons {
            let wanted = text.chars().count() as f32 * 8.0 + 56.0;
            if used + wanted + 8.0 > room {
                row_node = ui.add(self.plate, Widget::container(), fixed_row(BUTTONS, 8.0));
                height += GAP + BUTTONS;
                used = 0.0;
            }
            used += wanted + 8.0;
            button(ui, row_node, action.id(), icon, text, primary);
        }
        height
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        self.popover.close(ui, config);
    }

    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        self.popover.animate(ui, dt);
    }

    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.popover.contains(ui, p)
    }
}

fn translating_note(from: &str, done: u32, total: u32) -> String {
    axion_i18n::tr_fmt!("From {from}: {done} of {total}", from = from, done = done, total = total)
}

fn fraction(done: u64, total: u64) -> f32 {
    if total == 0 { 0.0 } else { (done as f32 / total as f32).clamp(0.0, 1.0) }
}

/// "12 / 40 MB".
fn megabytes(done: u64, total: u64) -> String {
    format!("{} / {} MB", done / 1_000_000, (total / 1_000_000).max(1))
}

/// A capsule button; `primary` is the accent one.
fn button(ui: &mut Ui, parent: NodeId, id: ActionId, icon: Icon, text: &str, primary: bool) {
    let (background, paint) = if primary {
        (Background::TintedGlass(GlassLevel::Thin, Paint::Accent), Paint::OnAccent)
    } else {
        (Background::Glass(GlassLevel::Thin), Paint::Label)
    };
    let node = ui.add(
        parent,
        Widget::container().with_action(id).with_radius(CAPSULE).with_background(background),
        Style {
            size: Size { width: auto(), height: length(BUTTONS) },
            padding: Rect { left: length(14.0), right: length(16.0), top: length(0.0), bottom: length(0.0) },
            flex_shrink: 1.0,
            justify_content: JustifyContent::CENTER,
            ..row(6.0)
        },
    );
    ui.add(node, Widget::icon(icon, 17.0).with_paint(paint), Style::default());
    let label = ui.add_label(node, text, TextStyle::Callout, shrinking());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(paint);
    }
}

fn uniform(v: f32) -> Rect<LengthPercentage> {
    Rect { left: length(v), right: length(v), top: length(v), bottom: length(v) }
}

fn row(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER,
        gap: Size { width: length(gap), height: length(0.0) },
        ..Default::default()
    }
}

fn fixed_row(height: f32, gap: f32) -> Style {
    Style { size: Size { width: auto(), height: length(height) }, flex_shrink: 0.0, ..row(gap) }
}

fn shrinking() -> Style {
    Style { flex_shrink: 1.0, min_size: Size { width: length(0.0), height: auto() }, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids_are_distinct_and_map_back() {
        for a in Action::ALL {
            assert!(a.id().0 >= ACTION_BASE);
            assert_eq!(Action::ALL.iter().filter(|b| b.id() == a.id()).count(), 1);
        }
    }

    #[test]
    fn sizes_read_in_megabytes() {
        assert_eq!(megabytes(12_000_000, 40_300_000), "12 / 40 MB");
        assert_eq!(fraction(5, 0), 0.0);
        assert_eq!(fraction(50, 100), 0.5);
    }
}
