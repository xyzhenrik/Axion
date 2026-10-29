//! Which connection a request uses.
//!
//! Precedence, strongest first: a per-tab override, then a per-site rule, then the private
//! default (private tabs only), then the account default.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::connection::Connection;

/// Where traffic goes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Target {
    /// Straight out, no connection.
    #[default]
    Direct,
    /// The connection with this id.
    Connection(String),
}

impl Target {
    pub fn is_direct(&self) -> bool {
        matches!(self, Target::Direct)
    }

    pub fn connection_id(&self) -> Option<&str> {
        match self {
            Target::Direct => None,
            Target::Connection(id) => Some(id),
        }
    }

    /// `direct` or the connection id.
    pub fn as_config_str(&self) -> &str {
        match self {
            Target::Direct => "direct",
            Target::Connection(id) => id,
        }
    }

    pub fn from_config_str(s: &str) -> Self {
        let s = s.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("direct") {
            Target::Direct
        } else {
            Target::Connection(s.to_owned())
        }
    }
}

impl Serialize for Target {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_config_str())
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|s| Target::from_config_str(&s))
    }
}

/// "Everything under this domain goes there": `example.com` also covers `www.example.com`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SiteRule {
    /// A host name; subdomains are included. Lowercase, no scheme or path.
    pub host: String,
    pub connection: Target,
}

/// A host name as rules and PAC scripts use it: lowercase letters, digits, `-`, `_`, dots.
/// `None` for anything else (so nothing a user types can end up inside a script).
pub fn normalize_host(input: &str) -> Option<String> {
    let mut h = input.trim().to_ascii_lowercase();
    if let Some(rest) = h.strip_prefix("*.") {
        h = rest.to_owned();
    }
    let h = h.trim_matches('.');
    let ok = !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        });
    ok.then(|| h.to_owned())
}

/// The host of a URL, lowercase, without port and credentials (`None` for URLs without one).
pub fn host_of_url(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        authority.split(':').next()?
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Whether `host` is `suffix` or one of its subdomains.
pub fn host_matches(host: &str, suffix: &str) -> bool {
    host == suffix
        || (host.len() > suffix.len()
            && host.ends_with(suffix)
            && host.as_bytes().get(host.len() - suffix.len() - 1) == Some(&b'.'))
}

/// All routing settings (the `[network]` section minus the knobs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoutingConfig {
    pub connections: Vec<Connection>,
    /// The account's default. `None`: direct.
    pub default: Target,
    /// Private tabs' default. `None`: same as the account's.
    pub private: Option<Target>,
    pub site_rules: Vec<SiteRule>,
    /// Block instead of falling back to a direct connection when a connection is down.
    pub kill_switch: bool,
}

/// Which request to route.
#[derive(Clone, Copy, Debug, Default)]
pub struct RouteQuery<'a> {
    /// The host being contacted (the page's site for a tab-level question).
    pub host: &'a str,
    pub private: bool,
    pub tab_override: Option<&'a Target>,
}

impl RoutingConfig {
    pub fn connection(&self, id: &str) -> Option<&Connection> {
        self.connections.iter().find(|c| c.id == id)
    }

    /// The default for a kind of tab.
    pub fn default_for(&self, private: bool) -> &Target {
        if private { self.private.as_ref().unwrap_or(&self.default) } else { &self.default }
    }

    /// The most specific site rule for a host (the longest matching suffix).
    pub fn site_rule_for(&self, host: &str) -> Option<&SiteRule> {
        self.site_rules.iter().filter(|r| host_matches(host, &r.host)).max_by_key(|r| r.host.len())
    }

    /// The route for a request.
    pub fn resolve(&self, query: &RouteQuery<'_>) -> Target {
        if let Some(t) = query.tab_override {
            return t.clone();
        }
        let host = query.host.trim().trim_end_matches('.').to_ascii_lowercase();
        if let Some(rule) = self.site_rule_for(&host) {
            return rule.connection.clone();
        }
        self.default_for(query.private).clone()
    }

    /// The valid site rules, most specific first. (A rule equal to the default is kept: it can
    /// override a broader rule.)
    pub fn effective_rules(&self, _private: bool) -> Vec<&SiteRule> {
        let mut rules: Vec<&SiteRule> =
            self.site_rules.iter().filter(|r| normalize_host(&r.host).is_some()).collect();
        rules.sort_by_key(|r| std::cmp::Reverse(r.host.len()));
        rules.dedup_by(|a, b| a.host == b.host);
        rules
    }

    /// Every connection id a kind of tab can end up using.
    pub fn targets_for(&self, private: bool) -> Vec<Target> {
        let mut out = vec![self.default_for(private).clone()];
        for r in self.effective_rules(private) {
            if !out.contains(&r.connection) {
                out.push(r.connection.clone());
            }
        }
        out
    }

