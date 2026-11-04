//! The browser's side of the History and Downloads sheets (`list_sheet`): opening them out of the
//! sidebar row, filling them with the same rows the sidebar lists have, and what a click in them
//! does. With `general.list_sheets` off the sidebar lists are used as before.

use super::*;
use crate::list_sheet::{Click, ListSheet, SheetKind};

impl Browser {
    pub(super) fn sheet(&self, kind: SheetKind) -> &ListSheet {
        match kind {
            SheetKind::History => &self.history_sheet,
            SheetKind::Downloads => &self.downloads_sheet,
        }
    }

    /// A list sheet is open.
    pub(super) fn any_list_sheet_open(&self) -> bool {
        self.history_sheet.is_open() || self.downloads_sheet.is_open()
    }

    /// The sidebar's History or Downloads row (or a shortcut): the sheet, or the sidebar list when
    /// sheets are off. The sheet flows out of the row that was clicked (`sheet_origin`).
    pub(super) fn show_list(&mut self, kind: SheetKind) {
        if !self.config.general.list_sheets {
            let panel = if kind == SheetKind::History { Panel::History } else { Panel::Downloads };
            self.show_panel(panel);
            return;
        }
        self.open_list_sheet(kind);
    }

    pub(super) fn open_list_sheet(&mut self, kind: SheetKind) {
        tracing::debug!(?kind, open = self.sheet(kind).is_open(), "open list sheet");
        if self.sheet(kind).is_open() {
            self.close_list_sheet(kind);
            return;
        }
        for other in [SheetKind::History, SheetKind::Downloads] {
            if other != kind && self.sheet(other).is_open() {
                self.close_list_sheet(other);
            }
        }
        if self.settings.is_open() {
            self.close_settings();
        }
        if self.passwords.is_open() {
            self.close_passwords();
        }
        self.menu.close(&mut self.ui, &self.config);
        self.shields_popover.close(&mut self.ui, &self.config);
        self.downloads_popover.close(&mut self.ui, &self.config);
        self.history_confirm = false;
        let window = self.logical_size();
        let fallback = self.address_origin();
        let origin = self.sheet_origin.take().unwrap_or(fallback);
        let config = self.config.clone();
        let ui = &mut self.ui;
        match kind {
            SheetKind::History => self.history_sheet.open(ui, &config, window, origin, fallback),
            SheetKind::Downloads => self.downloads_sheet.open(ui, &config, window, origin, fallback),
        }
        self.refresh_sheet(kind, false);
        if kind == SheetKind::History {
            // Typing right away searches the history.
            let field = self.history_sheet.search_field();
            let events = self.ui.focus(field);
            self.handle_ui_events(events);
        } else if let Some(view) = self.active_view() {
            self.engine.set_focus(view, false);
        }
    }

    pub(super) fn close_list_sheet(&mut self, kind: SheetKind) {
        let fallback = self.address_origin();
        let config = self.config.clone();
        let ui = &mut self.ui;
        match kind {
            SheetKind::History => self.history_sheet.close(ui, &config, fallback),
            SheetKind::Downloads => self.downloads_sheet.close(ui, &config, fallback),
        }
        let _ = self.ui.blur();
        self.context_menus.close(&mut self.ui, &self.config);
    }

    /// Rebuilds the rows of the open sheet from the store (and the running downloads).
    pub(super) fn refresh_sheet(&mut self, kind: SheetKind, keep_scroll: bool) {
        if !self.sheet(kind).is_open() {
            return;
        }
        let Some(store) = &self.store else { return };
        let now = unix_now();
        let (items, clear, empty) = match kind {
            SheetKind::History => {
                let query = self.history_sheet.query().to_owned();
                let mut entries = if query.is_empty() {
                    store.recent_history(PANEL_HISTORY_LIMIT)
                } else {
                    store.search_history(&query, PANEL_HISTORY_LIMIT, now)
                }
                .unwrap_or_default();
                entries.sort_by_key(|e| std::cmp::Reverse(e.last_visit));
                let items = crate::history::panel_items(entries, now);
                let clear = (!items.is_empty() && query.is_empty()).then(|| {
                    if self.history_confirm { tr(CLEAR_HISTORY_CONFIRM) } else { tr(CLEAR_HISTORY) }
                });
                let empty = if query.is_empty() {
                    tr("Nothing here yet").to_owned()
                } else {
                    tr_fmt!("No results for “{query}”", query = query)
                };
                (items, clear, empty)
            }
            SheetKind::Downloads => {
                let items = self.downloads.items(List::Panel, Some(store), &self.config, now, usize::MAX);
                let clear = self.downloads.has_finished().then(|| tr("Clear List"));
                (items, clear, tr("Nothing downloaded yet").to_owned())
            }
        };
        let favicons = &self.favicons;
        let sheet = match kind {
            SheetKind::History => &mut self.history_sheet,
            SheetKind::Downloads => &mut self.downloads_sheet,
        };
        sheet.set_items(&mut self.ui, items, clear, &empty, favicons, keep_scroll);
    }

