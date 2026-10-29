//! What a named connection is: a proxy, a WireGuard tunnel or Tor. These types are saved in
//! `config.toml`, so they hold **no secrets**: passwords and keys live in the vault, keyed by
//! the connection's id (see [`crate::SecretStore`]).

use serde::{Deserialize, Serialize};

use crate::secret::Secret;
use crate::wireguard;

/// How an upstream proxy is spoken to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyScheme {
    /// SOCKS5; names are resolved by the proxy.
    #[default]
    Socks5,
    /// HTTP proxy (`CONNECT` for https sites).
    Http,
    /// HTTP proxy reached over TLS.
    Https,
}

impl ProxyScheme {
    pub const ALL: [ProxyScheme; 3] = [ProxyScheme::Socks5, ProxyScheme::Http, ProxyScheme::Https];

    pub fn label(self) -> &'static str {
        match self {
            ProxyScheme::Socks5 => "SOCKS5",
            ProxyScheme::Http => "HTTP",
            ProxyScheme::Https => "HTTPS",
        }
    }
}

/// An upstream proxy. The password is not here: it is `ConnectionSecrets::password`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProxyConfig {
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
    /// Empty: the proxy needs no login.
    pub username: String,
}

impl ProxyConfig {
    /// Whether the connection needs a login (a user name is set).
    pub fn has_auth(&self) -> bool {
        !self.username.is_empty()
    }

    /// Checks what a form can get wrong.
    pub fn validate(&self) -> Result<(), crate::NetError> {
        let host = self.host.trim();
        if host.is_empty() {
            return Err(crate::NetError::Invalid("the proxy needs a host".into()));
        }
        if host.chars().any(|c| c.is_whitespace() || matches!(c, '/' | '?' | '#' | '@' | '"' | '\'' | ';'))
            || host.contains("://")
        {
            return Err(crate::NetError::Invalid("the proxy host is not a valid name or address".into()));
        }
        if self.port == 0 {
            return Err(crate::NetError::Invalid("the proxy needs a port".into()));
        }
        if self.username.len() > 255 {
            return Err(crate::NetError::Invalid("the user name is too long".into()));
        }
        Ok(())
    }

