//! Secrets (proxy passwords, WireGuard keys) never print and are wiped when dropped.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroize;

/// A string that must not leak into logs, `Debug` output or config files.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret itself. Only hand it to the code that needs it (a handshake, the vault).
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() { "Secret(empty)" } else { "Secret(<redacted>)" })
    }
}

/// A `Secret` field is normally `#[serde(skip)]`; if one is serialized anyway it becomes an
/// empty string, never the value.
impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("")
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Secret)
    }
}

/// A user name and password for a proxy or a local endpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: Secret,
}

impl Credentials {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self { username: username.into(), password: Secret::new(password) }
    }

    /// Fresh random credentials for a local endpoint (valid for one session).
    pub fn random_session() -> Self {
        Self::new(format!("axion-{}", random_hex(4)), random_hex(16))
    }

    /// Compares in constant time (no early exit on the first differing byte).
    pub fn matches(&self, username: &str, password: &str) -> bool {
        let a = ct_eq(self.username.as_bytes(), username.as_bytes());
        let b = ct_eq(self.password.expose().as_bytes(), password.as_bytes());
        a & b
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credentials({:?}, <redacted>)", self.username)
    }
}

/// `len` random bytes as lowercase hex. If the system has no randomness at all (practically
/// never) the value is time-seeded; the endpoints it protects are loopback-only anyway.
pub fn random_hex(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (nanos >> ((i % 16) * 8)) as u8 ^ (i as u8).wrapping_mul(131);
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u32;
    for i in 0..a.len().max(b.len()) {
        diff |= u32::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_serialize_never_show_the_value() {
        let s = Secret::new("hunter2");
        assert!(!format!("{s:?}").contains("hunter2"));
        let c = Credentials::new("me", "hunter2");
        assert!(!format!("{c:?}").contains("hunter2"));
        #[derive(Serialize)]
        struct W {
            s: Secret,
        }
        let t = toml::to_string(&W { s }).unwrap_or_default();
        assert!(!t.contains("hunter2"));
    }

    #[test]
    fn credentials_compare() {
        let c = Credentials::new("a", "b");
        assert!(c.matches("a", "b"));
        assert!(!c.matches("a", "c"));
        assert!(!c.matches("a", "bb"));
        assert_ne!(Credentials::random_session(), Credentials::random_session());
    }
}
