//! Settings › Network & VPN: the connections (proxy, WireGuard, Tor) with their state, the
//! form to add or edit one, the routes (this account, private tabs, per-site rules) and the
//! privacy switches. Secrets typed here go to the vault (see `crate::network`), never to
//! `config.toml`.

use axion_net::{
    Connection, ConnectionKind, ConnectionStatus, ProxyConfig, ProxyScheme, Secret, Target, TorConfig,
    TorIsolation, WgConfig, WgSecrets,
};

use super::*;

/// Ids of the page's clicks (see `SettingsAction::from_id`).
const NET_BASE: u32 = 1800;
/// Connections and site rules the page lists.
pub const MAX_CONNECTIONS: usize = 12;
pub const MAX_RULES: usize = 30;

/// A click on the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetAction {
    AddProxy,
    AddWireGuard,
    AddTor,
    Save,
    Cancel,
    ImportConf,
    PasteConf,
    LoadConfPath,
    CheckIp,
    NewRuleTarget,
    AddRule,
    SaveCheckUrl,
    TorBridges,
    TorIsolate(TorIsolation),
    Scheme(ProxyScheme),
    /// 0 = direct, then the connections.
    Default(usize),
    /// 0 = same as the account, 1 = direct, then the connections.
    Private(usize),
    Edit(usize),
    /// Connect or disconnect.
    Toggle(usize),
    Remove(usize),
    RuleTarget(usize),
    RemoveRule(usize),
}

impl NetAction {
    pub fn id(self) -> u32 {
        NET_BASE
            + match self {
                NetAction::AddProxy => 0,
                NetAction::AddWireGuard => 1,
                NetAction::AddTor => 2,
                NetAction::Save => 3,
                NetAction::Cancel => 4,
                NetAction::ImportConf => 5,
                NetAction::PasteConf => 6,
                NetAction::LoadConfPath => 7,
                NetAction::CheckIp => 8,
                NetAction::NewRuleTarget => 9,
                NetAction::AddRule => 10,
                NetAction::SaveCheckUrl => 11,
                NetAction::TorBridges => 12,
                NetAction::TorIsolate(m) => {
                    160 + TorIsolation::ALL.iter().position(|x| *x == m).unwrap_or(0) as u32
                }
                NetAction::Scheme(s) => {
                    16 + ProxyScheme::ALL.iter().position(|x| *x == s).unwrap_or(0) as u32
                }
                NetAction::Default(i) => 20 + i.min(MAX_CONNECTIONS) as u32,
                NetAction::Private(i) => 34 + i.min(MAX_CONNECTIONS + 1) as u32,
                NetAction::Edit(i) => 50 + i.min(MAX_CONNECTIONS - 1) as u32,
                NetAction::Toggle(i) => 62 + i.min(MAX_CONNECTIONS - 1) as u32,
                NetAction::Remove(i) => 74 + i.min(MAX_CONNECTIONS - 1) as u32,
                NetAction::RuleTarget(i) => 90 + i.min(MAX_RULES - 1) as u32,
                NetAction::RemoveRule(i) => 120 + i.min(MAX_RULES - 1) as u32,
            }
    }

    pub fn from_id(n: u32) -> Option<Self> {
        let i = n.checked_sub(NET_BASE)?;
        let at = |base: u32, len: usize| ((i - base) as usize).min(len);
        Some(match i {
            0 => NetAction::AddProxy,
            1 => NetAction::AddWireGuard,
            2 => NetAction::AddTor,
            3 => NetAction::Save,
            4 => NetAction::Cancel,
            5 => NetAction::ImportConf,
            6 => NetAction::PasteConf,
            7 => NetAction::LoadConfPath,
            8 => NetAction::CheckIp,
            9 => NetAction::NewRuleTarget,
            10 => NetAction::AddRule,
            11 => NetAction::SaveCheckUrl,
            12 => NetAction::TorBridges,
            160..=162 => NetAction::TorIsolate(TorIsolation::ALL[(i - 160) as usize]),
            16..=18 => NetAction::Scheme(ProxyScheme::ALL[(i - 16) as usize]),
            20..=33 if (i - 20) as usize <= MAX_CONNECTIONS => NetAction::Default(at(20, MAX_CONNECTIONS)),
            34..=48 if (i - 34) as usize <= MAX_CONNECTIONS + 1 => {
                NetAction::Private(at(34, MAX_CONNECTIONS + 1))
            }
            50..=61 => NetAction::Edit(at(50, MAX_CONNECTIONS - 1)),
            62..=73 => NetAction::Toggle(at(62, MAX_CONNECTIONS - 1)),
            74..=85 => NetAction::Remove(at(74, MAX_CONNECTIONS - 1)),
            90..=119 => NetAction::RuleTarget(at(90, MAX_RULES - 1)),
            120..=149 => NetAction::RemoveRule(at(120, MAX_RULES - 1)),
            _ => return None,
        })
    }
}

