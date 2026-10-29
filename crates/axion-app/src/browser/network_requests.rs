//! Requests from the Network & VPN settings page: storing connections (secrets in the
//! vault), removing them, connecting, and reading WireGuard configs.

use axion_net::{Connection, ConnectionKind, ConnectionSecrets, ConnectionStatus, Secret, Target};

use super::*;
use crate::network as net;
use crate::settings::{ConnectionDraft, ConnectionRow, NetRequest};

impl Browser {
    /// A request from the Network & VPN page.
    pub(super) fn network_request(&mut self, request: NetRequest) {
        match request {
            NetRequest::Save(draft) => self.network_save(*draft),
            NetRequest::Remove(id) => self.network_remove(&id),
            NetRequest::Toggle(id) => {
                match self.net.manager.status(&id) {
                    Some(ConnectionStatus::Up { .. } | ConnectionStatus::Connecting) => {
                        self.net.manager.stop(&id)
                    }
                    _ => self.net.manager.start(&id),
                }
                self.network_tick();
            }
            NetRequest::ImportConf => self.conf_chooser.open(),
            NetRequest::PasteConf => {
                let text = self.clipboard.as_mut().and_then(|c| c.get_text().ok());
                match text {
                    Some(text) => self.wireguard_conf_text(&text),
                    None => {
                        self.settings.set_wireguard_import(Err(tr("The clipboard holds no text.").to_owned()))
                    }
                }
                self.refresh_settings();
            }
            NetRequest::LoadConf(path) => self.load_wireguard_conf(&path),
            NetRequest::CheckIp => {
                let url = self.config.network.check_ip_url.clone();
                if url.starts_with("https://") || url.starts_with("http://") {
                    if let Err(e) = self.open_tab(&url, true) {
                        tracing::error!("opening the address check failed: {e:#}");
                    }
                    self.close_settings();
                }
            }
        }
    }

    /// The file chooser answered.
    pub(super) fn network_chooser_tick(&mut self) {
        if let Some(path) = self.conf_chooser.take() {
            self.load_wireguard_conf(&path);
        }
    }

    fn load_wireguard_conf(&mut self, path: &std::path::Path) {
        let text = match std::fs::metadata(path) {
            Ok(m) if m.len() as usize > axion_net::wgconf::MAX_CONF_BYTES => {
                self.settings.set_wireguard_import(Err(
                    tr("That file is too large for a WireGuard config.").to_owned()
                ));
                self.refresh_settings();
                return;
            }
            Ok(_) => std::fs::read_to_string(path),
            Err(e) => Err(e),
        };
        match text {
            Ok(text) => self.wireguard_conf_text(&text),
            Err(e) => {
                tracing::warn!(error = %e, "reading a WireGuard config failed");
                self.settings.set_wireguard_import(Err(tr("That file couldn't be read.").to_owned()));
            }
        }
        self.refresh_settings();
    }

    fn wireguard_conf_text(&mut self, text: &str) {
        let result = match axion_net::wgconf::parse(text) {
            Ok(parsed) => Ok((parsed.config, parsed.ignored)),
            // The error names the line, never its content (keys).
            Err(e) => Err(e.to_string()),
        };
        self.settings.set_wireguard_import(result);
    }

    /// Stores a connection from the form: the secrets in the vault, the rest in the settings.
    fn network_save(&mut self, draft: ConnectionDraft) {
        let id = draft.id.clone().unwrap_or_else(Connection::new_id);
        let is_new = draft.id.is_none();
        let old_secrets: Option<ConnectionSecrets> = self
            .passwords
            .network_secrets()
            .and_then(|items| items.into_iter().find(|(i, _)| *i == id))
            .and_then(|(_, text)| net::secrets_from_json(&text));
        let mut secrets = old_secrets.clone().unwrap_or_default();
        let mut write = false;
        let mut wipe = false;
        match &draft.kind {
            ConnectionKind::Proxy(p) => {
                if !p.has_auth() {
                    secrets.password = Secret::default();
                    wipe = old_secrets.is_some();
                } else if let Some(password) = &draft.password {
                    secrets.password = password.clone();
                    write = true;
                }
            }
            ConnectionKind::WireGuard(_) => {
                if let Some(wg) = &draft.wireguard {
                    secrets.wireguard = Some(wg.clone());
                    write = true;
                }
            }
            ConnectionKind::Tor(_) | ConnectionKind::Direct => wipe = old_secrets.is_some(),
        }
        if write || wipe {
            if !self.passwords.is_unlocked() {
                self.settings.set_network_error(Some(
                    tr("To store the password or keys, unlock Passwords first (Settings › Passwords).")
                        .to_owned(),
                ));
                self.refresh_settings();
                return;
            }
            let result = if wipe && !write {
                self.passwords.remove_network_secret(&id)
            } else {
                self.passwords.set_network_secret(&id, &net::secrets_to_json(&secrets))
            };
            if let Err(e) = result {
                tracing::error!(error = %e, "storing connection secrets failed");
                self.settings
                    .set_network_error(Some(tr("The secrets couldn't be stored in the vault.").to_owned()));
                self.refresh_settings();
                return;
            }
            self.net.cache_secrets(&id, &secrets);
        }
        let connection = Connection { id: id.clone(), name: draft.name, kind: draft.kind };
        match self.config.network.connections.iter_mut().find(|c| c.id == id) {
            Some(existing) => *existing = connection,
            None => self.config.network.connections.push(connection),
        }
        self.config_dirty_since.get_or_insert(Instant::now());
        self.apply_network_config();
        if is_new {
            self.net.manager.start(&id);
        }
        self.settings.close_network_form();
        self.refresh_settings();
        self.network_tick();
    }

    fn network_remove(&mut self, id: &str) {
        let used = |t: &Target| t.connection_id() == Some(id);
        let network = &mut self.config.network;
        network.connections.retain(|c| c.id != id);
        // Whatever used it goes back to the default (a dangling route would block, which is
        // safe but confusing).
        if used(&network.default_connection) {
            network.default_connection = Target::Direct;
        }
        if network.private_connection.as_ref().is_some_and(used) {
            network.private_connection = None;
        }
        network.site_rules.retain(|r| !used(&r.connection));
        self.net.forget_secrets(id);
        if self.passwords.is_unlocked() {
            let _ = self.passwords.remove_network_secret(id);
        }
        self.config_dirty_since.get_or_insert(Instant::now());
        self.apply_network_config();
        self.refresh_settings();
        self.network_tick();
    }

    /// The connections' states for the settings page.
    pub(super) fn push_network_state(&mut self) {
        let rows = self
            .config
            .network
            .connections
            .iter()
            .filter_map(|c| {
                let status = self.net.manager.status(&c.id)?;
                let progress = (status == axion_net::ConnectionStatus::Connecting)
                    .then(|| self.net.manager.progress(&c.id))
                    .flatten()
                    .map(|p| crate::network::bootstrap_text(Some(p)));
                Some(ConnectionRow { id: c.id.clone(), status, progress })
            })
            .collect();
        self.settings.set_network_state(rows, self.net.vault_unlocked());
    }
}
