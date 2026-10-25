//! What the address field turns into: a URL to load, or a search.

/// Turns address-field input into a URL. Anything that doesn't look like an address becomes a
/// search with `search_url` (where `{}` is replaced by the encoded query).
pub fn resolve_input(input: &str, search_url: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if has_scheme(input) {
        return Some(input.to_owned());
    }
    if is_address(input) {
        let host = input.split(['/', '?', '#']).next().unwrap_or(input);
        let local = host.starts_with("localhost") || host.starts_with("127.") || host.starts_with("[::1]");
        let scheme = if local { "http" } else { "https" };
        return Some(format!("{scheme}://{input}"));
    }
    Some(search_url.replace("{}", &encode_query(input)))
}

fn has_scheme(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    lower.contains("://")
        || ["about:", "data:", "file:", "chrome:", "view-source:", "mailto:"]
            .iter()
            .any(|s| lower.starts_with(s))
}

/// Whether address-field input is meant as an address (rather than a search).
pub fn is_address(input: &str) -> bool {
    let input = input.trim();
    has_scheme(input) || (!input.contains(char::is_whitespace) && looks_like_host(input))
}

fn looks_like_host(input: &str) -> bool {
    let host = input.split(['/', '?', '#']).next().unwrap_or(input);
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    if host.starts_with('[') {
        return host.contains(']');
    }
    let name = host.split(':').next().unwrap_or(host);
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let labels: Vec<&str> = name.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_alphanumeric() || c == '-'))
        // A TLD of letters (example.com) or a dotted IPv4 address.
        && (labels.last().is_some_and(|tld| tld.chars().all(char::is_alphabetic))
            || (labels.len() == 4 && labels.iter().all(|l| l.parse::<u8>().is_ok())))
}

/// Percent-encodes a search query (spaces become `+`).
fn encode_query(query: &str) -> String {
    let mut out = String::with_capacity(query.len() * 3);
    for byte in query.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The short form shown in the address bar when it isn't being edited: the host without `www.`.
pub fn display_host(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return if url == "about:blank" { String::new() } else { url.to_owned() };
    };
    if scheme == "file" {
        return rest.rsplit('/').next().unwrap_or(rest).to_owned();
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    host.strip_prefix("www.").unwrap_or(host).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = "https://search.example/?q={}";

    #[test]
    fn addresses() {
        let r = |s| resolve_input(s, SEARCH).unwrap();
        assert_eq!(r("example.com"), "https://example.com");
        assert_eq!(r("  en.wikipedia.org/wiki/Glass "), "https://en.wikipedia.org/wiki/Glass");
        assert_eq!(r("localhost:8080/app"), "http://localhost:8080/app");
        assert_eq!(r("192.168.1.1"), "https://192.168.1.1");
        assert_eq!(r("http://example.com"), "http://example.com");
        assert_eq!(r("about:blank"), "about:blank");
    }

    #[test]
    fn searches() {
        let r = |s| resolve_input(s, SEARCH).unwrap();
        assert_eq!(r("liquid glass"), "https://search.example/?q=liquid+glass");
        assert_eq!(r("rust"), "https://search.example/?q=rust");
        assert_eq!(r("c++ & c#"), "https://search.example/?q=c%2B%2B+%26+c%23");
        assert_eq!(r("version 1.2"), "https://search.example/?q=version+1.2");
        assert_eq!(r("äpfel"), "https://search.example/?q=%C3%A4pfel");
        assert!(resolve_input("   ", SEARCH).is_none());
    }

    #[test]
    fn hosts() {
        assert_eq!(display_host("https://www.tidewater.travel/notes?id=3"), "tidewater.travel");
        assert_eq!(display_host("http://user@localhost:3000/"), "localhost:3000");
        assert_eq!(display_host("about:blank"), "");
    }
}
