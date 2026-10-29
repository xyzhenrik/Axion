//! The Tor circuit display: the button in the address field for pages that go through Tor,
//! its popover (the path of the site's circuit, refreshed while open) and the two actions
//! (a new circuit for the site, a new identity). See `docs/network.md`.

use axion_net::{ConnectionKind, Hop, HopRole, NameLanguage, Target, country_name};

use super::*;
use crate::circuit_popover::{CircuitAction, CircuitRow, CircuitView, RowKind};
use crate::network::{self as net, ChipState};

/// How often an open circuit popover looks at the circuit again.
const REFRESH: Duration = Duration::from_secs(1);

/// The page's circuit: the Tor connection it goes through and the page's host.
struct CircuitTarget {
    connection: String,
    host: String,
}

impl Browser {
    /// The active page, if it goes through a Tor connection.
    fn circuit_target(&self) -> Option<CircuitTarget> {
        let tab = self.tabs.active()?;
        let url = net::parse_blocked_url(&tab.url).map_or_else(|| tab.url.clone(), |(url, _)| url);
        self.circuit_target_of(&url, tab.private)
    }

    fn circuit_target_of(&self, url: &str, private: bool) -> Option<CircuitTarget> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return None;
        }
        let Target::Connection(id) = self.net.route_of(&self.config, url, private) else { return None };
        let is_tor =
            self.config.network.connection(&id).is_some_and(|c| matches!(c.kind, ConnectionKind::Tor(_)));
        let host = axion_net::host_of_url(url)?;
        is_tor.then_some(CircuitTarget { connection: id, host })
    }

    /// Shows or hides the circuit button for the active page.
    pub(super) fn update_circuit_button(&mut self) {
        let shown = self.config.network.show_chip && self.circuit_target().is_some();
        self.chrome.set_circuit_button(&mut self.ui, shown);
        if !shown && self.circuit_popover.is_open() {
            self.circuit_popover.close(&mut self.ui, &self.config);
        }
    }

    /// What the popover shows now.
    fn circuit_view(&self) -> Option<CircuitView> {
        let target = self.circuit_target()?;
        let status = self.net.manager.status(&target.connection)?;
        let host = site_label(&target.host);
        let title = tr_fmt!("Circuit for {host}", host = host);
        let mut rows = vec![CircuitRow {
            kind: RowKind::Browser,
            badge: None,
            title: tr("This browser").to_owned(),
            detail: String::new(),
        }];
        let mut note = None;
        let circuit =
            status.is_up().then(|| self.net.manager.circuit_for(&target.connection, &target.host)).flatten();
        match circuit {
            Some(circuit) => {
                for hop in &circuit.hops {
                    rows.push(hop_row(hop));
                }
            }
            None => {
                if !status.is_up() {
                    note = Some(self.connecting_text(&target.connection, &status));
                } else {
                    note = Some(
                        tr("No circuit for this site yet. It appears once the page connects.").to_owned(),
                    );
                }
                rows.push(CircuitRow {
                    kind: RowKind::Relay,
                    badge: Some("··".to_owned()),
                    title: tr("Connecting…").to_owned(),
                    detail: String::new(),
                });
            }
        }
        rows.push(CircuitRow { kind: RowKind::Site, badge: None, title: host, detail: String::new() });
        Some(CircuitView { title, status: note, rows })
    }

    /// "Bootstrapping 40%: loading relays", or why the connection is not up.
    fn connecting_text(&self, id: &str, status: &axion_net::ConnectionStatus) -> String {
        match status {
            axion_net::ConnectionStatus::Failed(r) | axion_net::ConnectionStatus::Blocked(r) => r.clone(),
            _ => net::bootstrap_text(self.net.manager.progress(id)),
        }
    }

    pub(super) fn toggle_circuit_popover(&mut self) {
        if self.circuit_popover.is_open() {
            self.circuit_popover.close(&mut self.ui, &self.config);
            return;
        }
        let Some(view) = self.circuit_view() else { return };
        self.shields_popover.close(&mut self.ui, &self.config);
        self.network_popover.close(&mut self.ui, &self.config);
        self.reading.close_popovers(&mut self.ui, &self.config);
        let anchor = self.chrome.circuit_button_frame(&self.ui);
        let anchor = if anchor.size.x > 0.0 { anchor } else { self.chrome.address_frame(&self.ui) };
        let width = self.logical_size().x;
        self.circuit_refreshed = Instant::now();
        self.circuit_popover.open(&mut self.ui, &self.config, anchor, width, &view);
    }

    /// Looks at the circuit again while the popover is open.
    pub(super) fn refresh_circuit_popover(&mut self) {
        if !self.circuit_popover.is_open() {
            return;
        }
        let Some(view) = self.circuit_view() else {
            self.circuit_popover.close(&mut self.ui, &self.config);
            return;
        };
        let anchor = self.chrome.circuit_button_frame(&self.ui);
        let width = self.logical_size().x;
        self.circuit_popover.update(&mut self.ui, &self.config, anchor, width, &view);
    }

    pub(super) fn circuit_popover_action(&mut self, action: CircuitAction) {
        let Some(target) = self.circuit_target() else { return };
        self.circuit_popover.close(&mut self.ui, &self.config);
        match action {
            CircuitAction::NewCircuit => {
                self.net.manager.new_circuit(&target.connection, &target.host);
                if let Some(view) = self.active_view() {
                    self.engine.reload(view, true);
                }
                self.show_toast(
                    Icon::Route,
                    &tr_fmt!("New Tor circuit for {host}", host = site_label(&target.host)),
                );
            }
            CircuitAction::NewIdentity => {
                self.net.manager.new_identity(&target.connection);
                let views: Vec<ViewId> = self
                    .tabs
                    .all()
                    .iter()
                    .filter(|t| {
                        let url =
                            net::parse_blocked_url(&t.url).map_or_else(|| t.url.clone(), |(url, _)| url);
                        self.circuit_target_of(&url, t.private)
                            .is_some_and(|c| c.connection == target.connection)
                    })
                    .map(|t| t.view)
                    .collect();
                for view in views {
                    self.engine.reload(view, true);
                }
                self.show_toast(Icon::Route, tr("New Tor identity"));
            }
        }
    }

    /// Every loop iteration: once a second, an open circuit popover looks at the circuit again
    /// (it changes as pages connect), and the network popover at the bootstrap progress while
    /// a connection starts.
    pub(super) fn circuit_tick(&mut self) {
        if self.circuit_refreshed.elapsed() < REFRESH {
            return;
        }
        self.circuit_refreshed = Instant::now();
        self.refresh_circuit_popover();
        if self.chip_shown == Some(ChipState::Connecting) {
            self.refresh_network_popover();
        }
    }
}

