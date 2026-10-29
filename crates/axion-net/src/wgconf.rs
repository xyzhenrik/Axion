//! Parser and validator for WireGuard `.conf` files (`wg-quick` format).
//!
//! Only what a userspace client needs is read. Lines that would run commands on the machine
//! (`PreUp`, `PostUp`, `PreDown`, `PostDown`, `SaveConfig`) and routing-table options are
//! ignored and reported in [`ParsedWg::ignored`]; Axion never runs a script from a config.

use std::net::IpAddr;

use base64::Engine as _;

use crate::connection::{WgConfig, WgInterface, WgPeer};
use crate::secret::Secret;

/// Largest `.conf` accepted (bytes).
pub const MAX_CONF_BYTES: usize = 64 * 1024;
/// Most peers accepted in one config.
pub const MAX_PEERS: usize = 64;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WgParseError {
    #[error("line {line}: {message}")]
    Line { line: usize, message: String },
    #[error("{0}")]
    Invalid(String),
}

/// A parsed config and what was left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedWg {
    pub config: WgConfig,
    /// Keys that were present but are not used (scripts, routing table, …).
    pub ignored: Vec<String>,
}

fn line_err(line: usize, message: impl Into<String>) -> WgParseError {
    WgParseError::Line { line, message: message.into() }
}

/// Parses a `.conf` and validates it.
pub fn parse(text: &str) -> Result<ParsedWg, WgParseError> {
    if text.len() > MAX_CONF_BYTES {
        return Err(WgParseError::Invalid("the file is too large for a WireGuard config".into()));
    }
    let mut interface = WgInterface::default();
    let mut peers: Vec<WgPeer> = Vec::new();
    let mut ignored = Vec::new();
    #[derive(PartialEq)]
    enum Section {
        None,
        Interface,
        Peer,
    }
    let mut section = Section::None;
    let mut seen_interface = false;
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        // Comments start at `#` (a base64 key never contains one).
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            match name.trim().to_ascii_lowercase().as_str() {
                "interface" => {
                    if seen_interface {
                        return Err(line_err(n, "a second [Interface] section"));
                    }
                    seen_interface = true;
                    section = Section::Interface;
                }
                "peer" => {
                    if peers.len() >= MAX_PEERS {
                        return Err(line_err(n, "too many peers"));
                    }
                    peers.push(WgPeer::default());
                    section = Section::Peer;
                }
                other => return Err(line_err(n, format!("unknown section [{other}]"))),
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(line_err(n, "expected `Key = Value`"));
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        match (&section, key.as_str()) {
            (Section::None, _) => return Err(line_err(n, "a setting before any section")),
            (Section::Interface, "privatekey") => {
                check_key(value).map_err(|e| line_err(n, format!("PrivateKey: {e}")))?;
                interface.private_key = Secret::new(value);
            }
            (Section::Interface, "address") => {
                for item in list(value) {
                    check_cidr(item).map_err(|e| line_err(n, format!("Address: {e}")))?;
                    interface.addresses.push(item.to_owned());
                }
            }
            (Section::Interface, "dns") => {
                for item in list(value) {
                    // IP addresses are resolvers; anything else is a search domain, which the
                    // userspace client does not use.
                    if item.parse::<IpAddr>().is_ok() {
                        interface.dns.push(item.to_owned());
                    } else if valid_domain(item) {
                        ignored.push(format!("DNS search domain {item}"));
                    } else {
                        return Err(line_err(n, format!("DNS: {item:?} is not an address")));
                    }
                }
            }
            (Section::Interface, "mtu") => {
                let mtu: u32 = value.parse().map_err(|_| line_err(n, "MTU is not a number"))?;
                if !(576..=9000).contains(&mtu) {
                    return Err(line_err(n, "MTU must be between 576 and 9000"));
                }
                interface.mtu = mtu as u16;
            }
            (Section::Interface, "listenport") => {
                interface.listen_port = value.parse().map_err(|_| line_err(n, "ListenPort is not a port"))?;
            }
            (
                Section::Interface,
                "preup" | "postup" | "predown" | "postdown" | "saveconfig" | "table" | "fwmark",
            ) => {
                ignored.push(raw_key(raw));
            }
            (Section::Peer, "publickey") => {
                check_key(value).map_err(|e| line_err(n, format!("PublicKey: {e}")))?;
                last(&mut peers).public_key = value.to_owned();
            }
            (Section::Peer, "presharedkey") => {
                check_key(value).map_err(|e| line_err(n, format!("PresharedKey: {e}")))?;
                let peer = last(&mut peers);
                peer.preshared_key = Secret::new(value);
                peer.has_preshared_key = true;
            }
            (Section::Peer, "allowedips") => {
                for item in list(value) {
                    check_cidr(item).map_err(|e| line_err(n, format!("AllowedIPs: {e}")))?;
                    last(&mut peers).allowed_ips.push(item.to_owned());
                }
            }
            (Section::Peer, "endpoint") => {
                check_endpoint(value).map_err(|e| line_err(n, format!("Endpoint: {e}")))?;
                last(&mut peers).endpoint = value.to_owned();
            }
            (Section::Peer, "persistentkeepalive") => {
                last(&mut peers).persistent_keepalive = if value.eq_ignore_ascii_case("off") {
                    0
                } else {
                    value
                        .parse()
                        .map_err(|_| line_err(n, "PersistentKeepalive is not a number of seconds"))?
                };
            }
            (_, other) => return Err(line_err(n, format!("unknown setting {other:?}"))),
        }
    }
    let config = WgConfig { interface, peers };
    validate(&config).map_err(|e| WgParseError::Invalid(e.to_string()))?;
    Ok(ParsedWg { config, ignored })
}

