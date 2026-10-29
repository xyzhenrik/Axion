//! Axion's privacy network: connections (upstream proxy, WireGuard, Tor), the rules that
//! route tabs and sites through them, and the leak-proof wiring the browser engine gets.
//!
//! No UI, rendering or browser-engine dependency: the app feeds it a [`RoutingConfig`] and
//! a [`SecretStore`], and asks [`NetManager::plan`] what the engine's proxy settings are.
//! See `docs/network.md`.
//!
//! - [`connection`]: what a connection is (saved without secrets).
//! - [`wgconf`]: the WireGuard `.conf` parser.
//! - [`route`] / [`pac`]: precedence of rules and the per-site PAC script.
//! - [`Tunnel`]: the trait WireGuard ([`wireguard`]) and Tor ([`tor`]) implement.
//! - [`socks5`] / [`server`] / [`conn`]: the minimal SOCKS5 and HTTP loopback servers.
//! - [`proxy`]: the upstream-proxy backend.

pub mod conn;
pub mod connection;
pub mod manager;
pub mod pac;
pub mod proxy;
pub mod route;
pub mod secret;
pub mod secrets;
pub mod server;
pub mod socks5;
pub mod testing;
pub mod tor;
pub mod tunnel;
pub mod wgconf;
pub mod wireguard;

pub use conn::{Address, Conn, Dialer, pipe};
pub use connection::{
    Connection, ConnectionKind, ConnectionSecrets, ProxyConfig, ProxyScheme, TorConfig, WgConfig,
    WgInterface, WgPeer, WgSecrets,
};
pub use manager::{
    ConnectionStatus, ContextPlan, NetEvent, NetManager, NetOptions, Plan, ProxyLogin, ProxySetting,
    TunnelOptions,
};
pub use proxy::ProxyTimings;
pub use route::{RouteQuery, RoutingConfig, SiteRule, Target, host_matches, host_of_url, normalize_host};
pub use secret::{Credentials, Secret};
pub use secrets::{NoSecrets, SecretError, SecretStore};
pub use server::{LocalServer, ServerOptions};
pub use tunnel::{LocalEndpoint, Tunnel, TunnelStatus};

/// Everything that can go wrong.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    /// The user's input does not make a valid connection.
    #[error("{0}")]
    Invalid(String),
    /// A backend that this build does not contain.
    #[error("{0} connections are not available in this build yet")]
    NotAvailable(&'static str),
    #[error("{0}")]
    Connect(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
