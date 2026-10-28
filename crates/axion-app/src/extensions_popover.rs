//! The extensions popover (the puzzle piece left of the share button): drips out of the toolbar
//! button and lists the installed extensions with their icon, what they can reach and an on/off
//! switch. A click on a row shows the extension's details: every permission, and Open Popup /
//! Options / Remove. At the bottom: install from a folder or a `.crx` file, and a Relaunch row
//! while a change waits for a restart. Built like the downloads popover. A Web Store install
//! that waits for the user's yes (name, icon, every permission) replaces the list until it is
//! answered.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_i18n::{tr, tr_n};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::extensions::{ExtensionView, access_summary, api_label};
use crate::home::Image;
use crate::popover::Popover;

const ACTION_BASE: u32 = 6300;
const INSTALL_FOLDER: ActionId = ActionId(ACTION_BASE);
const INSTALL_PACKAGE: ActionId = ActionId(ACTION_BASE + 1);
const RELAUNCH: ActionId = ActionId(ACTION_BASE + 2);
const BACK: ActionId = ActionId(ACTION_BASE + 3);
const DETAIL_TOGGLE: ActionId = ActionId(ACTION_BASE + 4);
const DETAIL_POPUP: ActionId = ActionId(ACTION_BASE + 5);
const DETAIL_OPTIONS: ActionId = ActionId(ACTION_BASE + 6);
const DETAIL_REMOVE: ActionId = ActionId(ACTION_BASE + 7);
const CONFIRM_ADD: ActionId = ActionId(ACTION_BASE + 8);
const CONFIRM_CANCEL: ActionId = ActionId(ACTION_BASE + 9);
const CHECK_UPDATES: ActionId = ActionId(ACTION_BASE + 20);
const ROW_BASE: u32 = ACTION_BASE + 10;
const TOGGLE_BASE: u32 = ACTION_BASE + 30;
/// How many extensions the list shows (the rest are reachable from Settings).
pub const ROWS: usize = 8;
const WIDTH: f32 = 360.0;
const PADDING: f32 = 6.0;
const INSET: f32 = 12.0;
const GAP: f32 = 4.0;
const GAP_BELOW_ANCHOR: f32 = 14.0;
const RADIUS: f32 = 24.0;
/// Fixed row heights, so the drip knows its target size before layout.
const HEADER: f32 = 30.0;
const ROW: f32 = 56.0;
const EMPTY: f32 = 40.0;
const ACTION: f32 = 36.0;
const LINE: f32 = 20.0;
const SECTION: f32 = 24.0;
const ICON: f32 = 30.0;
/// Permission lines the detail view shows before "and N more".
const MAX_LINES: usize = 10;
/// The install question shows more: the user decides on what they see there.
const MAX_CONFIRM_LINES: usize = 16;
/// Height of the question's buttons.
const BUTTON: f32 = 40.0;
const ERROR: Color = Color::rgb(0xFF6A5C);

/// A click in the popover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionsAction {
    InstallFolder,
    InstallPackage,
    Relaunch,
    /// Row `i`: show its details.
    Details(usize),
    /// Row `i`'s switch.
    Toggle(usize),
    /// Back from the details to the list.
    Back,
    /// The details' switch.
    DetailToggle,
    OpenPopup,
    OpenOptions,
    /// Removes the extension of the details (the first click only asks).
    Remove,
    /// The Web Store install may go ahead.
    ConfirmInstall,
    /// The Web Store install is cancelled.
    CancelInstall,
    /// Look for newer versions of the Web Store extensions now.
    CheckUpdates,
}

/// What the popover shows.
pub struct Content<'a> {
    pub extensions: &'a [ExtensionView],
    /// A texture per extension (same order), where it has an icon.
    pub icons: &'a [Option<Image>],
    pub needs_restart: bool,
    pub installing: bool,
    /// The extension whose details are shown, instead of the list.
    pub details: Option<&'a str>,
    /// The Remove button has been clicked once.
    pub confirm_remove: bool,
    /// A Web Store install waiting for the user's yes.
    pub pending: Option<Pending<'a>>,
    /// An update check is running.
    pub checking: bool,
    /// Whether any extension comes from the Web Store (the list offers "Check for Updates").
    pub has_store_extensions: bool,
}

/// What the confirmation shows.
pub struct Pending<'a> {
    pub extension: &'a ExtensionView,
    pub icon: Option<&'a Image>,
    /// The version installed now, when this replaces it.
    pub replaces: Option<&'a str>,
    /// An update that asks for more than before.
    pub update: bool,
    /// Permissions Axion switched off in the copy.
    pub unsupported: &'a [String],
}