fn raw_key(raw: &str) -> String {
    raw.split('=').next().unwrap_or_default().trim().to_owned()
}

fn last(peers: &mut [WgPeer]) -> &mut WgPeer {
    // Invariant: a peer setting is only parsed after a `[Peer]` header pushed a peer.
    let i = peers.len().saturating_sub(1);
    &mut peers[i]
}

fn list(value: &str) -> impl Iterator<Item = &str> {
    value.split(',').map(str::trim).filter(|s| !s.is_empty())
}

/// 32 bytes of standard base64.
fn check_key(value: &str) -> Result<(), String> {
    let bytes =
        base64::engine::general_purpose::STANDARD.decode(value).map_err(|_| "not valid base64".to_owned())?;
    if bytes.len() != 32 {
        return Err(format!("a key is 32 bytes, this one has {}", bytes.len()));
    }
    Ok(())
}

fn check_cidr(item: &str) -> Result<(), String> {
    let (addr, prefix) = match item.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (item, None),
    };
    let ip: IpAddr = addr.parse().map_err(|_| format!("{item:?} is not an IP address"))?;
    if let Some(p) = prefix {
        let max = if ip.is_ipv4() { 32 } else { 128 };
        let p: u8 = p.parse().map_err(|_| format!("{item:?}: the prefix is not a number"))?;
        if p > max {
            return Err(format!("{item:?}: the prefix is longer than {max}"));
        }
    }
    Ok(())
}

/// `host:port` or `[v6]:port` -> (host, port).
pub fn split_endpoint(endpoint: &str) -> Option<(&str, u16)> {
    let endpoint = endpoint.trim();
    let (host, port) = if let Some(rest) = endpoint.strip_prefix('[') {
        let (host, rest) = rest.split_once(']')?;
        (host, rest.strip_prefix(':')?)
    } else {
        endpoint.rsplit_once(':')?
    };
    let port: u16 = port.parse().ok()?;
    (!host.is_empty() && port != 0).then_some((host, port))
}

fn check_endpoint(value: &str) -> Result<(), String> {
    let (host, _) = split_endpoint(value).ok_or_else(|| format!("{value:?} is not host:port"))?;
    if host.parse::<IpAddr>().is_err() && !valid_domain(host) {
        return Err(format!("{host:?} is not a host name or address"));
    }
    Ok(())
}

