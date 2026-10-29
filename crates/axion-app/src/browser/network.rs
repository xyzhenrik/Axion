//! The privacy network in the browser: keeping the engine's proxy settings current, the
//! kill-switch page, the address field's chip and its popover.
//! See `docs/network.md` and `crate::network`.

use axion_net::{ConnectionStatus, NetEvent, Target};

use super::*;
use crate::network::{self as net, BlockedAction, Chip};

impl Browser {
    /// Every loop iteration: follow the vault, the connections and the engine's settings.
    pub(super) fn network_tick(&mut self) {
        let unlocked = self.passwords.is_unlocked();
        if unlocked != self.net.vault_unlocked() {
            let snapshot = if unlocked { self.passwords.network_secrets() } else { None };
            if self.net.sync_vault(snapshot) {
                self.refresh_settings();
            }
        }
        if self.private_mode || self.tabs_have_private() {
            self.net.private_tabs_opened();
        }
        let events = self.net.poll();
        let mut changed = !events.is_empty();
        // The engine learns the new route (and its login) before the events below load pages
        // through it: a page that starts first would meet the proxy's login challenge with no
        // login known and fail.
        if self.net.apply_to_engine(&mut self.engine, &self.config) {
            changed = true;
        }
        for event in &events {
            self.network_event(event);
        }
        if changed {
            self.update_network_chip();
            self.refresh_network_popover();
            self.refresh_circuit_popover();
            self.refresh_settings();
        }
        self.circuit_tick();
    }

    /// The settings (`[network]`) changed.
    pub(super) fn apply_network_config(&mut self) {
        self.net.apply_config(&self.config);
        self.net.start_default_route(&self.config);
        if self.private_mode || self.tabs_have_private() {
            self.net.private_tabs_opened();
        }
        if self.net.apply_to_engine(&mut self.engine, &self.config) {
            self.update_network_chip();
        }
    }

    fn tabs_have_private(&self) -> bool {
        self.tabs.all().iter().chain(self.parked.all()).any(|t| t.private)
    }

    fn network_event(&mut self, event: &NetEvent) {
        let name = event.name.clone();
        let was_up = event.was.is_up();
        if self.config.network.notify_changes {
            match &event.status {
                ConnectionStatus::Up { .. } if !was_up => {
                    self.show_toast(Icon::Network, &tr_fmt!("Connected via {name}", name = name));
                }
                ConnectionStatus::Failed(_) | ConnectionStatus::Blocked(_) if was_up => {
                    let text = if self.config.network.kill_switch {
                        tr_fmt!("Connection to {name} is down. Requests are blocked.", name = name)
                    } else {
                        tr_fmt!("Connection to {name} is down. Requests go directly.", name = name)
                    };
                    self.show_toast(Icon::WifiOff, &text);
                }
                _ => {}
            }
        }
        match &event.status {
            ConnectionStatus::Up { .. } => {
                // Pages waiting behind the kill-switch page load again by themselves.
                let waiting: Vec<(ViewId, String)> = self
                    .net
                    .blocked
                    .iter()
                    .filter(|(_, b)| b.connection == event.id)
                    .map(|(v, b)| (*v, b.url.clone()))
                    .collect();
                for (view, url) in waiting {
                    self.net.blocked.remove(&view);
                    self.navigate_view_to(view, &url);
                }
                self.net.suspects.retain(|_, b| b.connection != event.id);
            }
            ConnectionStatus::Failed(_) | ConnectionStatus::Blocked(_) => {
                let suspects: Vec<(ViewId, net::Blocked)> = self
                    .net
                    .suspects
                    .iter()
                    .filter(|(_, b)| b.connection == event.id)
                    .map(|(v, b)| (*v, b.clone()))
                    .collect();
                for (view, b) in suspects {
                    self.net.suspects.remove(&view);
                    self.show_network_blocked(view, &b.url, &b.connection);
                }
            }
            _ => {}
        }
    }

