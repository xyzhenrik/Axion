//! Hosts and sites: a *site* is the registrable domain (eTLD+1), so `news.example.co.uk` and
//! `www.example.co.uk` are the same site.

use url::Url;

/// The URL's host without a leading `www.`, lower case. `None` for URLs without a host.
pub fn host(url: &str) -> Option<String> {
    let host = Url::parse(url).ok()?.host_str()?.to_ascii_lowercase();
    Some(host.strip_prefix("www.").map(str::to_owned).unwrap_or(host))
}

/// The registrable domain of the URL's host (the host itself for IPs and unknown suffixes).
pub fn site(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    if parsed.host().is_some_and(|h| !matches!(h, url::Host::Domain(_))) {
        return Some(host);
    }
    let root = addr::parse_domain_name(&host).ok().and_then(|d| d.root().map(str::to_owned));
    Some(root.unwrap_or(host))
}

/// Whether two URLs belong to different sites. Pages without a site (about:blank, data:)
/// count as the same site as anything, so they never trigger cross-site rules.
pub fn is_cross_site(a: &str, b: &str) -> bool {
    match (site(a), site(b)) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_sites() {
        assert_eq!(host("https://www.Example.com/a").as_deref(), Some("example.com"));
        assert_eq!(site("https://news.bbc.co.uk/").as_deref(), Some("bbc.co.uk"));
        assert_eq!(site("http://127.0.0.1:8080/").as_deref(), Some("127.0.0.1"));
        assert_eq!(site("about:blank"), None);
        assert!(is_cross_site("https://a.example.com/", "https://ads.net/x"));
        assert!(!is_cross_site("https://a.example.com/", "https://b.example.com/"));
        assert!(!is_cross_site("about:blank", "https://ads.net/"));
    }
}