fn valid_domain(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

/// Checks a config as a whole (also used for configs that were not parsed from text).
pub fn validate(config: &WgConfig) -> Result<(), crate::NetError> {
    let invalid = |m: &str| Err(crate::NetError::Invalid(m.to_owned()));
    if config.interface.addresses.is_empty() {
        return invalid("the [Interface] has no Address");
    }
    if config.peers.is_empty() {
        return invalid("the config has no [Peer]");
    }
    // The key may be absent when the config was loaded from `config.toml` (it is in the vault).
    if !config.interface.private_key.is_empty() {
        check_key(config.interface.private_key.expose())
            .map_err(|e| crate::NetError::Invalid(format!("PrivateKey: {e}")))?;
    }
    for peer in &config.peers {
        if peer.public_key.is_empty() {
            return invalid("a [Peer] has no PublicKey");
        }
        if peer.allowed_ips.is_empty() {
            return invalid("a [Peer] has no AllowedIPs");
        }
    }
    if config.peers.iter().all(|p| p.endpoint.is_empty()) {
        return invalid("no [Peer] has an Endpoint to connect to");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
    const KEY_B: &str = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=";

    fn sample() -> String {
        format!(
            "# comment\n[Interface]\nPrivateKey = {KEY_A}\nAddress = 10.0.0.2/32, fd00::2/128\nDNS = 1.1.1.1, 9.9.9.9, corp.example\nMTU = 1380\nPostUp = iptables -A FORWARD -j ACCEPT\n\n[Peer]\nPublicKey = {KEY_B}\nPresharedKey = {KEY_B}\nAllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = vpn.example.net:51820\nPersistentKeepalive = 25 # keep NAT open\n"
        )
    }

    #[test]
    fn parses_a_typical_config() {
        let parsed = parse(&sample()).expect("parses");
        let c = parsed.config;
        assert_eq!(c.interface.private_key.expose(), KEY_A);
        assert_eq!(c.interface.addresses, ["10.0.0.2/32", "fd00::2/128"]);
        assert_eq!(c.interface.dns, ["1.1.1.1", "9.9.9.9"]);
        assert_eq!(c.interface.mtu, 1380);
        assert_eq!(c.peers.len(), 1);
        assert_eq!(c.peers[0].allowed_ips, ["0.0.0.0/0", "::/0"]);
        assert_eq!(c.peers[0].endpoint, "vpn.example.net:51820");
        assert_eq!(c.peers[0].persistent_keepalive, 25);
        assert!(c.peers[0].has_preshared_key);
        assert!(parsed.ignored.iter().any(|i| i == "PostUp"), "{:?}", parsed.ignored);
        assert!(parsed.ignored.iter().any(|i| i.contains("corp.example")));
    }

    #[test]
    fn rejects_bad_input() {
        let bad_key = sample().replace(KEY_A, "AAAA");
        assert!(matches!(parse(&bad_key), Err(WgParseError::Line { line: 3, .. })));
        assert!(parse(&sample().replace("10.0.0.2/32", "10.0.0.2/40")).is_err());
        assert!(parse(&sample().replace("vpn.example.net:51820", "vpn.example.net")).is_err());
        assert!(parse(&sample().replace("MTU = 1380", "MTU = 12")).is_err());
        assert!(parse("[Interface]\nBogus = 1\n").is_err());
        assert!(parse("PrivateKey = x").is_err());
        assert!(
            parse(&format!("[Interface]\nPrivateKey = {KEY_A}\nAddress = 10.0.0.2/32\n")).is_err(),
            "no peer"
        );
        let no_endpoint = sample().replace("Endpoint = vpn.example.net:51820\n", "");
        assert!(matches!(parse(&no_endpoint), Err(WgParseError::Invalid(_))));
        let two = format!("{}\n[Interface]\n", sample());
        assert!(parse(&two).is_err());
        assert!(parse(&"x".repeat(MAX_CONF_BYTES + 1)).is_err());
    }

    #[test]
    fn endpoints() {
        assert_eq!(split_endpoint("1.2.3.4:51820"), Some(("1.2.3.4", 51820)));
        assert_eq!(split_endpoint("[2001:db8::1]:443"), Some(("2001:db8::1", 443)));
        assert_eq!(split_endpoint("host"), None);
        assert_eq!(split_endpoint("host:0"), None);
        assert_eq!(split_endpoint("[::1]443"), None);
    }

    #[test]
    fn secrets_split_and_restore() {
        let parsed = parse(&sample()).expect("parses");
        let (stored, secrets) = parsed.config.clone().split_secrets();
        assert!(stored.interface.private_key.is_empty());
        // The stored form is valid without keys (loaded from config.toml).
        assert!(validate(&stored).is_ok());
        assert_eq!(stored.with_secrets(&secrets), parsed.config);
    }
}
