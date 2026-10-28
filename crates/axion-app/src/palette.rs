//! The command palette (Ctrl+K): a glass sheet with a search field and a list of every
//! browser action, fuzzy-filtered as you type, each with the shortcut it is bound to. Up/Down
//! move the selection, Enter runs it, Escape closes.
//!
//! The commands come from one registry: [`Action::all`] (the keymap's actions) named and
//! grouped by the Keyboard settings page (`settings::shortcut_name` / `shortcut_group`, whose
//! exhaustive matches make a new action fail to compile until it has a name). So a new action
//! shows up here by itself, with its shortcut, and runs through `Browser::run`.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Action, Config, Keymap};
use axion_i18n::tr;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::popover::Popover;

const ACTION_BASE: u32 = 6500;
/// Most rows a palette can hold (`palette.max_rows` is clamped to it).
const MAX_ROWS: usize = 24;
const HEADER_HEIGHT: f32 = 52.0;
const ROW_HEIGHT: f32 = 40.0;
const PADDING: f32 = 8.0;
const INSET: f32 = 12.0;
const GAP_BELOW_ANCHOR: f32 = 26.0;
const PAGE_INSET: f32 = 10.0;
const WINDOW_MARGIN: f32 = 8.0;
const RADIUS: f32 = 26.0;
const SELECTED_OPACITY: f32 = 0.2;

/// One thing the palette can run.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub action: Action,
    pub name: String,
    pub group: String,
    pub shortcut: Option<String>,
}

/// Every command, in the registry's order. Media keys (only meaningful in fullscreen video) and
/// the palette itself are left out.
pub fn commands(keymap: &Keymap) -> Vec<Command> {
    Action::all()
        .filter(|a| !a.is_media() && *a != Action::CommandPalette)
        .map(|action| Command {
            action,
            name: crate::settings::shortcut_name(action).to_owned(),
            group: crate::settings::shortcut_group(action).to_owned(),
            shortcut: keymap.chords_for(action).first().map(ToString::to_string),
        })
        .collect()
}

/// How well `query` matches `text`: `None` unless all of the query's characters occur in
/// `text` in order (ignoring case and spaces); higher is better. Consecutive characters, word
/// starts and an early first match score more.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> =
        query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.chars().collect();
    let mut score = 0;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, c) in text.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        if c.to_lowercase().next() != Some(query[qi]) {
            continue;
        }
        score += 10;
        if last.is_some_and(|l| l + 1 == i) {
            score += 15;
        }
        if i == 0 || !text[i - 1].is_alphanumeric() {
            score += 12;
        }
        if qi == 0 {
            score -= i.min(20) as i32;
        }
        last = Some(i);
        qi += 1;
    }
    (qi == query.len()).then_some(score)
}

