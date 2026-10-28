//! The page side of translation: the script (`page.js`) and its protocol.
//!
//! Page → browser: a console message `"\u{1}axion:tr:" + JSON` ([`parse`]). Anything a page
//! could say this way is harmless: it can claim a language or ask for its own text to be
//! translated.
//! Browser → page: JavaScript from [`start_script`], [`apply_script`], [`fail_script`] and
//! [`stop_script`].

use axion_config::Translation;
use serde::Deserialize;

/// Console messages starting with this come from the page script.
pub const MARKER: &str = "\u{1}axion:tr:";

const SOURCE: &str = include_str!("page.js");

/// The script injected into every top-level document.
pub fn page_script(config: &Translation) -> String {
    SOURCE.replace("__AXION_DETECT_MIN__", &config.detect_min_chars.to_string())
}

/// Starts translating the page into `target` (a language code).
pub fn start_script(config: &Translation, target: &str) -> String {
    let options = serde_json::json!({
        "chars": config.batch_chars.max(100),
        "pieces": config.batch_pieces.max(1),
        "observe": config.translate_changes,
        "target": target,
    });
    format!("window.__axionTr && window.__axionTr.start({options});")
}

/// Puts the translations of batch `id` into the page.
pub fn apply_script(id: u32, texts: &[String]) -> String {
    let texts = serde_json::to_string(texts).unwrap_or_else(|_| "[]".into());
    format!("window.__axionTr && window.__axionTr.apply({id}, {texts});")
}

/// Tells the page that batch `id` could not be translated.
pub fn fail_script(id: u32) -> String {
    format!("window.__axionTr && window.__axionTr.fail({id});")
}

/// Puts the original text back.
pub fn stop_script() -> &'static str {
    "window.__axionTr && window.__axionTr.stop();"
}

/// Asks the page to report its language again.
pub fn report_script() -> &'static str {
    "window.__axionTr && window.__axionTr.report();"
}

/// What the page tells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// The page's language code as declared (`declared`) or guessed; empty when unknown.
    Lang {
        lang: String,
        declared: bool,
    },
    /// Text to translate.
    Batch {
        id: u32,
        texts: Vec<String>,
    },
    Progress {
        done: u32,
        total: u32,
    },
    State {
        active: bool,
    },
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum Wire {
    Lang {
        #[serde(default)]
        lang: String,
        #[serde(default)]
        declared: bool,
    },
    Batch {
        id: u32,
        texts: Vec<String>,
    },
    Progress {
        #[serde(default)]
        done: u32,
        #[serde(default)]
        total: u32,
    },
    State {
        #[serde(default)]
        active: bool,
    },
}

/// Parses a console message (`None` for other or malformed ones).
pub fn parse(console_message: &str) -> Option<Message> {
    let json = console_message.strip_prefix(MARKER)?;
    Some(match serde_json::from_str::<Wire>(json).ok()? {
        Wire::Lang { lang, declared } => Message::Lang { lang, declared },
        Wire::Batch { id, texts } => Message::Batch { id, texts },
        Wire::Progress { done, total } => Message::Progress { done, total },
        Wire::State { active } => Message::State { active },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page_messages() {
        let m = format!(r#"{MARKER}{{"t":"lang","lang":"de-AT","declared":true}}"#);
        assert_eq!(parse(&m), Some(Message::Lang { lang: "de-AT".into(), declared: true }));
        let m = format!(r#"{MARKER}{{"t":"batch","id":3,"texts":["a","b"]}}"#);
        assert_eq!(parse(&m), Some(Message::Batch { id: 3, texts: vec!["a".into(), "b".into()] }));
        assert_eq!(parse("\u{1}axion:reader:{}"), None);
        assert_eq!(parse(&format!("{MARKER}[]")), None);
    }

    #[test]
    fn scripts_carry_json() {
        let script = apply_script(5, &["He said \"hi\"".into(), "a\nb".into()]);
        assert!(script.contains(r#"["He said \"hi\"","a\nb"]"#));
        let start = start_script(&Translation::default(), "en");
        assert!(start.contains(r#""target":"en""#) && start.contains(r#""chars":4000"#));
        assert!(!page_script(&Translation::default()).contains("__AXION_"));
    }
}
