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

    /// Fills the open sheet: History asks the worker thread (the answer arrives in
    /// `animate_list_sheets`), Downloads is read from the store and the running downloads.
    pub(super) fn refresh_sheet(&mut self, kind: SheetKind, keep_scroll: bool) {
        if !self.sheet(kind).is_open() {
            return;
        }
        let Some(store) = &self.store else { return };
        let now = unix_now();
        match kind {
            SheetKind::History => {
                let Some(worker) = &self.list_worker else { return };
                self.list_generation += 1;
                self.list_keep_scroll = keep_scroll;
                worker.send(crate::list_worker::Job::Load {
                    generation: self.list_generation,
                    query: self.history_sheet.query().to_owned(),
                    limit: self.config.history.sheet_max_entries as usize,
                    now,
                });
            }
            SheetKind::Downloads => {
                let items = self.downloads.items(List::Panel, Some(store), &self.config, now, usize::MAX);
                let clear = self.downloads.has_finished().then(|| tr("Clear List"));
                let favicons = &self.favicons;
                self.downloads_sheet.set_items(
                    &mut self.ui,
                    items,
                    clear,
                    tr("Nothing downloaded yet"),
                    favicons,
                    keep_scroll,
                );
            }
        }
    }

    /// History rows the worker read: shown if they answer the newest question.
    fn history_loaded(&mut self, generation: u64, items: Vec<PanelItem>, took: Duration) {
        if generation != self.list_generation || !self.history_sheet.is_open() {
            return;
        }
        tracing::debug!(rows = items.len(), ms = took.as_millis(), "history loaded");
        let query = self.history_sheet.query().to_owned();
        let clear = (!items.is_empty() && query.is_empty())
            .then(|| if self.history_confirm { tr(CLEAR_HISTORY_CONFIRM) } else { tr(CLEAR_HISTORY) });
        let empty = if query.is_empty() {
            tr("Nothing here yet").to_owned()
        } else {
            tr_fmt!("No results for “{query}”", query = query)
        };
        let started = Instant::now();
        let favicons = &self.favicons;
        self.history_sheet.set_items(&mut self.ui, items, clear, &empty, favicons, self.list_keep_scroll);
        tracing::debug!(ms = started.elapsed().as_secs_f32() * 1000.0, "history sheet built");
    }

    /// Refreshes whatever list shows history or downloads (the sheets, the sidebar panel).
    pub(super) fn refresh_lists(&mut self) {
        self.refresh_sheet(SheetKind::History, true);
        self.refresh_sheet(SheetKind::Downloads, true);
        if let Some((panel, _)) = self.panel {
            self.show_panel_with(panel, true);
        }
    }

    /// Deletes rows of a sheet: History's entries go to the worker thread and leave the list at
    /// once (no reload); Downloads leave the list (their files stay).
    pub(super) fn delete_sheet_rows(&mut self, kind: SheetKind, rows: &[usize]) {
        if rows.is_empty() {
            return;
        }
        match kind {
            SheetKind::History => {
                let urls: Vec<String> =
                    rows.iter().filter_map(|&i| self.history_sheet.item_url(i).map(str::to_owned)).collect();
                if let Some(worker) = &self.list_worker {
                    worker.send(crate::list_worker::Job::DeleteHistory(urls));
                }
                let favicons = &self.favicons;
                self.history_sheet.remove_items(&mut self.ui, favicons, rows);
                // A reload that was already on its way would bring the rows back.
                self.list_generation += 1;
                self.bookmarks_changed = true;
            }
            SheetKind::Downloads => {
                let ids: Vec<i64> = rows.iter().filter_map(|&i| self.downloads.stored_id(i)).collect();
                if let Some(store) = &self.store
                    && let Err(e) = store.remove_downloads(&ids)
                {
                    tracing::error!("removing downloads from the list failed: {e}");
                }
                self.downloads_sheet.end_selection(&mut self.ui);
                self.refresh_sheet(kind, true);
            }
        }
    }

    /// The selected rows of a sheet (the bar's Delete button, the Delete key, the context menu).
    pub(super) fn delete_sheet_selection_of(&mut self, kind: SheetKind) {
        let rows = self.sheet(kind).selected_items();
        self.delete_sheet_rows(kind, &rows);
    }

    /// The Delete key and test aid: the open sheet's selection.
    pub(super) fn delete_sheet_selection(&mut self) {
        for kind in [SheetKind::History, SheetKind::Downloads] {
            if self.sheet(kind).is_open() {
                self.delete_sheet_selection_of(kind);
            }
        }
    }

    pub(super) fn end_sheet_selection(&mut self, kind: SheetKind) {
        match kind {
            SheetKind::History => self.history_sheet.end_selection(&mut self.ui),
            SheetKind::Downloads => self.downloads_sheet.end_selection(&mut self.ui),
        }
    }

    /// Ctrl+A in the open sheet (test aid `sheet-select-all`).
    pub(super) fn sheet_select_all(&mut self) {
        for kind in [SheetKind::History, SheetKind::Downloads] {
            if self.sheet(kind).is_open() {
                match kind {
                    SheetKind::History => self.history_sheet.select_all(&mut self.ui),
                    SheetKind::Downloads => self.downloads_sheet.select_all(&mut self.ui),
                }
            }
        }
    }

    /// Keys of the open sheet: Ctrl+A selects every row, Delete deletes the selection. While the
    /// search field holds text they belong to it. Returns whether the key was used.
    pub(super) fn list_sheet_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        let field = self.history_sheet.search_field();
        let typing =
            self.ui.focused() == Some(field) && self.ui.field_text(field).is_some_and(|t| !t.is_empty());
        if typing {
            return false;
        }
        match crate::input::key(event) {
            Some(Key::Char('a')) if self.modifiers.ctrl && !self.modifiers.shift && !self.modifiers.alt => {
                self.sheet_select_all();
                true
            }
            Some(Key::Named(name)) if name == "Delete" => {
                let any = [SheetKind::History, SheetKind::Downloads]
                    .into_iter()
                    .any(|k| self.sheet(k).is_open() && self.sheet(k).selected_count() > 0);
                if any {
                    self.delete_sheet_selection();
                }
                any
            }
            _ => false,
        }
    }

    /// A click in a list sheet.
    pub(super) fn list_sheet_click(&mut self, kind: SheetKind, click: Click) {
        let (shift, ctrl) = (self.modifiers.shift, self.modifiers.ctrl);
        let favicons = &self.favicons;
        let sheet = match kind {
            SheetKind::History => &mut self.history_sheet,
            SheetKind::Downloads => &mut self.downloads_sheet,
        };
        match click {
            Click::Close => self.close_list_sheet(kind),
            Click::Select => sheet.toggle_selecting(&mut self.ui),
            Click::CancelSelection => sheet.end_selection(&mut self.ui),
            Click::DeleteSelected => self.delete_sheet_selection_of(kind),
            Click::Check(i) => sheet.click_row(&mut self.ui, i, shift),
            Click::Group(g) => sheet.toggle_group(&mut self.ui, favicons, g),
            Click::GroupCheck(g) => sheet.click_group_check(&mut self.ui, g),
            // In selection mode (or with Ctrl or Shift held) a click on a row selects it.
            Click::Open(i) if sheet.is_selecting() || ctrl || shift => {
                sheet.click_row(&mut self.ui, i, shift)
            }
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
                    if let Some(worker) = &self.list_worker {
                        worker.send(crate::list_worker::Job::ClearHistory);
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
            Click::Delete(i) => self.delete_sheet_rows(kind, &[i]),
            Click::Button(i, k) => {
                self.downloads.row_button(List::Panel, i, k, &mut self.engine);
                self.refresh_sheet(kind, true);
            }
        }
    }

    /// Every frame: the sheets' flow, the history search, what the worker read, icons that arrived.
    pub(super) fn animate_list_sheets(&mut self, dt: f32) {
        let pointer = self.pointer;
        let favicons = &self.favicons;
        let query = self.history_sheet.animate(&mut self.ui, dt, pointer, favicons);
        self.downloads_sheet.animate(&mut self.ui, dt, pointer, favicons);
        if query.is_some() {
            self.history_confirm = false;
            self.refresh_sheet(SheetKind::History, false);
        }
        let done = self.list_worker.as_ref().map(crate::list_worker::ListWorker::poll).unwrap_or_default();
        for done in done {
            let crate::list_worker::Done::Loaded { generation, items, took } = done;
            self.history_loaded(generation, items, took);
        }
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

    /// Test aid (`seed-history:N`): N history rows spread over the last 90 days.
    pub(super) fn seed_history_for_test(&mut self, count: &str) {
        let (Some(store), Ok(count)) = (&self.store, count.trim().parse::<i64>()) else { return };
        let now = unix_now();
        let span = 90 * 86_400;
        let items: Vec<axion_store::NewHistory> = (0..count)
            .map(|i| axion_store::NewHistory {
                url: format!("https://site{}.example.com/page/{i}", i % 3000),
                title: format!("Page {i}: an article about topic {}", i % 977),
                visit_count: 1 + (i % 5) as u32,
                last_visit: now - i * span / count.max(1) - 30,
            })
            .collect();
        let started = Instant::now();
        match store.import_history(&items) {
            Ok(counts) => {
                tracing::info!(added = counts.added, ms = started.elapsed().as_millis(), "seeded history")
            }
            Err(e) => tracing::warn!("seeding history failed: {e}"),
        }
        self.refresh_lists();
    }

    /// Test aid (`sheet-scroll:PX`): scrolls the open list sheet down by PX.
    pub(super) fn sheet_scroll_for_test(&mut self, px: &str) {
        let Ok(px) = px.trim().parse::<f32>() else { return };
        for kind in [SheetKind::History, SheetKind::Downloads] {
            if self.sheet(kind).is_open() {
                let sheet = match kind {
                    SheetKind::History => &mut self.history_sheet,
                    SheetKind::Downloads => &mut self.downloads_sheet,
                };
                sheet.scroll_by(&mut self.ui, &self.favicons, -px);
            }
        }
    }

    /// Test aid (`sheet-stats`): the open sheet's widget count and what a layout costs.
    pub(super) fn sheet_stats_for_test(&mut self) {
        fn count(ui: &Ui, node: axion_ui::taffy::NodeId) -> usize {
            1 + ui.children(node).into_iter().map(|c| count(ui, c)).sum::<usize>()
        }
        let total = count(&self.ui, self.ui.root());
        // The cost of scrolling through the list: a wheel step, then the layout it makes.
        let steps = 200;
        let (mut sync, mut layout) = (Duration::ZERO, Duration::ZERO);
        for _ in 0..steps {
            let started = Instant::now();
            self.history_sheet.scroll_by(&mut self.ui, &self.favicons, -37.0);
            sync += started.elapsed();
            let started = Instant::now();
            self.ui.layout();
            layout += started.elapsed();
        }
        let scroll_ms = (sync + layout).as_secs_f32() * 1000.0 / steps as f32;
        let layout_ms = layout.as_secs_f32() * 1000.0 / steps as f32;
        let (rows, built, scroll) =
            (self.history_sheet.item_count(), self.history_sheet.built_count(), self.history_sheet.scroll());
        self.history_sheet.scroll_to(&mut self.ui, &self.favicons, 0.0);
        tracing::info!(
            widgets = total,
            rows,
            built,
            scroll,
            scroll_step_ms = scroll_ms,
            layout_step_ms = layout_ms,
            "sheet stats"
        );
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
        // A real pointer rests a frame or more before it clicks: the sheet reveals the checkbox
        // under it in that time.
        self.animate_list_sheets(0.0);
        self.ui.layout();
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