    /// `host:port`, IPv6 in brackets.
    pub fn host_port(&self) -> String {
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// A parsed WireGuard `.conf`. The keys are `#[serde(skip)]`: they never reach `config.toml`
/// and are restored from the vault with [`WgConfig::with_secrets`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WgConfig {
    pub interface: WgInterface,
    pub peers: Vec<WgPeer>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WgInterface {
    /// Base64, 32 bytes. Vault only.
    #[serde(skip)]
    pub private_key: Secret,
    /// Addresses of the tunnel interface, `10.0.0.2/32` style.
    pub addresses: Vec<String>,
    /// DNS servers (reached through the tunnel); names after them are search domains.
    pub dns: Vec<String>,
    /// Tunnel MTU (0: choose, 1420).
    pub mtu: u16,
    /// 0: random.
    pub listen_port: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WgPeer {
    /// Base64, 32 bytes.
    pub public_key: String,
    /// Base64, 32 bytes, optional. Vault only.
    #[serde(skip)]
    pub preshared_key: Secret,
    /// Whether this peer has a preshared key in the vault (saved, so the UI can say so).
    pub has_preshared_key: bool,
    pub allowed_ips: Vec<String>,
    /// `host:port` or `[v6]:port`; empty for a peer that only connects to us.
    pub endpoint: String,
    /// Seconds; 0: off.
    pub persistent_keepalive: u16,
}

/// The secrets of a [`WgConfig`] as they are kept in the vault.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WgSecrets {
    pub private_key: Secret,
    /// Preshared keys by peer index (empty: none).
    pub preshared_keys: Vec<Secret>,
}

impl WgConfig {
    /// A copy without any secret, plus the secrets.
    pub fn split_secrets(mut self) -> (WgConfig, WgSecrets) {
        let secrets = WgSecrets {
            private_key: std::mem::take(&mut self.interface.private_key),
            preshared_keys: self.peers.iter_mut().map(|p| std::mem::take(&mut p.preshared_key)).collect(),
        };
        for (peer, key) in self.peers.iter_mut().zip(&secrets.preshared_keys) {
            peer.has_preshared_key = !key.is_empty();
        }
        (self, secrets)
    }

    /// Puts the vault's secrets back.
    pub fn with_secrets(mut self, secrets: &WgSecrets) -> WgConfig {
        self.interface.private_key = secrets.private_key.clone();
        for (peer, key) in self.peers.iter_mut().zip(&secrets.preshared_keys) {
            peer.preshared_key = key.clone();
        }
        self
    }

    /// The first endpoint's host (what must stay reachable outside the tunnel), if any.
    pub fn endpoint_hosts(&self) -> Vec<String> {
        self.peers
            .iter()
            .filter_map(|p| crate::wgconf::split_endpoint(&p.endpoint).map(|(h, _)| h.to_owned()))
            .collect()
    }
}

/// Tor options. Everything is optional; the defaults are Tor's own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TorConfig {
    /// Reach the Tor network through bridges (for networks that block Tor).
    pub use_bridges: bool,
    /// One bridge line per entry (`obfs4 1.2.3.4:443 FINGERPRINT cert=… iat-mode=0`).
    pub bridges: Vec<String>,
    /// Two-letter country code to leave the network from (`de`); empty: any. Narrows the
    /// anonymity set, off by default.
    pub exit_country: String,
    /// Which connections may share a circuit.
    pub isolation: TorIsolation,
}

/// Which connections share a Tor circuit (`isolation` of a Tor connection).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TorIsolation {
    /// One set of circuits per site (registrable domain): different sites cannot be linked by
    /// their exit relay. The default.
    #[default]
    Site,
    /// One set of circuits for everything this connection carries.
    Context,
    /// Tor's own default (no isolation beyond what Arti does itself).
    None,
}

impl TorIsolation {
    pub fn label(self) -> &'static str {
        match self {
            TorIsolation::Site => "site",
            TorIsolation::Context => "context",
            TorIsolation::None => "none",
        }
    }
}

impl TorConfig {
    pub fn validate(&self) -> Result<(), crate::NetError> {
        let c = self.exit_country.trim();
        if !(c.is_empty() || (c.len() == 2 && c.chars().all(|ch| ch.is_ascii_alphabetic()))) {
            return Err(crate::NetError::Invalid("the exit country is a two-letter code such as de".into()));
        }
        if self.use_bridges && self.bridges.iter().all(|b| b.trim().is_empty()) {
            return Err(crate::NetError::Invalid("bridges are on, but no bridge is listed".into()));
        }
        Ok(())
    }
}

/// What a connection does with the traffic.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ConnectionKind {
    /// No connection: straight out of this computer.
    #[default]
    Direct,
    Proxy(ProxyConfig),
    WireGuard(WgConfig),
    Tor(TorConfig),
}

impl ConnectionKind {
    /// Whether this build can run the kind (WireGuard and Tor arrive with their backends).
    pub fn is_available(&self) -> bool {
        match self {
            ConnectionKind::Direct | ConnectionKind::Proxy(_) => true,
            ConnectionKind::WireGuard(_) => wireguard::AVAILABLE,
            ConnectionKind::Tor(_) => crate::tor::AVAILABLE,
        }
    }

    /// Whether starting it needs secrets from the vault (a proxy login, WireGuard keys).
    pub fn needs_secrets(&self) -> bool {
        match self {
            ConnectionKind::Direct | ConnectionKind::Tor(_) => false,
            ConnectionKind::Proxy(p) => p.has_auth(),
            ConnectionKind::WireGuard(_) => true,
        }
    }