/// What the browser is asked to do.
#[derive(Clone, Debug)]
pub enum NetRequest {
    /// Save a connection (new when `id` is `None`).
    Save(Box<ConnectionDraft>),
    Remove(String),
    /// Connect it, or disconnect it when it is up.
    Toggle(String),
    /// Pick a WireGuard `.conf` with the file chooser.
    ImportConf,
    /// Read a WireGuard config from the clipboard.
    PasteConf,
    LoadConf(PathBuf),
    /// Open the configured "check my IP" address in a tab.
    CheckIp,
}

/// A connection as the form describes it.
#[derive(Clone, Debug)]
pub struct ConnectionDraft {
    pub id: Option<String>,
    pub name: String,
    pub kind: ConnectionKind,
    /// A new proxy password (`None`: keep the saved one).
    pub password: Option<Secret>,
    /// WireGuard keys of a newly imported config (`None`: keep the saved ones).
    pub wireguard: Option<WgSecrets>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormKind {
    Proxy,
    WireGuard,
    Tor,
}

/// The form's text fields (valid for one build of the page).
#[derive(Clone, Copy, Default)]
struct Fields {
    name: Option<NodeId>,
    host: Option<NodeId>,
    port: Option<NodeId>,
    user: Option<NodeId>,
    password: Option<NodeId>,
    path: Option<NodeId>,
    bridges: Option<NodeId>,
    exit: Option<NodeId>,
}

/// A connection's form, open.
#[derive(Clone)]
struct Editor {
    kind: FormKind,
    /// `None`: a new connection.
    id: Option<String>,
    scheme: ProxyScheme,
    /// What the fields held when the page was last built (they are rebuilt with it).
    name: String,
    host: String,
    port: String,
    user: String,
    password: String,
    path: String,
    bridges: String,
    exit: String,
    use_bridges: bool,
    isolation: TorIsolation,
    /// The saved connection has a password (leave the field empty to keep it).
    has_saved_password: bool,
    wg: Option<WgConfig>,
    wg_secrets: Option<WgSecrets>,
    wg_note: Option<(String, bool)>,
    error: Option<String>,
    fields: Fields,
}

impl Editor {
    fn new(kind: FormKind) -> Self {
        Self {
            kind,
            id: None,
            scheme: ProxyScheme::Socks5,
            name: String::new(),
            host: String::new(),
            port: String::new(),
            user: String::new(),
            password: String::new(),
            path: String::new(),
            bridges: String::new(),
            exit: String::new(),
            use_bridges: false,
            isolation: TorIsolation::default(),
            has_saved_password: false,
            wg: None,
            wg_secrets: None,
            wg_note: None,
            error: None,
            fields: Fields::default(),
        }
    }
}

/// A connection's state for the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionRow {
    pub id: String,
    pub status: ConnectionStatus,
    /// How far a starting connection has come ("Bootstrapping 40%: loading relays").
    pub progress: Option<String>,
}

/// What the page holds besides the config.
#[derive(Default)]
pub struct NetView {
    editor: Option<Editor>,
    statuses: Vec<ConnectionRow>,
    vault_unlocked: bool,
    confirm_remove: Option<usize>,
    /// Target of the rule being added: 0 = direct, then the connections.
    new_rule_target: usize,
    rule_host: Option<NodeId>,
    check_url: Option<NodeId>,
}

impl NetView {
    fn status_of(&self, id: &str) -> Option<&ConnectionStatus> {
        self.statuses.iter().find(|r| r.id == id).map(|r| &r.status)
    }

    fn progress_of(&self, id: &str) -> Option<String> {
        self.statuses.iter().find(|r| r.id == id).and_then(|r| r.progress.clone())
    }
}

impl SettingsSheet {
    /// The connections' states (the browser pushes them whenever one changes).
    pub fn set_network_state(&mut self, statuses: Vec<ConnectionRow>, vault_unlocked: bool) {
        self.net.statuses = statuses;
        self.net.vault_unlocked = vault_unlocked;
    }

    /// An error under the form (the save failed).
    pub fn set_network_error(&mut self, error: Option<String>) {
        if let Some(editor) = &mut self.net.editor {
            editor.error = error;
        }
    }

    /// The form is done (saved).
    pub fn close_network_form(&mut self) {
        self.net.editor = None;
    }

