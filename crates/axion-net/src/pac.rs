//! Proxy auto-config scripts for per-site routing in one browser context.
//!
//! The script only compares host *names* as strings (no `dnsResolve`, `isInNet`, … which
//! would resolve names on this computer and leak them). Every non-direct answer is a single
//! proxy with no `DIRECT` after it, so Chromium never falls back to a direct connection.

use base64::Engine as _;

use crate::route::normalize_host;

/// What the script answers for a host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PacTarget {
    Direct,
    /// A proxy in Chromium's PAC syntax: `SOCKS5`, `PROXY` (HTTP) or `HTTPS`.
    Proxy {
        kind: PacProxyKind,
        host_port: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacProxyKind {
    Socks5,
    Http,
    Https,
}

impl PacTarget {
    fn answer(&self) -> String {
        match self {
            PacTarget::Direct => "DIRECT".into(),
            PacTarget::Proxy { kind, host_port } => {
                let word = match kind {
                    PacProxyKind::Socks5 => "SOCKS5",
                    PacProxyKind::Http => "PROXY",
                    PacProxyKind::Https => "HTTPS",
                };
                format!("{word} {host_port}")
            }
        }
    }
}

/// Makes a PAC script: the rules (most specific first, as given) and a default.
/// Rules with an invalid host are skipped.
pub fn generate(default: &PacTarget, rules: &[(String, PacTarget)]) -> String {
    let mut js = String::from("function FindProxyForURL(url, host) {\n  host = host.toLowerCase();\n");
    js.push_str("  function under(s) { return host === s || (host.length > s.length && host.substr(host.length - s.length - 1) === '.' + s); }\n");
    for (suffix, target) in rules {
        let Some(suffix) = normalize_host(suffix) else { continue };
        js.push_str(&format!(
            "  if (under({})) return {};\n",
            js_string(&suffix),
            js_string(&target.answer())
        ));
    }
    js.push_str(&format!("  return {};\n}}\n", js_string(&default.answer())));
    js
}

/// A string literal safe to paste into a script.
fn js_string(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '<' => out.push_str("\\x3c"),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            c => out.push_str(&format!("\\u{:04x}", c as u32)),
        }
    }
    out.push('\'');
    out
}

/// The `pac_url` of Chromium's `proxy` preference for a script.
pub fn data_url(script: &str) -> String {
    format!(
        "data:application/x-ns-proxy-autoconfig;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(script)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socks(hp: &str) -> PacTarget {
        PacTarget::Proxy { kind: PacProxyKind::Socks5, host_port: hp.into() }
    }

    #[test]
    fn script_has_rules_then_default() {
        let js = generate(
            &socks("127.0.0.1:9050"),
            &[
                ("shop.example.com".into(), PacTarget::Direct),
                (
                    "example.com".into(),
                    PacTarget::Proxy { kind: PacProxyKind::Http, host_port: "127.0.0.1:8118".into() },
                ),
            ],
        );
        let shop = js.find("'shop.example.com'").expect("shop rule");
        let ex = js.find("'example.com'").expect("example rule");
        assert!(shop < ex, "order kept");
        assert!(js.contains("return 'DIRECT'"));
        assert!(js.contains("return 'PROXY 127.0.0.1:8118'"));
        assert!(js.trim_end().ends_with("return 'SOCKS5 127.0.0.1:9050';\n}"));
        // Only name comparisons: nothing that resolves on this computer.
        for banned in ["dnsResolve", "isInNet", "myIpAddress", "isResolvable"] {
            assert!(!js.contains(banned));
        }
        // No fallback to direct behind a proxy answer.
        assert!(!js.contains("; DIRECT") && !js.contains(";DIRECT"));
    }

    #[test]
    fn hostile_hosts_are_dropped_and_strings_escaped() {
        let js = generate(
            &PacTarget::Direct,
            &[("x'); evil(); ('".into(), PacTarget::Direct), ("ok.com".into(), PacTarget::Direct)],
        );
        assert!(!js.contains("evil"));
        assert!(js.contains("'ok.com'"));
        assert_eq!(js_string("a'b\\c<"), "'a\\'b\\\\c\\x3c'");
    }

    #[test]
    fn matching_semantics_in_rust_mirror_the_script() {
        // The script's `under` is the same test as route::host_matches.
        use crate::route::host_matches;
        assert!(host_matches("www.example.com", "example.com"));
        assert!(!host_matches("badexample.com", "example.com"));
    }

    #[test]
    fn data_url_is_base64() {
        let u = data_url("function FindProxyForURL(){return 'DIRECT';}");
        assert!(u.starts_with("data:application/x-ns-proxy-autoconfig;base64,"));
    }
}