    /// Whether every request of a kind of tab goes through a connection (no direct default,
    /// no direct site rule).
    pub fn always_routed(&self, private: bool) -> bool {
        self.targets_for(private).iter().all(|t| !t.is_direct())
    }

    /// Whether any request of any tab can be routed (leak hardening applies).
    pub fn any_routed(&self) -> bool {
        self.targets_for(false).iter().chain(self.targets_for(true).iter()).any(|t| !t.is_direct())
    }

    /// Adds or replaces the rule for `host`; `None` removes it.
    pub fn set_site_rule(&mut self, host: &str, target: Option<Target>) {
        let Some(host) = normalize_host(host) else { return };
        self.site_rules.retain(|r| r.host != host);
        if let Some(connection) = target {
            self.site_rules.push(SiteRule { host, connection });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RoutingConfig {
        RoutingConfig {
            connections: vec![],
            default: Target::Connection("work".into()),
            private: Some(Target::Connection("tor".into())),
            site_rules: vec![
                SiteRule { host: "example.com".into(), connection: Target::Direct },
                SiteRule { host: "shop.example.com".into(), connection: Target::Connection("home".into()) },
            ],
            kill_switch: true,
        }
    }

    fn q<'a>(host: &'a str, private: bool, over: Option<&'a Target>) -> RouteQuery<'a> {
        RouteQuery { host, private, tab_override: over }
    }

    #[test]
    fn precedence_tab_site_private_account() {
        let c = cfg();
        let tor = Target::Connection("tor".into());
        // account default
        assert_eq!(c.resolve(&q("news.org", false, None)), Target::Connection("work".into()));
        // private default
        assert_eq!(c.resolve(&q("news.org", true, None)), tor);
        // site rule beats the private default and the account default
        assert_eq!(c.resolve(&q("www.example.com", true, None)), Target::Direct);
        assert_eq!(c.resolve(&q("www.example.com", false, None)), Target::Direct);
        // the most specific rule wins
        assert_eq!(c.resolve(&q("shop.example.com", false, None)), Target::Connection("home".into()));
        assert_eq!(c.resolve(&q("a.shop.example.com", false, None)), Target::Connection("home".into()));
        // the tab override beats everything
        assert_eq!(c.resolve(&q("shop.example.com", false, Some(&tor))), tor);
        assert_eq!(c.resolve(&q("news.org", false, Some(&Target::Direct))), Target::Direct);
        // `notexample.com` is not a subdomain of `example.com`
        assert_eq!(c.resolve(&q("notexample.com", false, None)), Target::Connection("work".into()));
        // no private default: inherit
        let mut d = cfg();
        d.private = None;
        assert_eq!(d.resolve(&q("news.org", true, None)), Target::Connection("work".into()));
    }

    #[test]
    fn effective_rules_and_flags() {
        let c = cfg();
        let rules = c.effective_rules(false);
        assert_eq!(rules[0].host, "shop.example.com", "most specific first");
        assert!(!c.always_routed(false), "example.com goes direct");
        let all = RoutingConfig { site_rules: vec![], ..cfg() };
        assert!(all.always_routed(false) && all.always_routed(true));
        assert!(!RoutingConfig::default().any_routed());
        assert!(all.any_routed());
    }

    #[test]
    fn hosts() {
        assert_eq!(normalize_host(" *.Example.COM. "), Some("example.com".into()));
        assert_eq!(normalize_host("a b"), None);
        assert_eq!(normalize_host("https://x.com"), None);
        assert_eq!(normalize_host("x\"); evil(); //"), None);
        assert_eq!(host_of_url("https://user:pw@Sub.Example.com:8443/p?q#f"), Some("sub.example.com".into()));
        assert_eq!(host_of_url("http://[::1]:80/"), Some("::1".into()));
        assert_eq!(host_of_url("data:text/html,x"), None);
        assert!(
            host_matches("a.b.com", "b.com")
                && host_matches("b.com", "b.com")
                && !host_matches("ab.com", "b.com")
        );
    }

    #[test]
    fn rule_editing_and_serde() {
        let mut c = RoutingConfig::default();
        c.set_site_rule("Example.com", Some(Target::Connection("x".into())));
        c.set_site_rule("example.com", Some(Target::Direct));
        assert_eq!(c.site_rules.len(), 1);
        c.set_site_rule("bad host", Some(Target::Direct));
        assert_eq!(c.site_rules.len(), 1);
        c.set_site_rule("example.com", None);
        assert!(c.site_rules.is_empty());
        #[derive(Serialize, Deserialize)]
        struct W {
            rules: Vec<SiteRule>,
        }
        let w = W {
            rules: vec![
                SiteRule { host: "a.com".into(), connection: Target::Direct },
                SiteRule { host: "b.com".into(), connection: Target::Connection("c-1".into()) },
            ],
        };
        let text = toml::to_string(&w).unwrap_or_default();
        let back: W = toml::from_str(&text).unwrap_or(W { rules: vec![] });
        assert_eq!(back.rules, w.rules);
    }
}