pub struct ExtensionsPopover {
    popover: Popover,
    plate: NodeId,
}

impl ExtensionsPopover {
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

    /// Opens below `anchor` (the toolbar button), right-aligned with it, or refreshes the
    /// content if already open.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        content: &Content,
    ) {
        ui.clear_children(self.plate);
        let mut heights = Vec::new();
        if let Some(pending) = &content.pending {
            self.fill_confirm(ui, pending, &mut heights);
        } else {
            match content.details.and_then(|id| content.extensions.iter().position(|e| e.id == id)) {
                Some(i) => self.fill_details(ui, content, i, &mut heights),
                None => self.fill_list(ui, content, &mut heights),
            }
        }
        let height = 2.0 * (PADDING + INSET) + heights.iter().sum::<f32>() + (heights.len() - 1) as f32 * GAP;
        let right = anchor.origin.x + anchor.size.x + PADDING + 8.0;
        let left = (right - WIDTH).clamp(8.0, (window_width - WIDTH - 8.0).max(8.0));
        let target = Frame::new(left, anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR, WIDTH, height);
        self.popover.open(ui, config, anchor, target);
    }

    fn fill_list(&mut self, ui: &mut Ui, content: &Content, heights: &mut Vec<f32>) {
        let header = ui.add(self.plate, Widget::container(), fixed_row(HEADER, 8.0));
        ui.add_label(header, tr("Extensions"), TextStyle::BodyEmphasis, Style::default());
        heights.push(HEADER);
        if content.extensions.is_empty() {
            let empty = ui.add(self.plate, Widget::container(), fixed_row(EMPTY, 0.0));
            secondary(ui, empty, tr("No extensions installed"), TextStyle::Footnote);
            heights.push(EMPTY);
        }
        for (i, ext) in content.extensions.iter().take(ROWS).enumerate() {
            let row_node = ui.add(
                self.plate,
                Widget::container().with_action(ActionId(ROW_BASE + i as u32)).with_radius(14.0),
                Style { padding: padding(8.0, 4.0), ..fixed_row(ROW, 10.0) },
            );
            icon_of(ui, row_node, ext, content.icons.get(i).and_then(Option::as_ref));
            let text = ui.add(row_node, Widget::container(), column_grow());
            ui.add_label(text, &ext.name, TextStyle::Body, shrinking());
            let line = if ext.broken {
                tr("Files are missing").to_owned()
            } else {
                access_summary(&ext.permissions)
            };
            let note = secondary(ui, text, &line, TextStyle::Footnote);
            if ext.permissions.broad()
                && let Some(w) = ui.widget_mut(note)
            {
                w.set_paint(Paint::Fixed(ERROR));
            }
            if !ext.broken {
                ui.add(
                    row_node,
                    Widget::toggle(ext.enabled).with_action(ActionId(TOGGLE_BASE + i as u32)),
                    Style::default(),
                );
            }
            heights.push(ROW);
        }
        if content.needs_restart {
            action_row(ui, self.plate, RELAUNCH, Icon::Reload, tr("Relaunch to Apply Changes"), true);
            heights.push(ACTION);
        }
        let (folder, package) = if content.installing {
            (tr("Installing…"), tr("Installing…"))
        } else {
            (tr("Install from Folder…"), tr("Install from .crx File…"))
        };
        action_row(ui, self.plate, INSTALL_FOLDER, Icon::Folder, folder, false);
        action_row(ui, self.plate, INSTALL_PACKAGE, Icon::Plus, package, false);
        heights.extend([ACTION, ACTION]);
        if content.has_store_extensions {
            let text = if content.checking { tr("Checking for Updates…") } else { tr("Check for Updates") };
            action_row(ui, self.plate, CHECK_UPDATES, Icon::Reload, text, false);
            heights.push(ACTION);
        }
    }

    /// The question before a Web Store extension is installed: what it is and everything it
    /// may do.
    fn fill_confirm(&mut self, ui: &mut Ui, pending: &Pending, heights: &mut Vec<f32>) {
        let ext = pending.extension;
        let head = ui.add(
            self.plate,
            Widget::container(),
            Style { padding: padding(4.0, 0.0), ..fixed_row(HEADER, 8.0) },
        );
        icon_of(ui, head, ext, pending.icon);
        ui.add_label(head, &ext.name, TextStyle::BodyEmphasis, shrinking());
        heights.push(HEADER);
        let source = tr("Chrome Web Store");
        let sub = match (pending.update, pending.replaces) {
            (true, Some(old)) => format!("{} → {} · {source}", old, ext.version),
            _ if ext.version.is_empty() => source.to_owned(),
            _ => format!("{} · {source}", ext.version),
        };
        line(ui, self.plate, &sub, TextStyle::Footnote, true);
        heights.push(LINE);
        if !ext.description.is_empty() {
            line(ui, self.plate, &ext.description, TextStyle::Footnote, false);
            heights.push(LINE);
        }
        let question = if pending.update {
            tr("This update asks for more permissions. It can:")
        } else {
            tr("If you add it, it can:")
        };
        let title = ui.add(self.plate, Widget::container(), fixed_row(SECTION, 0.0));
        ui.add_label(title, question, TextStyle::Callout, Style::default());
        heights.push(SECTION);
        self.permission_lines(ui, ext, MAX_CONFIRM_LINES, heights);
        if !pending.unsupported.is_empty() {
            line(ui, self.plate, tr("Network request APIs are off in Axion"), TextStyle::Footnote, true);
            heights.push(LINE);
        }
        line(ui, self.plate, tr("It starts after Axion is relaunched."), TextStyle::Footnote, true);
        heights.push(LINE);
        let buttons = ui.add(
            self.plate,
            Widget::container(),
            Style { gap: Size { width: length(8.0), height: length(0.0) }, ..fixed_row(BUTTON, 8.0) },
        );
        let add = if pending.update { tr("Update Extension") } else { tr("Add Extension") };
        capsule_button(ui, buttons, CONFIRM_CANCEL, tr("Cancel"), false);
        capsule_button(ui, buttons, CONFIRM_ADD, add, true);
        heights.push(BUTTON);
    }

    fn fill_details(&mut self, ui: &mut Ui, content: &Content, i: usize, heights: &mut Vec<f32>) {
        let ext = &content.extensions[i];
        let back = ui.add(
            self.plate,
            Widget::container().with_action(BACK).with_radius(CAPSULE),
            Style { padding: padding(4.0, 0.0), ..fixed_row(HEADER, 6.0) },
        );
        ui.add(back, Widget::icon(Icon::Back, 14.0).with_paint(Paint::Accent), Style::default());
        icon_of(ui, back, ext, content.icons.get(i).and_then(Option::as_ref));
        ui.add_label(back, &ext.name, TextStyle::BodyEmphasis, shrinking());
        heights.push(HEADER);
        let version = if ext.version.is_empty() { String::new() } else { format!("{} ", ext.version) };
        let ident = format!("{version}· {}", ext.id);
        line(ui, self.plate, &ident, TextStyle::Footnote, true);
        heights.push(LINE);
        if ext.store {
            line(ui, self.plate, tr("Installed from the Chrome Web Store"), TextStyle::Footnote, true);
            heights.push(LINE);
        }
        if !ext.description.is_empty() {
            line(ui, self.plate, &ext.description, TextStyle::Footnote, false);
            heights.push(LINE);
        }
        if !ext.broken {
            let row_node = ui.add(self.plate, Widget::container(), fixed_row(ACTION, 8.0));
            let label = ui.add_label(row_node, tr("Enabled"), TextStyle::Body, Style::default());
            grow(ui, label);
            ui.add(row_node, Widget::toggle(ext.enabled).with_action(DETAIL_TOGGLE), Style::default());
            heights.push(ACTION);
        }
        // What the extension may do: the user sees all of it.
        let title = ui.add(self.plate, Widget::container(), fixed_row(SECTION, 0.0));
        ui.add_label(title, tr("Permissions"), TextStyle::Callout, Style::default());
        heights.push(SECTION);
        self.permission_lines(ui, ext, MAX_LINES, heights);
        if !ext.broken && ext.enabled {
            if ext.popup.is_some() {
                action_row(ui, self.plate, DETAIL_POPUP, Icon::Extensions, tr("Open Popup in a Tab"), false);
                heights.push(ACTION);
            }
            if ext.options_page.is_some() {
                action_row(ui, self.plate, DETAIL_OPTIONS, Icon::Settings, tr("Open Options"), false);
                heights.push(ACTION);
            }
        }
        let remove =
            if content.confirm_remove { tr("Click Again to Remove") } else { tr("Remove Extension") };
        let (_, label) = action_row_with_label(ui, self.plate, DETAIL_REMOVE, Icon::Trash, remove, false);
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::Fixed(ERROR));
        }
        heights.push(ACTION);
    }

    /// Every permission of `ext`, one line each (sites in red when that is all of them), and
    /// what it may ask for later.
    fn permission_lines(&mut self, ui: &mut Ui, ext: &ExtensionView, limit: usize, heights: &mut Vec<f32>) {
        let permissions = &ext.permissions;
        let mut lines: Vec<(String, bool)> = Vec::new();
        if permissions.broad() {
            lines.push((tr("Read and change your data on all websites").to_owned(), true));
        }
        lines.extend(permissions.hosts.iter().filter(|h| !axion_extensions::is_broad_host(h)).map(|h| {
            let text = format!("{} {h}", tr("Read and change data on"));
            (text, false)
        }));
        lines.extend(permissions.apis.iter().map(|p| (api_label(p), false)));
        // Several permissions say the same in words ("webRequest", "webNavigation"): once.
        let mut seen = std::collections::HashSet::new();
        lines.retain(|(text, _)| seen.insert(text.clone()));
        if lines.is_empty() {
            lines.push((tr("It asks for no permissions").to_owned(), false));
        }
        let more = lines.len().saturating_sub(limit);
        for (text, warning) in lines.into_iter().take(limit) {
            let label = line(ui, self.plate, &format!("•  {text}"), TextStyle::Footnote, !warning);
            if warning && let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Fixed(ERROR));
            }
            heights.push(LINE);
        }
        if more > 0 {
            let text = tr_n("and {n} more", "and {n} more", more);
            line(ui, self.plate, &format!("•  {text}"), TextStyle::Footnote, true);
            heights.push(LINE);
        }
        if !permissions.optional.is_empty() {
            let names = permissions.optional.iter().map(|p| api_label(p)).collect::<Vec<_>>().join(", ");
            line(ui, self.plate, &format!("{} {names}", tr("May ask later:")), TextStyle::Footnote, true);
            heights.push(LINE);
        }
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        self.popover.close(ui, config);
    }

    /// Advances the drip; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        self.popover.animate(ui, dt);
    }

    pub fn action(&self, id: ActionId) -> Option<ExtensionsAction> {
        action_of(id)
    }

    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.popover.contains(ui, p)
    }
}