    /// A WireGuard config was read (or failed to): the form shows it.
    pub fn set_wireguard_import(&mut self, result: Result<(WgConfig, Vec<String>), String>) {
        let Some(editor) = &mut self.net.editor else { return };
        if editor.kind != FormKind::WireGuard {
            return;
        }
        match result {
            Ok((config, ignored)) => {
                let (stored, secrets) = config.split_secrets();
                if editor.name.trim().is_empty() {
                    editor.name = stored
                        .peers
                        .iter()
                        .find_map(|p| {
                            axion_net::wgconf::split_endpoint(&p.endpoint).map(|(h, _)| h.to_owned())
                        })
                        .unwrap_or_else(|| "WireGuard".to_owned());
                }
                let mut note = tr("Config read. Keys will be stored in the password vault.").to_owned();
                if !ignored.is_empty() {
                    note.push(' ');
                    note.push_str(&tr_fmt!("Ignored: {items}.", items = ignored.join(", ")));
                }
                editor.wg = Some(stored);
                editor.wg_secrets = Some(secrets);
                editor.wg_note = Some((note, false));
                editor.error = None;
            }
            Err(message) => editor.wg_note = Some((message, true)),
        }
    }

    /// Reads what the fields hold into the form (before the page is rebuilt).
    pub(super) fn capture_network_form(&mut self, ui: &Ui) {
        let Some(editor) = &mut self.net.editor else { return };
        let f = editor.fields;
        let read = |node: Option<NodeId>| node.and_then(|n| ui.field_text(n)).map(str::to_owned);
        if let Some(t) = read(f.name) {
            editor.name = t;
        }
        if let Some(t) = read(f.host) {
            editor.host = t;
        }
        if let Some(t) = read(f.port) {
            editor.port = t;
        }
        if let Some(t) = read(f.user) {
            editor.user = t;
        }
        if let Some(t) = read(f.password) {
            editor.password = t;
        }
        if let Some(t) = read(f.path) {
            editor.path = t;
        }
        if let Some(t) = read(f.bridges) {
            editor.bridges = t;
        }
        if let Some(t) = read(f.exit) {
            editor.exit = t;
        }
    }