    /// Refreshes whatever list shows history or downloads (the sheets, the sidebar panel).
    pub(super) fn refresh_lists(&mut self) {
        self.refresh_sheet(SheetKind::History, true);
        self.refresh_sheet(SheetKind::Downloads, true);
        if let Some((panel, _)) = self.panel {
            self.show_panel_with(panel, true);
        }
    }

    /// A click in a list sheet.
    pub(super) fn list_sheet_click(&mut self, kind: SheetKind, click: Click) {
        match click {
            Click::Close => self.close_list_sheet(kind),
            Click::Clear => match kind {
                SheetKind::Downloads => {
                    Downloads::clear_list(self.store.as_ref());
                    self.refresh_sheet(kind, true);
                }
                // The first click asks, the second clears.
                SheetKind::History if !self.history_confirm => {
                    self.history_confirm = true;
                    self.history_sheet.set_clear_label(&mut self.ui, tr(CLEAR_HISTORY_CONFIRM));
                }
                SheetKind::History => {
                    self.history_confirm = false;
                    if let Some(store) = &self.store
                        && let Err(e) = store.clear_history_since(0)
                    {
                        tracing::error!("clearing history failed: {e}");
                    }
                    self.refresh_sheet(kind, false);
                }
            },
            Click::Open(i) => match kind {
                SheetKind::History => {
                    if let Some(url) = self.history_sheet.item_url(i).map(str::to_owned) {
                        self.close_list_sheet(kind);
                        self.navigate_to(url);
                    }
                }
                SheetKind::Downloads => self.downloads.open_row(List::Panel, i),
            },
            Click::Delete(i) => match kind {
                SheetKind::History => {
                    let url = self.history_sheet.item_url(i).map(str::to_owned);
                    if let (Some(store), Some(url)) = (&self.store, url)
                        && let Err(e) = store.delete_history_url(&url)
                    {
                        tracing::error!("deleting a history entry failed: {e}");
                    }
                    self.refresh_sheet(kind, true);
                }
                SheetKind::Downloads => {
                    self.downloads.remove_row(i, self.store.as_ref());
                    self.refresh_sheet(kind, true);
                }
            },
            Click::Button(i, k) => {
                self.downloads.row_button(List::Panel, i, k, &mut self.engine);
                self.refresh_sheet(kind, true);
            }
        }
    }

    /// Every frame: the sheets' flow, the history search, icons that arrived.
    pub(super) fn animate_list_sheets(&mut self, dt: f32) {
        if let Some(_query) = self.history_sheet.animate(&mut self.ui, dt) {
            self.history_confirm = false;
            self.refresh_sheet(SheetKind::History, false);
        }
        self.downloads_sheet.animate(&mut self.ui, dt);
        if self.config.downloads.show_site_icons || self.history_sheet.is_open() {
            let favicons = &self.favicons;
            for sheet in [&mut self.history_sheet, &mut self.downloads_sheet] {
                if sheet.is_open() {
                    sheet.sync_icons(&mut self.ui, favicons);
                }
            }
        }
    }

    /// The window changed size: the open sheets follow.
    pub(super) fn resize_list_sheets(&mut self, size: Vec2) {
        for sheet in [&mut self.history_sheet, &mut self.downloads_sheet] {
            if sheet.is_open() {
                sheet.resize(&mut self.ui, size);
            }
        }
    }

    /// The sheet comes out of the sidebar row `node` (the sidebar is too big an origin).
    pub(super) fn sheet_from_row(&mut self, node: axion_ui::taffy::NodeId) {
        let row = self.ui.frame(node);
        self.sheet_origin =
            Some(Origin { plane: self.chrome.sidebar_node(), point: row.center(), control: row });
    }

    // ---- capture and test aids (never in normal use) ----