/// An extension's icon (its picture, else the puzzle piece) at the start of a row.
fn icon_of(ui: &mut Ui, parent: NodeId, ext: &ExtensionView, image: Option<&Image>) {
    match image {
        Some(image) => {
            let node = ui.add(
                parent,
                Widget::image().with_radius(ICON * 0.22),
                Style {
                    size: Size { width: length(ICON), height: length(ICON) },
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            ui.set_image(node, Some(image.clone()));
        }
        None => {
            let paint = if ext.enabled && !ext.broken { Paint::Accent } else { Paint::Secondary };
            let node = ui.add(
                parent,
                Widget::container(),
                Style {
                    size: Size { width: length(ICON), height: length(ICON) },
                    flex_shrink: 0.0,
                    justify_content: JustifyContent::CENTER,
                    ..row(0.0)
                },
            );
            ui.add(node, Widget::icon(Icon::Extensions, 24.0).with_paint(paint), Style::default());
        }
    }
}

/// A capsule button of the install question; `primary` is the accent one.
fn capsule_button(ui: &mut Ui, parent: NodeId, id: ActionId, text: &str, primary: bool) {
    let background = if primary {
        Background::TintedGlass(GlassLevel::Thin, Paint::Accent)
    } else {
        Background::Glass(GlassLevel::Thin)
    };
    let node = ui.add(
        parent,
        Widget::container().with_action(id).with_radius(CAPSULE).with_background(background),
        Style {
            flex_grow: 1.0,
            flex_basis: length(0.0),
            min_size: Size { width: length(0.0), height: length(BUTTON) },
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    );
    let label = ui.add_label(node, text, TextStyle::BodyEmphasis, Style::default());
    ui.set_label_lines(label, 1);
    if primary && let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::OnAccent);
    }
}

/// A tappable row with an icon and an accent label; returns the row.
fn action_row(ui: &mut Ui, parent: NodeId, id: ActionId, icon: Icon, text: &str, emphasis: bool) -> NodeId {
    action_row_with_label(ui, parent, id, icon, text, emphasis).0
}

/// [`action_row`] that also returns the label.
fn action_row_with_label(
    ui: &mut Ui,
    parent: NodeId,
    id: ActionId,
    icon: Icon,
    text: &str,
    emphasis: bool,
) -> (NodeId, NodeId) {
    let node = ui.add(
        parent,
        Widget::container().with_action(id).with_radius(CAPSULE),
        Style { padding: padding(8.0, 0.0), ..fixed_row(ACTION, 8.0) },
    );
    ui.add(node, Widget::icon(icon, 15.0).with_paint(Paint::Accent), Style::default());
    let style = if emphasis { TextStyle::BodyEmphasis } else { TextStyle::Callout };
    let label = ui.add_label(node, text, style, shrinking());
    // Accent text on dense glass reads poorly over a light page; only the primary row has it.
    if emphasis && let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Accent);
    }
    (node, label)
}

