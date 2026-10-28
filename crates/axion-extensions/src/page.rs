//! The Chrome Web Store's page side: the script (`page.js`) and its protocol.
//!
//! Page → browser: a console message `"\u{1}axion:ws:" + JSON` ([`parse`]). Any page can send
//! one, so the browser acts on it only for a tab that shows the Web Store ([`is_store_url`]),
//! and never installs anything without its own confirmation dialog.
//! Browser → page: [`configure_script`].

use serde::Deserialize;

use crate::webstore::is_extension_id;

/// Console messages starting with this come from the Web Store script.
pub const MARKER: &str = "\u{1}axion:ws:";

const SOURCE: &str = include_str!("page.js");

/// The script injected into every top-level document (it does nothing outside the Web Store).
/// `installed` are the ids of the installed extensions, `add` / `remove` the labels of the
/// script's own button (used only when the store page has no install API of its own).
pub fn page_script(installed: &[String], add: &str, remove: &str) -> String {
    let init = serde_json::json!({ "installed": installed, "add": add, "remove": remove });
    SOURCE.replace("__AXION_INIT__", &init.to_string())
}

/// Tells the page which extensions are installed and how its button reads then.
pub fn configure_script(installed: &[String], add: &str, remove: &str) -> String {
    let config = serde_json::json!({ "installed": installed, "add": add, "remove": remove });
    format!("window.__axionStore && window.__axionStore.configure({config});")
}

/// What the page tells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// The user asked to install extension `id`.
    Install(String),
    /// The user asked to remove the installed extension `id`.
    Remove(String),
    /// A store page was (re)loaded; `id` is the extension it shows, if any.
    Ready,
    /// What the script found (debugging aid).
    Probe(String),
}

#[derive(Deserialize)]
struct Wire {
    e: String,
    #[serde(default)]
    id: String,
}

/// Reads a console message; `None` if it is not (valid) Web Store script output.
pub fn parse(message: &str) -> Option<Message> {
    let json = message.strip_prefix(MARKER)?;
    let wire: Wire = serde_json::from_str(json).ok()?;
    Some(match wire.e.as_str() {
        "install" if is_extension_id(&wire.id) => Message::Install(wire.id),
        "remove" if is_extension_id(&wire.id) => Message::Remove(wire.id),
        "ready" => Message::Ready,
        "probe" => Message::Probe(json.chars().take(500).collect()),
        _ => return None,
    })
}

/// Whether `url` is a page of the Chrome Web Store.
pub fn is_store_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.split([':', '?', '#']).next().unwrap_or(host).to_ascii_lowercase();
    host == "chromewebstore.google.com"
        || ((host == "chrome.google.com" || host == "www.chrome.google.com") && path.starts_with("webstore"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "dhdgffkkebhmkfjojejmpbldmpobfkfo";

    fn say(json: &str) -> String {
        format!("{MARKER}{json}")
    }

    #[test]
    fn messages() {
        assert_eq!(
            parse(&say(&format!(r#"{{"e":"install","id":"{ID}"}}"#))),
            Some(Message::Install(ID.into()))
        );
        assert_eq!(
            parse(&say(&format!(r#"{{"e":"remove","id":"{ID}"}}"#))),
            Some(Message::Remove(ID.into()))
        );
        assert_eq!(parse(&say(r#"{"e":"ready","id":""}"#)), Some(Message::Ready));
        assert!(matches!(parse(&say(r#"{"e":"probe","buttons":[]}"#)), Some(Message::Probe(_))));
    }

    #[test]
    fn bad_input_is_ignored() {
        assert_eq!(parse(&say(r#"{"e":"install","id":"../../etc"}"#)), None);
        assert_eq!(parse(&say(r#"{"e":"install"}"#)), None);
        assert_eq!(parse(&say(r#"{"e":"launch","id":"x"}"#)), None);
        assert_eq!(parse(&say("not json")), None);
        assert_eq!(parse(r#"{"e":"ready"}"#), None);
    }

    #[test]
    fn store_urls() {
        assert!(is_store_url("https://chromewebstore.google.com/detail/x/abc"));
        assert!(is_store_url("https://chrome.google.com/webstore/detail/x/abc"));
        assert!(is_store_url("https://CHROMEWEBSTORE.google.com:443/"));
        assert!(!is_store_url("http://chromewebstore.google.com/"));
        assert!(!is_store_url("https://chrome.google.com/intl/en/chrome/"));
        assert!(!is_store_url("https://example.com/chromewebstore.google.com"));
        assert!(!is_store_url("https://notchromewebstore.google.com/"));
    }

    #[test]
    fn configure_is_valid_json_for_awkward_text() {
        let script = configure_script(&[ID.into()], "Add \"x\"", "Remove </script>");
        assert!(script.starts_with("window.__axionStore"));
        assert!(script.contains(r#"Add \"x\""#));
    }

    #[test]
    fn the_script_is_self_contained() {
        let script = page_script(&[ID.into()], "Add", "Remove");
        assert!(script.contains("beginInstallWithManifest3"));
        assert!(script.contains(&MARKER.replace('\u{1}', "\\u0001")));
        assert!(!script.contains("__AXION_INIT__"));
        assert!(script.contains(&format!(r#"{{"add":"Add","installed":["{ID}"],"remove":"Remove"}}"#)));
    }
}
