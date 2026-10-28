//! Chrome match patterns (`host_permissions`, content script `matches`).

/// Whether a host permission / match pattern (`https://*.example.com/*`, `*://*/*`,
/// `<all_urls>`) covers the page `url`. Only http, https and file pages can match, like in
/// Chrome; `*` as the scheme means http or https.
pub fn host_matches(pattern: &str, url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else { return false };
    if !matches!(scheme, "http" | "https" | "file") {
        return false;
    }
    if pattern == "<all_urls>" {
        return true;
    }
    let Some((p_scheme, p_rest)) = pattern.split_once("://") else { return false };
    if p_scheme != "*" && p_scheme != scheme || p_scheme == "*" && scheme == "file" {
        return false;
    }
    let (p_host, p_path) = p_rest.split_once('/').unwrap_or((p_rest, ""));
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    // Without credentials and port.
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    let p_host = p_host.to_ascii_lowercase();
    let host_ok = if scheme == "file" || p_host == "*" {
        true
    } else if let Some(suffix) = p_host.strip_prefix("*.") {
        host == suffix || host.ends_with(&format!(".{suffix}"))
    } else {
        host == p_host.split(':').next().unwrap_or(&p_host)
    };
    // The query and fragment are part of the path in Chrome's patterns; the pattern's `*`
    // covers them in practice, so they are kept.
    host_ok && glob(p_path, path)
}

/// `*` matches any run of characters.
fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || text.len() < first.len() + last.len() || !text.ends_with(last) {
        return false;
    }
    let mut at = first.len();
    let end = text.len() - last.len();
    for part in &parts[1..parts.len() - 1] {
        match text[at..end].find(part) {
            Some(i) => at += i + part.len(),
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::host_matches;

    #[test]
    fn patterns_cover_pages() {
        assert!(host_matches("<all_urls>", "https://a.com/x"));
        assert!(host_matches("*://*/*", "http://a.com/"));
        assert!(!host_matches("*://*/*", "file:///tmp/a.html"));
        assert!(host_matches("https://*.example.com/*", "https://www.example.com/a?b=1"));
        assert!(host_matches("https://*.example.com/*", "https://example.com/"));
        assert!(!host_matches("https://*.example.com/*", "https://notexample.com/"));
        assert!(!host_matches("https://example.com/*", "http://example.com/"));
        assert!(host_matches("https://example.com/a/*/c", "https://example.com:8080/a/b/c"));
        assert!(!host_matches("https://example.com/a", "https://example.com/ab"));
        assert!(!host_matches("<all_urls>", "axion://home/"));
        assert!(!host_matches("https://*/*", "chrome-extension://abc/popup.html"));
    }
}