/// A single-line label of the plate, `LINE` high.
fn line(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle, dim: bool) -> NodeId {
    let node = ui.add(parent, Widget::container(), fixed_row(LINE, 0.0));
    let label = ui.add_label(node, text, style, shrinking());
    if dim && let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
    label
}

fn grow(ui: &mut Ui, node: NodeId) {
    let mut style = ui.style(node);
    style.flex_grow = 1.0;
    ui.set_style(node, style);
}

/// The popover's clicks, by action id.
fn action_of(id: ActionId) -> Option<ExtensionsAction> {
    let n = id.0;
    Some(match id {
        INSTALL_FOLDER => ExtensionsAction::InstallFolder,
        INSTALL_PACKAGE => ExtensionsAction::InstallPackage,
        RELAUNCH => ExtensionsAction::Relaunch,
        BACK => ExtensionsAction::Back,
        DETAIL_TOGGLE => ExtensionsAction::DetailToggle,
        DETAIL_POPUP => ExtensionsAction::OpenPopup,
        DETAIL_OPTIONS => ExtensionsAction::OpenOptions,
        DETAIL_REMOVE => ExtensionsAction::Remove,
        CONFIRM_ADD => ExtensionsAction::ConfirmInstall,
        CONFIRM_CANCEL => ExtensionsAction::CancelInstall,
        CHECK_UPDATES => ExtensionsAction::CheckUpdates,
        _ if (ROW_BASE..ROW_BASE + ROWS as u32).contains(&n) => {
            ExtensionsAction::Details((n - ROW_BASE) as usize)
        }
        _ if (TOGGLE_BASE..TOGGLE_BASE + ROWS as u32).contains(&n) => {
            ExtensionsAction::Toggle((n - TOGGLE_BASE) as usize)
        }
        _ => return None,
    })
}

