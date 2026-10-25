//! Which saved logins belong to a page: same registrable domain (example.com covers
//! www.example.com and login.example.com, but not example.org or evil-example.com).

use url::Url;

/// The URL's host without "www.", or the input itself when it isn't an URL.
pub fn display_host(site: &str) -> String {
    host(site).unwrap_or_else(|| site.trim().to_owned())
}

fn host(site: &str) -> Option<String> {
    let site = site.trim();
    let parsed = Url::parse(site).or_else(|_| Url::parse(&format!("https://{site}"))).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some(host.strip_prefix("www.").map(str::to_owned).unwrap_or(host))
}

/// The registrable domain of a site (the host itself for IPs and unknown suffixes).
pub fn registrable(site: &str) -> Option<String> {
    let host = host(site)?;
    if host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
        return Some(host);
    }
    let root = addr::parse_domain_name(&host).ok().and_then(|d| d.root().map(str::to_owned));
    Some(root.unwrap_or(host))
}

/// What a new login saved from `page_url` records as its site ("https://login.example.com").
pub fn origin(page_url: &str) -> String {
    Url::parse(page_url).map_or_else(|_| page_url.trim().to_owned(), |u| u.origin().ascii_serialization())
}

/// Whether a login saved for `saved` may be filled into a page at `page_url`.
pub fn same_site(saved: &str, page_url: &str) -> bool {
    matches!((registrable(saved), registrable(page_url)), (Some(a), Some(b)) if a == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching() {
        assert!(same_site("example.com", "https://login.example.com/signin"));
        assert!(same_site("https://www.example.co.uk", "https://shop.example.co.uk/"));
        assert!(!same_site("example.com", "https://example.org/"));
        assert!(!same_site("example.com", "https://evil-example.com/"));
        assert!(!same_site("example.com", "https://example.com.evil.net/"));
        assert!(same_site("http://127.0.0.1:8765/a", "http://127.0.0.1:8765/b"));
        assert_eq!(display_host("https://www.Example.com/x"), "example.com");
    }
}