    /// A short word for lists: "SOCKS5", "WireGuard", "Tor".
    pub fn label(&self) -> &'static str {
        match self {
            ConnectionKind::Direct => "Direct",
            ConnectionKind::Proxy(p) => p.scheme.label(),
            ConnectionKind::WireGuard(_) => "WireGuard",
            ConnectionKind::Tor(_) => "Tor",
        }
    }

    pub fn validate(&self) -> Result<(), crate::NetError> {
        match self {
            ConnectionKind::Direct => Ok(()),
            ConnectionKind::Proxy(p) => p.validate(),
            ConnectionKind::WireGuard(w) => crate::wgconf::validate(w),
            ConnectionKind::Tor(t) => t.validate(),
        }
    }
}

/// A connection the user named. `id` is stable (the vault and the rules refer to it).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Connection {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub kind: ConnectionKind,
}

impl Connection {
    /// A new id (`c-` and 8 hex digits).
    pub fn new_id() -> String {
        format!("c-{}", crate::secret::random_hex(4))
    }
}

/// The secrets of one connection, as stored in the vault.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionSecrets {
    /// Proxy password.
    pub password: Secret,
    pub wireguard: Option<WgSecrets>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_roundtrips_through_toml_without_secrets() {
        let mut wg = WgConfig::default();
        wg.interface.private_key = Secret::new("PRIVATEKEYVALUE");
        wg.interface.addresses.push("10.0.0.2/32".into());
        wg.peers.push(WgPeer {
            public_key: "pub".into(),
            preshared_key: Secret::new("PSKVALUE"),
            endpoint: "vpn.example.net:51820".into(),
            ..Default::default()
        });
        let (stored, secrets) = wg.clone().split_secrets();
        assert!(stored.peers[0].has_preshared_key);
        assert_eq!(secrets.preshared_keys[0].expose(), "PSKVALUE");
        let conns = vec![
            Connection {
                id: "c-1".into(),
                name: "Work".into(),
                kind: ConnectionKind::Proxy(ProxyConfig {
                    scheme: ProxyScheme::Socks5,
                    host: "p.example".into(),
                    port: 1080,
                    username: "bob".into(),
                }),
            },
            Connection { id: "c-2".into(), name: "VPN".into(), kind: ConnectionKind::WireGuard(wg.clone()) },
            Connection {
                id: "c-3".into(),
                name: "Tor".into(),
                kind: ConnectionKind::Tor(TorConfig::default()),
            },
        ];
        #[derive(Serialize, Deserialize)]
        struct W {
            connections: Vec<Connection>,
        }
        let text = toml::to_string(&W { connections: conns.clone() }).unwrap_or_default();
        assert!(!text.contains("PRIVATEKEYVALUE") && !text.contains("PSKVALUE"), "{text}");
        let back: W = toml::from_str(&text).unwrap_or(W { connections: vec![] });
        assert_eq!(back.connections.len(), 3);
        assert_eq!(back.connections[0], conns[0]);
        let restored = back.connections[1].clone();
        let ConnectionKind::WireGuard(w) = restored.kind else { panic!("kind") };
        assert!(w.interface.private_key.is_empty());
        assert_eq!(w.with_secrets(&secrets).peers[0].preshared_key.expose(), "PSKVALUE");
    }

    #[test]
    fn proxy_validation() {
        let ok = ProxyConfig { host: "p.example".into(), port: 1080, ..Default::default() };
        assert!(ok.validate().is_ok());
        assert!(ProxyConfig { host: "".into(), ..ok.clone() }.validate().is_err());
        assert!(ProxyConfig { port: 0, ..ok.clone() }.validate().is_err());
        assert!(ProxyConfig { host: "socks5://x".into(), ..ok.clone() }.validate().is_err());
        assert!(ProxyConfig { host: "a b".into(), ..ok.clone() }.validate().is_err());
        assert_eq!(ProxyConfig { host: "::1".into(), port: 9, ..Default::default() }.host_port(), "[::1]:9");
    }

    #[test]
    fn availability() {
        assert!(ConnectionKind::Direct.is_available());
        assert!(ConnectionKind::Proxy(ProxyConfig::default()).is_available());
        assert_eq!(
            ConnectionKind::WireGuard(WgConfig::default()).is_available(),
            crate::wireguard::AVAILABLE
        );
    }
}