/// The commands matching `query`, best first (registry order among equals); all of them for an
/// empty query. The group name matches too ("tab close").
pub fn filter(all: &[Command], query: &str) -> Vec<usize> {
    let mut scored: Vec<(i32, usize)> = all
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let own = fuzzy_score(query, &c.name);
            let grouped = fuzzy_score(query, &format!("{} {}", c.group, c.name)).map(|s| s - 8);
            own.into_iter().chain(grouped).max().map(|s| (s, i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

pub struct CommandPalette {
    popover: Popover,
    field: NodeId,
    list: NodeId,
    rows: Vec<NodeId>,
    commands: Vec<Command>,
    /// Indices into `commands`, best first.
    shown: Vec<usize>,
    /// Position in `shown` of the selected row.
    selected: usize,
    /// First visible position in `shown`.
    offset: usize,
    query: String,
    target: Option<Frame>,
    max_rows: usize,
    show_shortcuts: bool,
    width: f32,
}

impl CommandPalette {
    /// `parent` is the toolbar, like the find bar's.
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
        let popover = Popover::build(
            ui,
            parent,
            RADIUS,
            Style {
                flex_direction: FlexDirection::Column,
                padding: Rect {
                    left: length(PADDING),
                    right: length(PADDING),
                    top: length(0.0),
                    bottom: length(PADDING),
                },
                ..Default::default()
            },
        )
        .detached();
        let plate = popover.root();
        let header = ui.add(
            plate,
            Widget::container(),
            Style {
                size: Size { width: auto(), height: length(HEADER_HEIGHT) },
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(10.0), height: length(0.0) },
                padding: Rect {
                    left: length(INSET),
                    right: length(INSET),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        );
        ui.add(
            header,
            Widget::icon(Icon::Search, 18.0).with_paint(Paint::Secondary),
            Style { flex_shrink: 0.0, ..Default::default() },
        );
        let field = ui.add_text_field(
            header,
            tr("Search commands"),
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                size: Size { width: auto(), height: length(24.0) },
                ..Default::default()
            },
        );
        let list = ui.add(
            plate,
            Widget::container(),
            Style { flex_direction: FlexDirection::Column, flex_shrink: 0.0, ..Default::default() },
        );
        Self {
            popover,
            field,
            list,
            rows: Vec::new(),
            commands: Vec::new(),
            shown: Vec::new(),
            selected: 0,
            offset: 0,
            query: String::new(),
            target: None,
            max_rows: 8,
            show_shortcuts: true,
            width: 560.0,
        }
    }

    pub fn is_open(&self) -> bool {
        self.popover.is_open()
    }

    pub fn field(&self) -> NodeId {
        self.field
    }

    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.popover.contains(ui, p)
    }

    /// The command row a click landed on (index in the visible list).
    pub fn row_action(id: ActionId) -> Option<usize> {
        (ACTION_BASE..ACTION_BASE + MAX_ROWS as u32).contains(&id.0).then(|| (id.0 - ACTION_BASE) as usize)
    }

    /// Opens below `anchor` (the address field) with an empty search.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        keymap: &Keymap,
        anchor: Frame,
        window_width: f32,
        page_top: f32,
    ) {
        self.max_rows = (config.command_palette.max_rows as usize).clamp(3, MAX_ROWS);
        self.show_shortcuts = config.command_palette.show_shortcuts;
        self.width = config.command_palette.width;
        self.commands = commands(keymap);
        self.query.clear();
        ui.set_field_text(self.field, "");
        self.refilter();
        self.rebuild(ui);
        let target = self.target_frame(anchor, window_width, page_top);
        self.target = Some(target);
        self.popover.open(ui, config, anchor, target);
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        self.popover.close(ui, config);
    }

    /// Keeps the sheet centred under the bar when the window changes.
    pub fn follow(&mut self, ui: &mut Ui, anchor: Frame, window_width: f32, page_top: f32) {
        if !self.is_open() {
            return;
        }
        let target = self.target_frame(anchor, window_width, page_top);
        if self.target != Some(target) {
            self.target = Some(target);
            self.popover.retarget(ui, target);
        }
    }

    /// Advances the drip and filters when the field changed; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        self.popover.animate(ui, dt);
        if !self.is_open() {
            return;
        }
        let text = ui.field_text(self.field).unwrap_or_default().to_owned();
        if text != self.query {
            self.query = text;
            self.refilter();
            self.rebuild(ui);
            if let Some(mut target) = self.target {
                target.size.y = self.height();
                self.target = Some(target);
                self.popover.retarget(ui, target);
            }
        }
    }

    /// Moves the selection by `delta` rows (wrapping).
    pub fn move_selection(&mut self, ui: &mut Ui, delta: isize) {
        let n = self.shown.len() as isize;
        if n == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        self.rebuild(ui);
    }

    /// The command of the visible row `row`, or the selected one.
    pub fn command_at(&self, row: Option<usize>) -> Option<&Command> {
        let position = row.map_or(self.selected, |r| self.offset + r);
        self.shown.get(position).and_then(|&i| self.commands.get(i))
    }

    fn refilter(&mut self) {
        self.shown = filter(&self.commands, &self.query);
        self.selected = 0;
        self.offset = 0;
    }

    fn visible_rows(&self) -> usize {
        self.shown.len().min(self.max_rows)
    }

    fn height(&self) -> f32 {
        let rows = self.visible_rows().max(1);
        HEADER_HEIGHT + rows as f32 * ROW_HEIGHT + PADDING
    }

    fn target_frame(&self, anchor: Frame, window_width: f32, page_top: f32) -> Frame {
        let width = self.width.min((window_width - 2.0 * WINDOW_MARGIN).max(200.0));
        let left = ((window_width - width) / 2.0).max(WINDOW_MARGIN);
        let top = (anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR).max(page_top + PAGE_INSET);
        Frame::new(left, top, width, self.height())
    }

    fn rebuild(&mut self, ui: &mut Ui) {
        // The selection stays inside the window of visible rows.
        let rows = self.visible_rows();
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if rows > 0 && self.selected >= self.offset + rows {
            self.offset = self.selected + 1 - rows;
        }
        ui.clear_children(self.list);
        self.rows.clear();
        if self.shown.is_empty() {
            let label = ui.add_label(
                self.list,
                tr("No matching commands"),
                TextStyle::Body,
                Style {
                    size: Size { width: auto(), height: length(ROW_HEIGHT) },
                    padding: Rect {
                        left: length(INSET),
                        right: length(INSET),
                        top: length(10.0),
                        bottom: length(0.0),
                    },
                    ..Default::default()
                },
            );
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
            return;
        }
        for r in 0..rows {
            let position = self.offset + r;
            let Some(command) = self.shown.get(position).and_then(|&i| self.commands.get(i)) else { break };
            let selected = position == self.selected;
            let row = ui.add(
                self.list,
                Widget::container()
                    .with_action(ActionId(ACTION_BASE + r as u32))
                    .with_radius(CAPSULE)
                    .with_background(if selected {
                        Background::Fill(Paint::Accent, SELECTED_OPACITY)
                    } else {
                        Background::None
                    }),
                Style {
                    size: Size { width: auto(), height: length(ROW_HEIGHT) },
                    flex_shrink: 0.0,
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::CENTER,
                    gap: Size { width: length(10.0), height: length(0.0) },
                    padding: Rect {
                        left: length(INSET),
                        right: length(INSET),
                        top: length(0.0),
                        bottom: length(0.0),
                    },
                    ..Default::default()
                },
            );
            ui.add_label(
                row,
                &command.name,
                TextStyle::Body,
                Style { min_size: Size { width: length(0.0), height: auto() }, ..Default::default() },
            );
            let group = ui.add_label(
                row,
                &command.group,
                TextStyle::Footnote,
                Style {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_size: Size { width: length(0.0), height: auto() },
                    ..Default::default()
                },
            );
            if let Some(w) = ui.widget_mut(group) {
                w.set_paint(Paint::Secondary);
            }
            if self.show_shortcuts
                && let Some(keys) = &command.shortcut
            {
                let keys = ui.add_label(
                    row,
                    keys,
                    TextStyle::Footnote,
                    Style { flex_shrink: 0.0, ..Default::default() },
                );
                if let Some(w) = ui.widget_mut(keys) {
                    w.set_paint(Paint::Secondary);
                }
            }
            self.rows.push(row);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_needs_the_characters_in_order() {
        assert!(fuzzy_score("rld", "Reload").is_some());
        assert!(fuzzy_score("dlr", "Reload").is_none());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        assert_eq!(fuzzy_score("  ", "anything"), Some(0));
        assert!(fuzzy_score("NEW TAB", "New tab").is_some());
    }

    #[test]
    fn better_matches_score_higher() {
        let prefix = fuzzy_score("tab", "Tab overview").unwrap();
        let scattered = fuzzy_score("tab", "Hard reload tab").unwrap();
        assert!(prefix > scattered);
        let word_start = fuzzy_score("ct", "Close tab").unwrap();
        let inside = fuzzy_score("ct", "Hard reload").unwrap_or(i32::MIN);
        assert!(word_start > inside);
    }

    #[test]
    fn registry_lists_actions_with_names_and_shortcuts() {
        let keymap = Keymap::default();
        let all = commands(&keymap);
        let find = |name: &str| all.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(find("Zoom in").action, Action::ZoomIn);
        assert_eq!(find("Reload").shortcut.as_deref(), Some("Ctrl+R"));
        assert!(all.iter().all(|c| !c.action.is_media() && c.action != Action::CommandPalette));
        // Every other keymap action is in there.
        let expected = Action::all().filter(|a| !a.is_media() && *a != Action::CommandPalette).count();
        assert_eq!(all.len(), expected);
        assert!(all.iter().all(|c| !c.name.is_empty() && !c.group.is_empty()));
    }

    #[test]
    fn filter_ranks_and_empty_query_lists_everything() {
        let all = commands(&Keymap::default());
        assert_eq!(filter(&all, "").len(), all.len());
        let hits = filter(&all, "zoom");
        assert!(hits.len() >= 3);
        assert!(all[hits[0]].name.starts_with("Zoom"));
        assert!(filter(&all, "qqqqzz").is_empty());
        let reload = filter(&all, "reload");
        assert_eq!(all[reload[0]].action, Action::Reload);
    }
}
