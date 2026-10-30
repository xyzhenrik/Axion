//! Where connection secrets are kept. The browser implements [`SecretStore`] on top of the
//! password vault; this crate only knows the interface.

use crate::connection::ConnectionSecrets;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// The vault exists but is locked (or not set up): the connection cannot start.
    #[error("the password vault is locked")]
    Locked,
    #[error("{0}")]
    Unavailable(String),
}

/// Keeps the secrets of connections by connection id.
pub trait SecretStore: Send + Sync {
    /// The secrets of a connection (empty ones if it has none stored).
    fn load(&self, id: &str) -> Result<ConnectionSecrets, SecretError>;
    fn save(&self, id: &str, secrets: &ConnectionSecrets) -> Result<(), SecretError>;
    fn delete(&self, id: &str) -> Result<(), SecretError>;
}

/// A store that keeps nothing (connections without secrets only).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoSecrets;

impl SecretStore for NoSecrets {
    fn load(&self, _id: &str) -> Result<ConnectionSecrets, SecretError> {
        Ok(ConnectionSecrets::default())
    }

    fn save(&self, _id: &str, _secrets: &ConnectionSecrets) -> Result<(), SecretError> {
        Err(SecretError::Unavailable("no secret store".into()))
    }

    fn delete(&self, _id: &str) -> Result<(), SecretError> {
        Ok(())
    }
}