    /// Test aid (`--open=seed-lists`, needs `AXION_PROFILE_DIR`): history over three days, finished
    /// downloads (one whose file is gone) and a site icon for example.org.
    pub(super) fn seed_lists_for_test(&mut self) {
        let Some(store) = &self.store else { return };
        let now = unix_now();
        let day = 86_400;
        for (i, (url, title)) in [
            ("https://example.org/docs", "Example documentation"),
            ("https://news.example.com/today", "Today's headlines"),
            ("https://rust-lang.org/", "Rust Programming Language"),
            ("https://example.org/old", "An older page"),
            ("https://wiki.example.net/a", "A wiki article"),
        ]
        .iter()
        .enumerate()
        {
            let at = now - [60, 3_600, 7_200, day + 100, 3 * day][i];
            if let Err(e) = store.record_visit(url, title, at) {
                tracing::warn!("seeding history failed: {e}");
            }
        }
        if let Err(e) = store.pin_top_site("example.org", "https://example.org/", "Example") {
            tracing::warn!("seeding a start page tile failed: {e}");
        }
        let dir = std::env::var_os("AXION_PROFILE_DIR").map(PathBuf::from).unwrap_or_default();
        let file = dir.join("seeded-download.pdf");
        if let Err(e) = std::fs::write(&file, b"%PDF-1.4 test") {
            tracing::warn!("seeding a download failed: {e}");
        }
        for (i, (url, name, path, state)) in [
            ("https://example.org/files/report.pdf", "report.pdf", file.clone(), "complete"),
            ("https://cdn.example.com/archive.zip", "archive.zip", dir.join("gone.zip"), "complete"),
            ("https://example.net/broken.iso", "broken.iso", dir.join("broken.iso"), "failed"),
        ]
        .iter()
        .enumerate()
        {
            let record = axion_store::StoredDownload {
                id: 0,
                url: (*url).into(),
                path: path.to_string_lossy().into_owned(),
                file_name: (*name).into(),
                mime_type: String::new(),
                size: 2_400_000,
                state: (*state).into(),
                finished: now - 600 * (i as i64 + 1),
            };
            if let Err(e) = store.add_download(&record) {
                tracing::warn!("seeding a download failed: {e}");
            }
        }
        // A blue square as example.org's icon.
        let rgba: Vec<u8> = (0..32 * 32).flat_map(|_| [0x2D, 0x7B, 0xF0, 0xFF]).collect();
        self.favicons.insert("https://example.org/", "https://example.org/favicon.png", 32, 32, &rgba, false);
        self.refresh_lists();
        self.home.icons_changed();
        self.update_home();
    }

    /// The node that carries `action` (a test aid's way to find a button without coordinates).
    fn node_with_action(&self, action: axion_ui::ActionId) -> Option<axion_ui::taffy::NodeId> {
        fn find(
            ui: &Ui,
            node: axion_ui::taffy::NodeId,
            action: axion_ui::ActionId,
        ) -> Option<axion_ui::taffy::NodeId> {
            if ui.is_visible(node) && ui.widget(node).and_then(|w| w.action) == Some(action) {
                return Some(node);
            }
            ui.children(node).into_iter().find_map(|c| find(ui, c, action))
        }
        find(&self.ui, self.ui.root(), action)
    }

    /// Test aid (`click-action:N`, `context-action:N`): the real pointer path (move, press, release)
    /// at the centre of the visible node with action id N, with the left or right button.
    pub(super) fn click_action_for_test(&mut self, id: &str, right: bool) {
        let Ok(n) = id.trim().parse::<u32>() else { return };
        self.ui.layout();
        let Some(node) = self.node_with_action(axion_ui::ActionId(n)) else {
            tracing::warn!(action = n, "test click: no such button is shown");
            return;
        };
        let frame = self.ui.frame(node);
        let p = frame.center();
        tracing::info!(action = n, ?p, right, "test click");
        let button = if right { winit::event::MouseButton::Right } else { winit::event::MouseButton::Left };
        self.pointer_moved(p);
        self.mouse_button(ElementState::Pressed, button);
        self.mouse_button(ElementState::Released, button);
    }

    /// Test aid (`context:XxY`): a right click at a point of the window, as a real pointer event.
    pub(super) fn context_click_for_test(&mut self, at: &str) {
        let Some((x, y)) =
            at.split_once('x').and_then(|(x, y)| Some((x.trim().parse().ok()?, y.trim().parse().ok()?)))
        else {
            return;
        };
        self.ui.layout();
        self.pointer_moved(Vec2::new(x, y));
        self.mouse_button(ElementState::Pressed, winit::event::MouseButton::Right);
        self.mouse_button(ElementState::Released, winit::event::MouseButton::Right);
    }
}
