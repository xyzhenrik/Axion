//! Proxy routing and leak protection, as the engine takes it (the browser's `axion-net`
//! decides; this crate only applies it, so it does not depend on that one).

/// What a browser context sends its traffic through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProxySetting {
    /// Chromium's default (the system's proxy settings, normally none).
    Direct,
    /// `socks5://host:port`, `http://host:port` or `https://host:port`. Names are always
    /// sent to the proxy to resolve; there is no fallback to a direct connection.
    Fixed(String),
    /// A PAC script. The proxy list it answers with never ends in `DIRECT` for a routed
    /// site, and the script is mandatory: if it cannot be used, requests fail.
    Pac(String),
}

/// A login Chromium gets when a proxy (HTTP or HTTPS) asks for one.
#[derive(Clone, PartialEq, Eq)]
pub struct ProxyLogin {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for ProxyLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProxyLogin({}:{}, <redacted>)", self.host, self.port)
    }
}

/// Errors Chromium reports when the proxy it was given cannot be used (the kill switch's
/// sink refuses, the proxy is down, a mandatory PAC failed, …).
pub fn is_proxy_error(code: i32) -> bool {
    matches!(
        code,
        -111 // ERR_TUNNEL_CONNECTION_FAILED
        | -115 // ERR_PROXY_AUTH_UNSUPPORTED
        | -120 // ERR_SOCKS_CONNECTION_FAILED
        | -121 // ERR_SOCKS_CONNECTION_HOST_UNREACHABLE
        | -130 // ERR_PROXY_CONNECTION_FAILED
        | -131 // ERR_MANDATORY_PROXY_CONFIGURATION_FAILED
        | -336 // ERR_NO_SUPPORTED_PROXIES
    )
}

/// The value of Chromium's `proxy` preference (JSON).
pub fn proxy_pref_json(setting: &ProxySetting) -> String {
    let value = match setting {
        // `system` is Chromium's default mode; resetting to it undoes an earlier route.
        ProxySetting::Direct => serde_json::json!({ "mode": "system" }),
        ProxySetting::Fixed(server) => serde_json::json!({
            "mode": "fixed_servers",
            "server": server,
            // Chromium's implicit rule keeps loopback and link-local addresses direct;
            // nothing else is exempt.
            "bypass_list": "",
        }),
        ProxySetting::Pac(script) => serde_json::json!({
            "mode": "pac_script",
            "pac_url": pac_data_url(script),
            "pac_mandatory": true,
        }),
    };
    value.to_string()
}

fn pac_data_url(script: &str) -> String {
    use base64::Engine as _;
    format!(
        "data:application/x-ns-proxy-autoconfig;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(script)
    )
}

/// The profile preferences set while a context's traffic is routed (name, JSON value). They
/// stop the browser from contacting anything outside the route on its own.
pub const LEAK_PROTECTION_PREFS: &[(&str, &str)] = &[
    // WebRTC: no UDP outside the proxy, no host candidates revealing local addresses.
    ("webrtc.ip_handling_policy", "\"disable_non_proxied_udp\""),
    ("webrtc.multiple_routes_enabled", "false"),
    ("webrtc.nonproxied_udp_enabled", "false"),
    // No DNS prefetching, speculative connections or preloading.
    ("net.network_prediction_options", "2"),
    // Secure DNS would resolve names itself, by a resolver of its own choosing.
    ("dns_over_https.mode", "\"off\""),
    // Navigation-error suggestions and Safe Browsing pings are requests to third parties.
    ("alternate_error_pages.enabled", "false"),
    ("safebrowsing.enabled", "false"),
    ("safebrowsing.scout_reporting_enabled", "false"),
    // Don't tell the old Chromium about the user's browsing.
    ("url_keyed_anonymized_data_collection.enabled", "false"),
];

/// What the prefs are set back to when protection is switched off (Chromium's defaults).
pub const LEAK_PROTECTION_DEFAULTS: &[(&str, &str)] = &[
    ("webrtc.ip_handling_policy", "\"default\""),
    ("webrtc.multiple_routes_enabled", "true"),
    ("webrtc.nonproxied_udp_enabled", "true"),
    ("net.network_prediction_options", "1"),
    ("dns_over_https.mode", "\"automatic\""),
    ("alternate_error_pages.enabled", "true"),
    ("safebrowsing.enabled", "true"),
    ("safebrowsing.scout_reporting_enabled", "true"),
    ("url_keyed_anonymized_data_collection.enabled", "true"),
];