    pub(super) fn network_action(&mut self, ui: &mut Ui, action: NetAction, config: &mut Config) -> Outcome {
        self.capture_network_form(ui);
        let conns = config.network.connections.clone();
        let keep_confirm = matches!(action, NetAction::Remove(_));
        if !keep_confirm {
            self.net.confirm_remove = None;
        }
        match action {
            NetAction::AddProxy => self.net.editor = Some(Editor::new(FormKind::Proxy)),
            NetAction::AddWireGuard => self.net.editor = Some(Editor::new(FormKind::WireGuard)),
            NetAction::AddTor => self.net.editor = Some(Editor::new(FormKind::Tor)),
            NetAction::Cancel => self.net.editor = None,
            NetAction::Scheme(s) => {
                if let Some(e) = &mut self.net.editor {
                    e.scheme = s;
                }
            }
            NetAction::TorBridges => {
                if let Some(e) = &mut self.net.editor {
                    e.use_bridges = !e.use_bridges;
                }
            }
            NetAction::TorIsolate(mode) => {
                if let Some(e) = &mut self.net.editor {
                    e.isolation = mode;
                }
            }
            NetAction::Edit(i) => {
                if let Some(c) = conns.get(i) {
                    self.net.editor = Some(editor_for(c));
                }
            }
            NetAction::Toggle(i) => {
                if let Some(c) = conns.get(i) {
                    return Outcome::Network(NetRequest::Toggle(c.id.clone()));
                }
            }
            NetAction::Remove(i) => {
                if self.net.confirm_remove == Some(i) {
                    self.net.confirm_remove = None;
                    if let Some(c) = conns.get(i) {
                        return Outcome::Network(NetRequest::Remove(c.id.clone()));
                    }
                } else {
                    self.net.confirm_remove = Some(i);
                }
            }
            NetAction::ImportConf => return Outcome::Network(NetRequest::ImportConf),
            NetAction::PasteConf => return Outcome::Network(NetRequest::PasteConf),
            NetAction::LoadConfPath => {
                let path = self.net.editor.as_ref().map(|e| e.path.trim().to_owned()).unwrap_or_default();
                if !path.is_empty() {
                    let path = path.strip_prefix("~/").map_or_else(
                        || PathBuf::from(&path),
                        |rest| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest),
                    );
                    return Outcome::Network(NetRequest::LoadConf(path));
                }
            }
            NetAction::Save => return self.save_form(),
            NetAction::Default(i) => {
                config.network.default_connection = match i {
                    0 => Target::Direct,
                    n => conns.get(n - 1).map_or(Target::Direct, |c| Target::Connection(c.id.clone())),
                };
                return Outcome::Changed;
            }
            NetAction::Private(i) => {
                config.network.private_connection = match i {
                    0 => None,
                    1 => Some(Target::Direct),
                    n => conns.get(n - 2).map(|c| Target::Connection(c.id.clone())),
                };
                return Outcome::Changed;
            }
            NetAction::NewRuleTarget => {
                self.net.new_rule_target = (self.net.new_rule_target + 1) % (conns.len() + 1);
            }
            NetAction::AddRule => {
                let text = self.net.rule_host.and_then(|f| ui.field_text(f)).unwrap_or_default().to_owned();
                let target = target_at(&conns, self.net.new_rule_target);
                if config.network.site_rules.len() < MAX_RULES && axion_net::normalize_host(&text).is_some() {
                    let mut routing = config.network.routing();
                    routing.set_site_rule(&text, Some(target));
                    config.network.site_rules = routing.site_rules;
                    return Outcome::Changed;
                }
            }
            NetAction::RuleTarget(i) => {
                if let Some(rule) = config.network.site_rules.get_mut(i) {
                    let now = conns
                        .iter()
                        .position(|c| rule.connection.connection_id() == Some(c.id.as_str()))
                        .map_or(0, |p| p + 1);
                    rule.connection = target_at(&conns, (now + 1) % (conns.len() + 1));
                    return Outcome::Changed;
                }
            }
            NetAction::RemoveRule(i) => {
                if i < config.network.site_rules.len() {
                    config.network.site_rules.remove(i);
                    return Outcome::Changed;
                }
            }
            NetAction::SaveCheckUrl => {
                let text =
                    self.net.check_url.and_then(|f| ui.field_text(f)).unwrap_or_default().trim().to_owned();
                if text.is_empty() || text.starts_with("https://") || text.starts_with("http://") {
                    config.network.check_ip_url = text;
                    return Outcome::Changed;
                }
            }
            NetAction::CheckIp => return Outcome::Network(NetRequest::CheckIp),
        }
        self.rebuild = true;
        Outcome::Nothing
    }

    fn save_form(&mut self) -> Outcome {
        let Some(e) = &mut self.net.editor else { return Outcome::Nothing };
        e.error = None;
        let name = e.name.trim().to_owned();
        let mut draft = ConnectionDraft {
            id: e.id.clone(),
            name: name.clone(),
            kind: ConnectionKind::Direct,
            password: None,
            wireguard: None,
        };
        match e.kind {
            FormKind::Proxy => {
                let Ok(port) = e.port.trim().parse::<u16>() else {
                    e.error = Some(tr("The port is a number from 1 to 65535.").to_owned());
                    self.rebuild = true;
                    return Outcome::Nothing;
                };
                let proxy = ProxyConfig {
                    scheme: e.scheme,
                    host: e.host.trim().to_owned(),
                    port,
                    username: e.user.trim().to_owned(),
                };
                if let Err(err) = proxy.validate() {
                    e.error = Some(err.to_string());
                    self.rebuild = true;
                    return Outcome::Nothing;
                }
                if draft.name.is_empty() {
                    draft.name = format!("{} {}", proxy.scheme.label(), proxy.host);
                }
                draft.password = (!e.password.is_empty()).then(|| Secret::new(e.password.clone()));
                draft.kind = ConnectionKind::Proxy(proxy);
            }
            FormKind::WireGuard => {
                let Some(wg) = e.wg.clone() else {
                    e.error = Some(tr("Import a WireGuard config first.").to_owned());
                    self.rebuild = true;
                    return Outcome::Nothing;
                };
                if draft.name.is_empty() {
                    draft.name = "WireGuard".to_owned();
                }
                draft.wireguard = e.wg_secrets.clone();
                draft.kind = ConnectionKind::WireGuard(wg);
            }
            FormKind::Tor => {
                let tor = TorConfig {
                    use_bridges: e.use_bridges,
                    bridges: e
                        .bridges
                        .split([';', '|'])
                        .map(|b| b.trim().to_owned())
                        .filter(|b| !b.is_empty())
                        .collect(),
                    exit_country: e.exit.trim().to_ascii_lowercase(),
                    isolation: e.isolation,
                };
                if let Err(err) = tor.validate() {
                    e.error = Some(err.to_string());
                    self.rebuild = true;
                    return Outcome::Nothing;
                }
                if draft.name.is_empty() {
                    draft.name = "Tor".to_owned();
                }
                draft.kind = ConnectionKind::Tor(tor);
            }
        }
        Outcome::Network(NetRequest::Save(Box::new(draft)))
    }

    /// The page.
    pub(super) fn network_page(&mut self, ui: &mut Ui, config: &Config) {
        self.net.rule_host = None;
        self.net.check_url = None;
        let network = &config.network;
        let conns = &network.connections;

        self.section(ui, tr("Connections"));
        if conns.is_empty() {
            self.row(ui, tr("No connections yet"));
        }
        for (i, c) in conns.iter().enumerate().take(MAX_CONNECTIONS) {
            let status = self.net.status_of(&c.id).cloned().unwrap_or(ConnectionStatus::Stopped);
            let title = format!("{}  ·  {}", c.name, crate::network::describe(c));
            let row_node = self.row_bare(ui, 52.0);
            let (color, text) = status_look(&status, &c.kind);
            let text = match (&status, self.net.progress_of(&c.id)) {
                (ConnectionStatus::Connecting, Some(p)) => p,
                _ => text,
            };
            ui.add(
                row_node,
                Widget::container()
                    .with_background(Background::Fill(Paint::Fixed(color), 1.0))
                    .with_radius(CAPSULE),
                Style {
                    size: Size { width: length(10.0), height: length(10.0) },
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            // Two lines: the name and kind, then the status (a failure reason in full).
            let column = ui.add(
                row_node,
                Widget::container(),
                Style {
                    flex_direction: FlexDirection::Column,
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_size: Size { width: length(0.0), height: auto() },
                    ..Default::default()
                },
            );
            let name = ui.add_label(
                column,
                &title,
                TextStyle::Body,
                Style { min_size: Size { width: length(0.0), height: auto() }, ..Default::default() },
            );
            ui.set_label_lines(name, 2);
            let label = ui.add_label(
                column,
                &text,
                TextStyle::Footnote,
                Style { min_size: Size { width: length(0.0), height: auto() }, ..Default::default() },
            );
            ui.set_label_lines(label, 6);
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
            let up = status.is_up() || matches!(status, ConnectionStatus::Connecting);
            glass_pill(
                ui,
                row_node,
                if up { tr("Disconnect") } else { tr("Connect") },
                SettingsAction::Network(NetAction::Toggle(i)),
            );
            glass_pill(ui, row_node, tr("Edit"), SettingsAction::Network(NetAction::Edit(i)));
            let confirming = self.net.confirm_remove == Some(i);
            destructive_button(
                ui,
                row_node,
                if confirming { tr("Click Again to Remove") } else { tr("Remove") },
                SettingsAction::Network(NetAction::Remove(i)),
                confirming,
            );
        }
        if !self.net.vault_unlocked && conns.iter().any(needs_vault) {
            self.note(
                ui,
                tr("These connections keep a password or key in your password vault. Unlock Passwords (Settings › Passwords) and they connect."),
            );
        }
        let row_node = self.button_row(ui);
        glass_pill(ui, row_node, tr("Add Proxy…"), SettingsAction::Network(NetAction::AddProxy));
        glass_pill(ui, row_node, tr("Add WireGuard…"), SettingsAction::Network(NetAction::AddWireGuard));
        glass_pill(ui, row_node, tr("Add Tor…"), SettingsAction::Network(NetAction::AddTor));

        if self.net.editor.is_some() {
            self.network_form(ui);
        }

        self.section(ui, tr("Routes"));
        let names: Vec<String> = conns.iter().map(|c| c.name.clone()).collect();
        let row_node = self.row(ui, tr("Tabs of this account"));
        let mut options: Vec<(&str, ActionId)> =
            vec![(tr("Direct"), SettingsAction::Network(NetAction::Default(0)).id())];
        options.extend(
            names
                .iter()
                .enumerate()
                .take(MAX_CONNECTIONS)
                .map(|(i, n)| (n.as_str(), SettingsAction::Network(NetAction::Default(i + 1)).id())),
        );
        let current = match &network.default_connection {
            Target::Direct => Some(0),
            Target::Connection(id) => conns.iter().position(|c| &c.id == id).map(|p| p + 1),
        };
        segmented(ui, row_node, &options, current);
        let row_node = self.row(ui, tr("Private tabs"));
        let mut options: Vec<(&str, ActionId)> = vec![
            (tr("Same as account"), SettingsAction::Network(NetAction::Private(0)).id()),
            (tr("Direct"), SettingsAction::Network(NetAction::Private(1)).id()),
        ];
        options.extend(
            names
                .iter()
                .enumerate()
                .take(MAX_CONNECTIONS)
                .map(|(i, n)| (n.as_str(), SettingsAction::Network(NetAction::Private(i + 2)).id())),
        );
        let current = match &network.private_connection {
            None => Some(0),
            Some(Target::Direct) => Some(1),
            Some(Target::Connection(id)) => conns.iter().position(|c| &c.id == id).map(|p| p + 2),
        };
        segmented(ui, row_node, &options, current);
        self.note(
            ui,
            tr("Sites with their own route below use that one in every tab. Everything else follows the choice above."),
        );

        self.section(ui, tr("Site rules"));
        for (i, rule) in network.site_rules.iter().enumerate().take(MAX_RULES) {
            let row_node = self.row_with_height(ui, &rule.host, 44.0);
            glass_pill(
                ui,
                row_node,
                &crate::network::target_label(network, &rule.connection),
                SettingsAction::Network(NetAction::RuleTarget(i)),
            );
            let remove = ui.add(
                row_node,
                Widget::container()
                    .with_action(SettingsAction::Network(NetAction::RemoveRule(i)).id())
                    .with_radius(CAPSULE)
                    .with_background(Background::Glass(GlassLevel::Thin)),
                Style {
                    size: Size { width: length(28.0), height: length(28.0) },
                    flex_shrink: 0.0,
                    justify_content: JustifyContent::CENTER,
                    ..row(0.0)
                },
            );
            ui.add(remove, Widget::icon(Icon::Close, 14.0).with_paint(Paint::Secondary), Style::default());
        }
        let row_node = self.row(ui, tr("Add a site"));
        self.net.rule_host = Some(text_field(ui, row_node, "example.com", 200.0, false));
        let target = target_at(conns, self.net.new_rule_target);
        glass_pill(
            ui,
            row_node,
            &crate::network::target_label(network, &target),
            SettingsAction::Network(NetAction::NewRuleTarget),
        );
        pill_button(ui, row_node, tr("Add"), SettingsAction::Network(NetAction::AddRule), false);
        self.note(
            ui,
            tr("A site rule covers the site and its subdomains. Click the connection's name to change it."),
        );

        self.section(ui, tr("Privacy"));
        for setting in [
            Setting::NetKillSwitch,
            Setting::NetLeakProtection,
            Setting::NetDisableQuic,
            Setting::NetStrictDns,
            Setting::NetConnectAtStart,
            Setting::NetShowChip,
            Setting::NetNotify,
        ] {
            self.control(ui, config, setting);
        }
        self.note(
            ui,
            tr("Kill switch: if a connection goes down, its requests are blocked and a page explains it. They are never sent directly. Leak protection keeps WebRTC from using your real address, turns off DNS prefetching and secure DNS, and stops Chromium's background services. QUIC and name-resolution settings apply after a restart. Addresses on this computer (localhost, 192.168.x.x) are never sent through a connection."),
        );
        self.section(ui, tr("Check your address"));
        let row_node = self.row(ui, tr("Address that shows your IP"));
        let field = text_field(ui, row_node, "https://", 260.0, false);
        ui.set_field_text(field, &network.check_ip_url);
        self.net.check_url = Some(field);
        glass_pill(ui, row_node, tr("Save"), SettingsAction::Network(NetAction::SaveCheckUrl));
        if !network.check_ip_url.is_empty() {
            let row_node = self.row(ui, tr("Open it in a tab, through this tab's route"));
            pill_button(ui, row_node, tr("Check My IP"), SettingsAction::Network(NetAction::CheckIp), false);
        }
        self.note(
            ui,
            tr("Axion never contacts this address by itself. Enter a service you trust; the button opens it like any page."),
        );
    }

    fn network_form(&mut self, ui: &mut Ui) {
        let Some(mut e) = self.net.editor.clone() else { return };
        let title = match (e.kind, e.id.is_some()) {
            (FormKind::Proxy, false) => tr("New proxy"),
            (FormKind::WireGuard, false) => tr("New WireGuard connection"),
            (FormKind::Tor, false) => tr("New Tor connection"),
            (_, true) => tr("Edit connection"),
        };
        self.section(ui, title);
        let available = match e.kind {
            FormKind::Proxy => true,
            FormKind::WireGuard => axion_net::wireguard::AVAILABLE,
            FormKind::Tor => axion_net::tor::AVAILABLE,
        };
        if !available {
            self.note(
                ui,
                tr("This kind of connection is coming in this build: you can set it up now, it connects as soon as the part that runs it is installed."),
            );
        }
        let row_node = self.row(ui, tr("Name"));
        let name = text_field(ui, row_node, tr("Optional"), 260.0, false);
        ui.set_field_text(name, &e.name);
        e.fields.name = Some(name);
        match e.kind {
            FormKind::Proxy => {
                let row_node = self.row(ui, tr("Type"));
                let options: Vec<(&str, ActionId)> = ProxyScheme::ALL
                    .iter()
                    .map(|s| (s.label(), SettingsAction::Network(NetAction::Scheme(*s)).id()))
                    .collect();
                let current = ProxyScheme::ALL.iter().position(|s| *s == e.scheme);
                segmented(ui, row_node, &options, current);
                let row_node = self.row(ui, tr("Host"));
                let host = text_field(ui, row_node, "proxy.example.net", 260.0, false);
                ui.set_field_text(host, &e.host);
                e.fields.host = Some(host);
                let row_node = self.row(ui, tr("Port"));
                let port = text_field(ui, row_node, "1080", 100.0, false);
                ui.set_field_text(port, &e.port);
                e.fields.port = Some(port);
                let row_node = self.row(ui, tr("User name"));
                let user = text_field(ui, row_node, tr("Optional"), 260.0, false);
                ui.set_field_text(user, &e.user);
                e.fields.user = Some(user);
                let row_node = self.row(ui, tr("Password"));
                let placeholder =
                    if e.has_saved_password { tr("Saved. Type to change it") } else { tr("Optional") };
                let password = text_field(ui, row_node, placeholder, 260.0, true);
                ui.set_field_text(password, &e.password);
                e.fields.password = Some(password);
                self.note(
                    ui,
                    tr("The password is stored in your password vault, not in the settings file. Names are always resolved by the proxy, never on this computer."),
                );
            }
            FormKind::WireGuard => {
                let row_node = self.row(ui, tr("Config file (.conf)"));
                glass_pill(ui, row_node, tr("Import .conf…"), SettingsAction::Network(NetAction::ImportConf));
                glass_pill(
                    ui,
                    row_node,
                    tr("Paste from Clipboard"),
                    SettingsAction::Network(NetAction::PasteConf),
                );
                let row_node = self.row(ui, tr("Or a path"));
                let path = text_field(ui, row_node, "~/wg0.conf", 220.0, false);
                ui.set_field_text(path, &e.path);
                e.fields.path = Some(path);
                glass_pill(ui, row_node, tr("Load"), SettingsAction::Network(NetAction::LoadConfPath));
                if let Some(wg) = &e.wg {
                    let endpoint = wg
                        .peers
                        .iter()
                        .find(|p| !p.endpoint.is_empty())
                        .map(|p| p.endpoint.as_str())
                        .unwrap_or("");
                    self.row(ui, &tr_fmt!("Peer {endpoint}", endpoint = endpoint));
                    self.row(
                        ui,
                        &tr_fmt!("Addresses {addresses}", addresses = wg.interface.addresses.join(", ")),
                    );
                    if !wg.interface.dns.is_empty() {
                        self.row(ui, &tr_fmt!("DNS {servers}", servers = wg.interface.dns.join(", ")));
                    }
                }
                if let Some((text, error)) = &e.wg_note {
                    self.note(ui, text);
                    if let Some(&label) = ui.children(self.content).last()
                        && let Some(w) = ui.widget_mut(label)
                    {
                        w.set_paint(if *error { Paint::Fixed(DESTRUCTIVE) } else { Paint::Accent });
                    }
                }
                self.note(
                    ui,
                    tr("The private key and any preshared keys go to your password vault. Scripts in the config (PostUp and the like) are never run."),
                );
            }
            FormKind::Tor => {
                let row_node = self.row(ui, tr("Use bridges"));
                glass_pill(
                    ui,
                    row_node,
                    if e.use_bridges { tr("On") } else { tr("Off") },
                    SettingsAction::Network(NetAction::TorBridges),
                );
                if e.use_bridges {
                    let row_node = self.row(ui, tr("Bridges"));
                    let bridges = text_field(ui, row_node, tr("One per entry, separated by ;"), 320.0, false);
                    ui.set_field_text(bridges, &e.bridges);
                    e.fields.bridges = Some(bridges);
                }
                let row_node = self.row(ui, tr("Exit country"));
                let exit = text_field(ui, row_node, tr("Any (or a code such as de)"), 200.0, false);
                ui.set_field_text(exit, &e.exit);
                e.fields.exit = Some(exit);
                self.note(
                    ui,
                    tr("Restricting the exit country narrows the crowd you blend into. Leave it empty unless you need it."),
                );
                let row_node = self.row(ui, tr("Circuits"));
                let options: Vec<(&str, ActionId)> = TorIsolation::ALL
                    .iter()
                    .map(|m| (isolation_label(*m), SettingsAction::Network(NetAction::TorIsolate(*m)).id()))
                    .collect();
                let current = TorIsolation::ALL.iter().position(|m| *m == e.isolation);
                segmented(ui, row_node, &options, current);
                self.note(
                    ui,
                    tr("Per site, every site gets its own Tor circuit, so no exit relay sees two of them. This is Tor in Axion's browser, not Tor Browser: pages can still recognise your browser, and only TCP traffic goes through Tor (no QUIC, UDP or WebRTC)."),
                );
            }
        }
        if let Some(error) = &e.error {
            self.note(ui, error);
            if let Some(&label) = ui.children(self.content).last()
                && let Some(w) = ui.widget_mut(label)
            {
                w.set_paint(Paint::Fixed(DESTRUCTIVE));
            }
        }
        let row_node = self.button_row(ui);
        glass_pill(ui, row_node, tr("Cancel"), SettingsAction::Network(NetAction::Cancel));
        primary_button(ui, row_node, tr("Save"), SettingsAction::Network(NetAction::Save));
        self.net.editor = Some(e);
    }
}

fn target_at(conns: &[Connection], index: usize) -> Target {
    match index.checked_sub(1).and_then(|i| conns.get(i)) {
        Some(c) => Target::Connection(c.id.clone()),
        None => Target::Direct,
    }
}

/// Whether the connection keeps something in the vault.
fn needs_vault(c: &Connection) -> bool {
    match &c.kind {
        ConnectionKind::Proxy(p) => p.has_auth(),
        ConnectionKind::WireGuard(_) => true,
        _ => false,
    }
}

fn editor_for(c: &Connection) -> Editor {
    let mut e = Editor::new(match &c.kind {
        ConnectionKind::Proxy(_) | ConnectionKind::Direct => FormKind::Proxy,
        ConnectionKind::WireGuard(_) => FormKind::WireGuard,
        ConnectionKind::Tor(_) => FormKind::Tor,
    });
    e.id = Some(c.id.clone());
    e.name = c.name.clone();
    match &c.kind {
        ConnectionKind::Proxy(p) => {
            e.scheme = p.scheme;
            e.host = p.host.clone();
            e.port = p.port.to_string();
            e.user = p.username.clone();
            e.has_saved_password = p.has_auth();
        }
        ConnectionKind::WireGuard(w) => {
            e.wg = Some(w.clone());
            e.wg_note =
                Some((tr("Keys are kept in the vault. Import a config to replace them.").to_owned(), false));
        }
        ConnectionKind::Tor(t) => {
            e.use_bridges = t.use_bridges;
            e.bridges = t.bridges.join("; ");
            e.exit = t.exit_country.clone();
            e.isolation = t.isolation;
        }
        ConnectionKind::Direct => {}
    }
    e
}

fn isolation_label(mode: TorIsolation) -> &'static str {
    match mode {
        TorIsolation::Site => tr("Per site"),
        TorIsolation::Context => tr("One for all"),
        TorIsolation::None => tr("Tor default"),
    }
}

