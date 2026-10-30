//! The network chip's popover and the per-site route switch (`toggle_network`).

use axion_net::Target;

use super::network::rule_host;
use super::*;
use crate::network::{self as net, ChipState};
use crate::network_popover::{NetworkInfo, PopoverAction};

impl Browser {
    /// The page the chip is about: the active tab's address (behind the kill-switch page, the
    /// page it stands in for) and whether the tab is private.
    fn network_page(&self) -> Option<(String, bool)> {
        let tab = self.tabs.active()?;
        let shown = net::parse_blocked_url(&tab.url).map_or_else(|| tab.url.clone(), |(url, _)| url);
        Some((shown, tab.private))
    }

    fn network_info(&self) -> Option<NetworkInfo> {
        let (url, private) = self.network_page()?;
        let site = rule_host(&url)?;
        let network = &self.config.network;
        let routing = network.routing();
        let target = self.net.route_of(&self.config, &url, private);
        let chip = self.net.chip_for(&self.config, &url, private);
        let (state, title, detail) = match (&chip, &target) {
            (Some(c), Target::Connection(_)) => (
                c.state,
                match c.state {
                    ChipState::Up => tr_fmt!("Connected via {name}", name = c.name),
                    ChipState::Connecting => tr_fmt!("Connecting to {name}…", name = c.name),
                    _ => tr_fmt!("Connection to {name} is down", name = c.name),
                },
                match c.state {
                    ChipState::Down if network.kill_switch => {
                        format!("{} {}", c.detail, tr("Requests are blocked."))
                    }
                    _ => c.detail.clone(),
                },
            ),
            (Some(c), Target::Direct) => (
                ChipState::Bypassed,
                tr("Direct for this site").to_owned(),
                tr_fmt!("Not using {name}", name = c.name),
            ),
            (None, _) => (ChipState::Bypassed, tr("Direct connection").to_owned(), String::new()),
        };
        let rule = routing.site_rules.iter().find(|r| r.host == site).map(|r| r.connection.clone());
        let default_name = net::target_label(network, routing.default_for(private));
        let mut choices = vec![(tr_fmt!("Default ({name})", name = default_name), rule.is_none())];
        choices.push((tr("Direct").to_owned(), rule == Some(Target::Direct)));
        for c in &routing.connections {
            choices.push((c.name.clone(), rule == Some(Target::Connection(c.id.clone()))));
        }
        Some(NetworkInfo { site, state, title, detail, choices, has_rule: rule.is_some() })
    }

    /// Opens or closes the chip's popover.
    pub(super) fn toggle_network_popover(&mut self) {
        if self.network_popover.is_open() {
            self.network_popover.close(&mut self.ui, &self.config);
            return;
        }
        if self.config.network.connections.is_empty() {
            self.settings.set_category(Category::Network);
            self.run(Action::OpenSettings);
            return;
        }
        let Some(info) = self.network_info() else { return };
        self.shields_popover.close(&mut self.ui, &self.config);
        let anchor = self.chrome.network_button_frame(&self.ui);
        let anchor = if anchor.size.x > 0.0 { anchor } else { self.chrome.address_frame(&self.ui) };
        let width = self.logical_size().x;
        self.network_popover.open(&mut self.ui, &self.config, anchor, width, &info);
    }

    pub(super) fn refresh_network_popover(&mut self) {
        if !self.network_popover.is_open() {
            return;
        }
        let Some(info) = self.network_info() else { return };
        let anchor = self.chrome.network_button_frame(&self.ui);
        let width = self.logical_size().x;
        self.network_popover.update(&mut self.ui, &self.config, anchor, width, &info);
    }

    pub(super) fn network_popover_action(&mut self, action: PopoverAction) {
        match action {
            PopoverAction::OpenSettings => {
                self.network_popover.close(&mut self.ui, &self.config);
                self.settings.set_category(Category::Network);
                if self.settings.is_open() {
                    self.refresh_settings();
                } else {
                    self.run(Action::OpenSettings);
                }
            }
            PopoverAction::Choose(i) => {
                let Some((url, _)) = self.network_page() else { return };
                let Some(site) = rule_host(&url) else { return };
                let connections: Vec<String> =
                    self.config.network.connections.iter().map(|c| c.id.clone()).collect();
                let target = match i {
                    0 => None,
                    1 => Some(Target::Direct),
                    k => connections.get(k - 2).map(|id| Target::Connection(id.clone())),
                };
                if i >= 2 && target.is_none() {
                    return;
                }
                self.set_site_route(&site, target);
                self.network_popover.close(&mut self.ui, &self.config);
            }
        }
    }

    /// Sets (or clears, `None`) the route of a site, saves it and reloads the active page so
    /// it takes effect.
    fn set_site_route(&mut self, site: &str, target: Option<Target>) {
        let mut routing = self.config.network.routing();
        routing.set_site_rule(site, target);
        self.config.network.site_rules = routing.site_rules;
        self.config_dirty_since.get_or_insert(Instant::now());
        self.apply_network_config();
        self.refresh_settings();
        self.network_tick();
        if let Some(view) = self.active_view()
            && let Some(tab) = find_tab(&self.tabs, &self.parked, view)
        {
            if let Some((url, _)) = net::parse_blocked_url(&tab.url) {
                self.net.blocked.remove(&view);
                if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) {
                    tab.url.clone_from(&url);
                }
                self.engine.navigate(view, &url);
                self.tabs_changed = true;
            } else {
                self.engine.reload(view, false);
            }
        }
    }

    /// The `toggle_network` action: the connection on or off for this site.
    pub(super) fn toggle_network_for_site(&mut self) {
        let Some((url, private)) = self.network_page() else { return };
        let Some(site) = rule_host(&url) else { return };
        let routing = self.config.network.routing();
        if routing.connections.is_empty() {
            self.settings.set_category(Category::Network);
            self.run(Action::OpenSettings);
            return;
        }
        let now = self.net.route_of(&self.config, &url, private);
        let (target, message) = if now.is_direct() {
            // On: the tab's default if that is a connection, else the first one.
            let default = routing.default_for(private).clone();
            let target = if default.is_direct() {
                routing
                    .connections
                    .first()
                    .map(|c| Target::Connection(c.id.clone()))
                    .unwrap_or(Target::Direct)
            } else {
                default
            };
            let text = tr_fmt!(
                "Using {name} for {site}",
                name = net::target_label(&self.config.network, &target),
                site = site
            );
            (target, text)
        } else {
            (Target::Direct, tr_fmt!("Direct for {site}", site = site))
        };
        let rule = if &target == routing.default_for(private) { None } else { Some(target) };
        self.set_site_route(&site, rule);
        self.show_toast(Icon::Network, &message);
    }
}