/// Chromium switches that keep background services from phoning home outside the route.
/// Global (a switch applies to the whole process), so they are used from the start when
/// `network.leak_protection` is on and a route exists at start.
pub fn hardening_switches(
    disable_quic: bool,
    host_resolver_rules: Option<&str>,
) -> Vec<(&'static str, Option<String>)> {
    let mut out: Vec<(&'static str, Option<String>)> = vec![
        ("disable-background-networking", None),
        ("disable-component-update", None),
        ("disable-sync", None),
        ("disable-domain-reliability", None),
        ("disable-client-side-phishing-detection", None),
        ("no-pings", None),
        ("disable-field-trial-config", None),
    ];
    if disable_quic {
        out.push(("disable-quic", None));
    }
    if let Some(rules) = host_resolver_rules {
        out.push(("host-resolver-rules", Some(rules.to_owned())));
    }
    out
}

/// `MAP * ~NOTFOUND, EXCLUDE a, EXCLUDE b`: no name resolves on this computer except these
/// (the proxies' own names and loopback).
pub fn strict_dns_rules(allowed_hosts: &[String]) -> String {
    let mut rules = String::from("MAP * ~NOTFOUND, EXCLUDE localhost");
    for host in allowed_hosts {
        if !host.is_empty() && !host.contains([',', ' ', '"', '\'']) {
            rules.push_str(", EXCLUDE ");
            rules.push_str(host);
        }
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_pref_shapes() {
        let v: serde_json::Value =
            serde_json::from_str(&proxy_pref_json(&ProxySetting::Fixed("socks5://127.0.0.1:1".into())))
                .unwrap();
        assert_eq!(v["mode"], "fixed_servers");
        assert_eq!(v["server"], "socks5://127.0.0.1:1");
        let v: serde_json::Value =
            serde_json::from_str(&proxy_pref_json(&ProxySetting::Pac("function f(){}".into()))).unwrap();
        assert_eq!(v["mode"], "pac_script");
        assert_eq!(v["pac_mandatory"], true);
        assert!(v["pac_url"].as_str().unwrap().starts_with("data:application/x-ns-proxy-autoconfig;base64,"));
        let v: serde_json::Value = serde_json::from_str(&proxy_pref_json(&ProxySetting::Direct)).unwrap();
        assert_eq!(v["mode"], "system");
    }

    #[test]
    fn prefs_are_valid_json_and_paired() {
        for (a, b) in LEAK_PROTECTION_PREFS.iter().chain(LEAK_PROTECTION_DEFAULTS) {
            assert!(serde_json::from_str::<serde_json::Value>(b).is_ok(), "{a}");
        }
        fn names<'a>(l: &[(&'a str, &str)]) -> Vec<&'a str> {
            l.iter().map(|(n, _)| *n).collect()
        }
        assert_eq!(names(LEAK_PROTECTION_PREFS), names(LEAK_PROTECTION_DEFAULTS));
    }

    #[test]
    fn dns_rules_and_switches() {
        let rules = strict_dns_rules(&["proxy.example".into(), "bad host".into()]);
        assert_eq!(rules, "MAP * ~NOTFOUND, EXCLUDE localhost, EXCLUDE proxy.example");
        let s = hardening_switches(true, Some(&rules));
        assert!(s.iter().any(|(n, _)| *n == "disable-quic"));
        assert!(s.iter().any(|(n, v)| *n == "host-resolver-rules" && v.as_deref() == Some(rules.as_str())));
        assert!(!hardening_switches(false, None).iter().any(|(n, _)| *n == "disable-quic"));
    }

    #[test]
    fn proxy_errors() {
        assert!(is_proxy_error(-130) && is_proxy_error(-120) && is_proxy_error(-111));
        assert!(!is_proxy_error(-105), "a name that does not resolve is not a proxy error");
    }

    #[test]
    fn login_debug_hides_the_password() {
        let l = ProxyLogin { host: "h".into(), port: 1, username: "u".into(), password: "secretpw".into() };
        assert!(!format!("{l:?}").contains("secretpw"));
    }
}
