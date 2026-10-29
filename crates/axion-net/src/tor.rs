//! Tor backend: **not in this build yet.**
//!
//! Plug-in point. To implement it:
//! 1. Replace [`create`] with a constructor returning a type that implements
//!    [`crate::Tunnel`] (an embedded Tor client, or a managed `tor` process whose control
//!    port stays private to this process).
//! 2. `start()` bootstraps (report progress through `status()` -> `Connecting`), then returns
//!    `LocalServer::socks5(credentials, dialer, options)` with a [`crate::Dialer`] that opens
//!    a stream over a circuit. Names are resolved by the exit, never on this computer.
//!    Apply [`crate::TorConfig`]: bridges, exit country.
//! 3. Set [`AVAILABLE`] to `true`.

use std::sync::Arc;

use crate::connection::TorConfig;
use crate::{NetError, Tunnel};

/// Whether this build can run Tor connections.
pub const AVAILABLE: bool = false;

/// Builds the tunnel for a config.
pub fn create(_config: &TorConfig, _options: &crate::TunnelOptions) -> Result<Arc<dyn Tunnel>, NetError> {
    Err(NetError::NotAvailable("Tor"))
}
