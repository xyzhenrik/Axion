//! The Add to Playlist popover: drips out of the Playlist button in the address field and
//! offers the videos of the page (usually one) with "Add" and, when the video is a plain
//! file, "Add and Download". For a video already saved it offers "Open Playlist" and
//! "Remove".

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_i18n::tr;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::popover::Popover;

const ACTION_BASE: u32 = 6600;
const OPEN: ActionId = ActionId(ACTION_BASE + 40);
/// Videos offered at most.
pub const MAX_VIDEOS: usize = 4;
const WIDTH: f32 = 340.0;
const PADDING: f32 = 6.0;
const INSET: f32 = 16.0;
const GAP: f32 = 10.0;
const GAP_BELOW_ANCHOR: f32 = 14.0;
const RADIUS: f32 = 24.0;
const HEADER: f32 = 36.0;
const TITLES: f32 = 42.0;
const NOTE: f32 = 36.0;
const BUTTONS: f32 = 36.0;

/// One video of the page.
pub struct VideoInfo {
    pub title: String,
    /// "Example.com · 12:05".
    pub subtitle: String,
    /// It is a plain file that can be saved for offline use.
    pub can_download: bool,
    /// Why it can't be (shown instead of the download button).
    pub note: Option<String>,
    /// It is in the Playlist already.
    pub added: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopoverAction {
    Add { video: usize, download: bool },
    Remove(usize),
    OpenPlaylist,
}

pub struct PlaylistPopover {
    popover: Popover,
    plate: NodeId,
}

impl PlaylistPopover {
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
        Self { popover, plate }
    }

    pub fn is_open(&self) -> bool {
        self.popover.is_open()
    }

    /// Opens below `anchor` (the Playlist button), or refreshes the content if already open.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        videos: &[VideoInfo],
    ) {
        let videos = &videos[..videos.len().min(MAX_VIDEOS)];
        self.fill(ui, videos);
        let mut height = 2.0 * (PADDING + INSET) + HEADER;
        for v in videos {
            height += GAP + TITLES + GAP + BUTTONS;
            if v.note.is_some() && !v.added {
                height += GAP + NOTE;
            }
        }
        let left = (anchor.origin.x - PADDING - 8.0).clamp(8.0, (window_width - WIDTH - 8.0).max(8.0));
        let target = Frame::new(left, anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR, WIDTH, height);
        self.popover.open(ui, config, anchor, target);
    }

    fn fill(&mut self, ui: &mut Ui, videos: &[VideoInfo]) {
        ui.clear_children(self.plate);
        let header = ui.add(self.plate, Widget::container(), fixed_row(HEADER, 10.0));
        let any_added = videos.iter().any(|v| v.added);
        ui.add(
            header,
            Widget::icon(if any_added { Icon::Playlist } else { Icon::PlaylistAdd }, 24.0)
                .with_paint(Paint::Accent),
            Style::default(),
        );
        let heading =
            if any_added && videos.len() == 1 { tr("In your Playlist") } else { tr("Add to Playlist") };
        ui.add_label(header, heading, TextStyle::BodyEmphasis, Style::default());
        for (i, v) in videos.iter().enumerate() {
            let titles = ui.add(
                self.plate,
                Widget::container(),
                Style {
                    size: Size { width: auto(), height: length(TITLES) },
                    flex_shrink: 0.0,
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::CENTER,
                    ..Default::default()
                },
            );
            ui.add_label(titles, &v.title, TextStyle::Callout, shrinking());
            secondary(ui, titles, &v.subtitle, TextStyle::Footnote);
            if let (Some(note), false) = (&v.note, v.added) {
                let box_node = ui.add(
                    self.plate,
                    Widget::container(),
                    Style {
                        size: Size { width: auto(), height: length(NOTE) },
                        flex_shrink: 0.0,
                        ..row(8.0)
                    },
                );
                ui.add(
                    box_node,
                    Widget::icon(Icon::Globe, 16.0).with_paint(Paint::Secondary),
                    Style { flex_shrink: 0.0, ..Default::default() },
                );
                // Labels are one line: the note is cut in two.
                let (a, b) = split_note(note, 40);
                let col = ui.add(
                    box_node,
                    Widget::container(),
                    Style { flex_direction: FlexDirection::Column, ..Default::default() },
                );
                secondary(ui, col, &a, TextStyle::Footnote);
                secondary(ui, col, &b, TextStyle::Footnote);
            }
            let buttons = ui.add(self.plate, Widget::container(), fixed_row(BUTTONS, 8.0));
            if v.added {
                button(
                    ui,
                    buttons,
                    PopoverAction::OpenPlaylist.id_for(i),
                    Icon::Playlist,
                    tr("Open Playlist"),
                    false,
                );
                button(ui, buttons, PopoverAction::Remove(i).id_for(i), Icon::Trash, tr("Remove"), false);
            } else {
                button(
                    ui,
                    buttons,
                    PopoverAction::Add { video: i, download: false }.id_for(i),
                    Icon::PlaylistAdd,
                    tr("Add"),
                    !v.can_download,
                );
                if v.can_download {
                    button(
                        ui,
                        buttons,
                        PopoverAction::Add { video: i, download: true }.id_for(i),
                        Icon::Download,
                        tr("Add and Download"),
                        true,
                    );
                }
            }
        }
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

    pub fn action(&self, id: ActionId) -> Option<PopoverAction> {
        if id == OPEN {
            return Some(PopoverAction::OpenPlaylist);
        }
        let n = id.0.checked_sub(ACTION_BASE).filter(|&n| n < 4 * MAX_VIDEOS as u32)?;
        let video = (n / 4) as usize;
        match n % 4 {
            0 => Some(PopoverAction::Add { video, download: false }),
            1 => Some(PopoverAction::Add { video, download: true }),
            2 => Some(PopoverAction::Remove(video)),
            _ => None,
        }
    }
}