fn uniform(v: f32) -> Rect<LengthPercentage> {
    Rect { left: length(v), right: length(v), top: length(v), bottom: length(v) }
}

fn padding(h: f32, v: f32) -> Rect<LengthPercentage> {
    Rect { left: length(h), right: length(h), top: length(v), bottom: length(v) }
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

fn column_grow() -> Style {
    Style {
        flex_grow: 1.0,
        flex_shrink: 1.0,
        min_size: Size { width: length(0.0), height: auto() },
        flex_direction: FlexDirection::Column,
        gap: Size { width: length(0.0), height: length(3.0) },
        ..Default::default()
    }
}

fn shrinking() -> Style {
    Style { flex_shrink: 1.0, min_size: Size { width: length(0.0), height: auto() }, ..Default::default() }
}

fn secondary(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle) -> NodeId {
    let label = ui.add_label(parent, text, style, shrinking());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids() {
        assert_eq!(action_of(INSTALL_FOLDER), Some(ExtensionsAction::InstallFolder));
        assert_eq!(action_of(CONFIRM_ADD), Some(ExtensionsAction::ConfirmInstall));
        assert_eq!(action_of(CONFIRM_CANCEL), Some(ExtensionsAction::CancelInstall));
        assert_eq!(action_of(CHECK_UPDATES), Some(ExtensionsAction::CheckUpdates));
        assert_eq!(action_of(DETAIL_REMOVE), Some(ExtensionsAction::Remove));
        assert_eq!(action_of(ActionId(ROW_BASE + 3)), Some(ExtensionsAction::Details(3)));
        assert_eq!(action_of(ActionId(TOGGLE_BASE + 2)), Some(ExtensionsAction::Toggle(2)));
        assert_eq!(action_of(ActionId(ACTION_BASE + 99)), None);
        assert!(ROW_BASE + ROWS as u32 <= TOGGLE_BASE);
    }
}
