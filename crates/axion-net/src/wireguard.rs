//! WireGuard backend (userspace, no root): **not in this build yet.**
//!
//! Plug-in point. To implement it:
//! 1. Replace [`create`] with a constructor returning a type that implements
//!    [`crate::Tunnel`]. `config` carries the keys (the manager restores them from the vault
//!    with [`crate::WgConfig::with_secrets`] before calling).
//! 2. `start()` runs the handshake and a userspace TCP/IP stack on its own thread(s), then
//!    returns `LocalServer::socks5(credentials, dialer, options)` where `dialer` implements
//!    [`crate::Dialer`] by opening a stream inside the tunnel. Names must be resolved through
//!    the tunnel's DNS servers (`config.interface.dns`), never on this computer.
//! 3. Set [`AVAILABLE`] to `true`.
//!
//! The endpoint host of the peers ([`crate::WgConfig::endpoint_hosts`]) is the one name that
//! is resolved outside the tunnel.

use std::sync::Arc;

use crate::connection::WgConfig;
use crate::{NetError, Tunnel};

/// Whether this build can run WireGuard connections.
pub const AVAILABLE: bool = false;

/// Builds the tunnel for a config.
pub fn create(_config: &WgConfig, _options: &crate::TunnelOptions) -> Result<Arc<dyn Tunnel>, NetError> {
    Err(NetError::NotAvailable("WireGuard"))
}