impl PopoverAction {
    /// The id of the button for video `video` (the Open button is shared).
    fn id_for(self, video: usize) -> ActionId {
        match self {
            PopoverAction::Add { download, .. } => {
                ActionId(ACTION_BASE + video as u32 * 4 + u32::from(download))
            }
            PopoverAction::Remove(_) => ActionId(ACTION_BASE + video as u32 * 4 + 2),
            PopoverAction::OpenPlaylist => OPEN,
        }
    }
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
            flex_shrink: 0.0,
            justify_content: JustifyContent::CENTER,
            ..row(6.0)
        },
    );
    ui.add(node, Widget::icon(icon, 17.0).with_paint(paint), Style::default());
    let label = ui.add_label(node, text, TextStyle::Callout, Style::default());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(paint);
    }
}

/// `note` as two lines of about `width` characters.
fn split_note(note: &str, width: usize) -> (String, String) {
    let lines = crate::playlist::wrap_title(note, width, 2);
    (lines.first().cloned().unwrap_or_default(), lines.get(1).cloned().unwrap_or_default())
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

fn secondary(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle) {
    let label = ui.add_label(parent, text, style, shrinking());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_map_back_to_actions() {
        let popover_ids = [
            (PopoverAction::Add { video: 2, download: true }, 2),
            (PopoverAction::Add { video: 0, download: false }, 0),
            (PopoverAction::Remove(3), 3),
        ];
        // The popover itself needs a Ui; the id mapping is plain arithmetic.
        for (action, video) in popover_ids {
            let id = action.id_for(video);
            let n = id.0 - ACTION_BASE;
            assert_eq!((n / 4) as usize, video);
        }
        assert_eq!(PopoverAction::OpenPlaylist.id_for(0), OPEN);
        assert!(OPEN.0 >= ACTION_BASE + 4 * MAX_VIDEOS as u32);
    }

    #[test]
    fn notes_split_in_two() {
        let (a, b) = split_note("HLS streams (.m3u8) can only be watched online for now.", 40);
        assert!(!a.is_empty() && !b.is_empty());
        assert!(a.chars().count() <= 40 && b.chars().count() <= 40);
    }
}