/// The dot's colour and the status words.
fn status_look(status: &ConnectionStatus, kind: &ConnectionKind) -> (Color, String) {
    let green = Color::rgb(0x30B050);
    let red = Color::rgb(0xE2453A);
    let grey = Color::rgb(0x8E8E93);
    match status {
        ConnectionStatus::Up { info, .. } => (green, format!("{} · {info}", tr("Connected"))),
        ConnectionStatus::Connecting => (Color::rgb(0x0A84FF), tr("Connecting…").to_owned()),
        ConnectionStatus::Failed(reason) => (red, reason.clone()),
        ConnectionStatus::Blocked(reason) => (red, reason.clone()),
        ConnectionStatus::Stopped => {
            let text = if kind.is_available() { tr("Not connected") } else { tr("Coming in this build") };
            (grey, text.to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        let mut all = vec![
            NetAction::AddProxy,
            NetAction::AddWireGuard,
            NetAction::AddTor,
            NetAction::Save,
            NetAction::Cancel,
            NetAction::ImportConf,
            NetAction::PasteConf,
            NetAction::LoadConfPath,
            NetAction::CheckIp,
            NetAction::NewRuleTarget,
            NetAction::AddRule,
            NetAction::SaveCheckUrl,
            NetAction::TorBridges,
        ];
        all.extend(TorIsolation::ALL.map(NetAction::TorIsolate));
        all.extend(ProxyScheme::ALL.map(NetAction::Scheme));
        all.extend((0..=MAX_CONNECTIONS).map(NetAction::Default));
        all.extend((0..=MAX_CONNECTIONS + 1).map(NetAction::Private));
        for i in 0..MAX_CONNECTIONS {
            all.extend([NetAction::Edit(i), NetAction::Toggle(i), NetAction::Remove(i)]);
        }
        for i in 0..MAX_RULES {
            all.extend([NetAction::RuleTarget(i), NetAction::RemoveRule(i)]);
        }
        let mut seen = std::collections::HashSet::new();
        for action in all {
            assert_eq!(NetAction::from_id(action.id()), Some(action), "{action:?}");
            assert!(seen.insert(action.id()), "{action:?} shares an id");
        }
        assert_eq!(NetAction::from_id(NET_BASE - 1), None);
        assert_eq!(NetAction::from_id(NET_BASE + 13), None);
        assert_eq!(NetAction::from_id(NET_BASE + 400), None);
    }

    #[test]
    fn rule_targets() {
        let conns = vec![Connection { id: "a".into(), name: "A".into(), kind: ConnectionKind::Direct }];
        assert_eq!(target_at(&conns, 0), Target::Direct);
        assert_eq!(target_at(&conns, 1), Target::Connection("a".into()));
        assert_eq!(target_at(&conns, 5), Target::Direct);
    }

    #[test]
    fn editing_fills_the_form_without_secrets() {
        let c = Connection {
            id: "c-1".into(),
            name: "Office".into(),
            kind: ConnectionKind::Proxy(ProxyConfig {
                scheme: ProxyScheme::Http,
                host: "p.example".into(),
                port: 3128,
                username: "me".into(),
            }),
        };
        let e = editor_for(&c);
        assert_eq!((e.host.as_str(), e.port.as_str(), e.user.as_str()), ("p.example", "3128", "me"));
        assert!(e.password.is_empty() && e.has_saved_password);
    }
}