/// The page's host without a leading `www.` for the title, or the onion address as it is.
fn site_label(host: &str) -> String {
    host.strip_prefix("www.").unwrap_or(host).to_owned()
}

fn hop_row(hop: &Hop) -> CircuitRow {
    let role = match hop.role {
        HopRole::Guard => tr("Guard"),
        HopRole::Middle => tr("Middle"),
        HopRole::Exit => tr("Exit"),
        HopRole::Rendezvous => tr("Rendezvous"),
        HopRole::OnionService => tr("Onion service"),
    };
    if hop.role == HopRole::OnionService {
        return CircuitRow {
            kind: RowKind::Relay,
            badge: Some("TOR".to_owned()),
            title: role.to_owned(),
            detail: String::new(),
        };
    }
    let language = match axion_i18n::language() {
        axion_i18n::Language::German => NameLanguage::German,
        axion_i18n::Language::English => NameLanguage::English,
    };
    let (badge, country) = match hop.country.as_deref() {
        Some(code) => {
            (code.to_owned(), country_name(code, language).map_or_else(|| code.to_owned(), str::to_owned))
        }
        None => ("??".to_owned(), tr("Unknown country").to_owned()),
    };
    CircuitRow {
        kind: RowKind::Relay,
        badge: Some(badge),
        title: tr_fmt!("{country} ({role})", country = country, role = role),
        detail: hop.ip_text(),
    }
}

#[cfg(test)]
mod tests {
    use axion_net::{HopRole, ProgressStage};

    use super::*;

    fn hop(role: HopRole, country: Option<&str>, ip: &str) -> Hop {
        Hop {
            role,
            fingerprint: "AB".repeat(20),
            addrs: vec![ip.parse().expect("ip")],
            country: country.map(str::to_owned),
        }
    }

    #[test]
    fn hop_rows_name_country_role_and_address() {
        axion_i18n::set_language(axion_i18n::Language::English);
        let row = hop_row(&hop(HopRole::Guard, Some("DE"), "185.220.101.46"));
        assert_eq!(row.badge.as_deref(), Some("DE"));
        assert_eq!(row.title, "Germany (Guard)");
        assert_eq!(row.detail, "185.220.101.46");
        let row = hop_row(&hop(HopRole::Exit, None, "192.0.2.1"));
        assert_eq!(row.badge.as_deref(), Some("??"));
        assert_eq!(row.title, "Unknown country (Exit)");
        let row = hop_row(&Hop {
            role: HopRole::OnionService,
            fingerprint: String::new(),
            addrs: vec![],
            country: None,
        });
        assert_eq!(row.title, "Onion service");
        assert!(row.detail.is_empty());
    }

    #[test]
    fn bootstrap_lines() {
        axion_i18n::set_language(axion_i18n::Language::English);
        assert_eq!(net::bootstrap_text(None), "Starting Tor…");
        let p = |percent, stage| axion_net::TunnelProgress { percent, stage, stuck: None };
        assert_eq!(
            net::bootstrap_text(Some(p(40, ProgressStage::Relays))),
            "Bootstrapping 40%: loading relays"
        );
        let stuck = axion_net::TunnelProgress {
            percent: 15,
            stage: ProgressStage::Consensus,
            stuck: Some("We seem to be offline".into()),
        };
        assert_eq!(net::bootstrap_text(Some(stuck)), "Bootstrapping 15%: We seem to be offline");
    }

    #[test]
    fn site_labels() {
        assert_eq!(site_label("www.example.com"), "example.com");
        assert_eq!(site_label("abc.onion"), "abc.onion");
    }
}