    fn navigate_view_to(&mut self, view: ViewId, url: &str) {
        if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) {
            tab.url = url.to_owned();
            self.engine.navigate(view, url);
            self.tabs_changed = true;
        }
    }

    /// A page could not load because of its proxy. If its connection is down this is the kill
    /// switch: say so (never load it another way).
    pub(super) fn network_proxy_failed(&mut self, view: ViewId, url: &str, code: i32) {
        if url.starts_with("axion:") {
            return;
        }
        let Some(tab) = find_tab(&self.tabs, &self.parked, view) else { return };
        let private = tab.private;
        let target = self.net.route_of(&self.config, url, private);
        let Target::Connection(id) = target else {
            tracing::debug!(code, "a proxy error on a direct route (the system's proxy?)");
            return;
        };
        match self.net.manager.status(&id) {
            Some(ConnectionStatus::Up { .. }) => {
                // Looks up, but a page failed through it: check now; the page shows if it is down.
                self.net.suspects.insert(view, net::Blocked { url: url.to_owned(), connection: id.clone() });
                self.net.manager.report_failure(&id);
            }
            _ => self.show_network_blocked(view, url, &id),
        }
    }

    fn show_network_blocked(&mut self, view: ViewId, url: &str, connection: &str) {
        let name = net::target_label(&self.config.network, &Target::Connection(connection.to_owned()));
        self.net
            .blocked
            .insert(view, net::Blocked { url: url.to_owned(), connection: connection.to_owned() });
        // The tab keeps the address the user asked for; the page shows the explanation.
        self.engine.navigate(view, &net::blocked_url(url, connection, &name));
    }

    /// A button of the kill-switch page.
    pub(super) fn network_page_message(&mut self, view: ViewId, message: &str) {
        let Some(action) = net::parse_blocked_message(message) else { return };
        match action {
            BlockedAction::Retry { url, connection } => {
                if connection.is_empty() || self.net.manager.connection(&connection).is_none() {
                    self.navigate_view_to(view, &url);
                    return;
                }
                if matches!(self.net.manager.status(&connection), Some(ConnectionStatus::Up { .. })) {
                    self.net.blocked.remove(&view);
                    self.navigate_view_to(view, &url);
                } else {
                    // Starts it again; the page loads by itself once it is up.
                    self.net.blocked.insert(view, net::Blocked { url, connection: connection.clone() });
                    self.net.manager.start(&connection);
                    self.network_tick();
                }
            }
            BlockedAction::Direct { url } => {
                if let Some(host) = rule_host(&url) {
                    self.config.network.site_rules.retain(|r| r.host != host);
                    self.config
                        .network
                        .site_rules
                        .push(axion_net::SiteRule { host, connection: Target::Direct });
                    self.config_dirty_since.get_or_insert(Instant::now());
                    self.apply_network_config();
                    self.refresh_settings();
                }
                self.net.blocked.remove(&view);
                self.navigate_view_to(view, &url);
            }
            BlockedAction::Settings => {
                self.settings.set_category(Category::Network);
                if self.settings.is_open() {
                    self.refresh_settings();
                } else {
                    self.run(Action::OpenSettings);
                }
            }
        }
    }

    /// The chip of the active tab's address field.
    pub(super) fn update_network_chip(&mut self) {
        let chip = self.active_network_chip();
        self.chrome.set_network_chip(&mut self.ui, chip.as_ref().map(|c| c.state));
        self.chip_shown = chip.map(|c| c.state);
        self.update_circuit_button();
    }

    fn active_network_chip(&self) -> Option<Chip> {
        let tab = self.tabs.active()?;
        let shown = net::parse_blocked_url(&tab.url).map_or_else(|| tab.url.clone(), |(url, _)| url);
        self.net.chip_for(&self.config, &shown, tab.private)
    }
}

/// The site a rule for this page is about: its registrable domain.
pub(super) fn rule_host(url: &str) -> Option<String> {
    let host = axion_net::host_of_url(url)?;
    axion_net::normalize_host(&axion_vault::site::registrable(&host).unwrap_or(host))
}
