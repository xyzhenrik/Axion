//! Reader view: a page's article on a clean sheet (font, size, theme and column width are
//! the reader's choice). The extraction runs in the page (see `reader.js`, our own compact
//! implementation of the text-density approach; no third-party code is bundled), which keeps
//! the original document aside and swaps it back on exit.
//!
//! Page → browser: a console message `"\u{1}axion:reader:" + JSON` ([`parse`]).
//! Browser → page: JavaScript from [`enter_script`], [`apply_script`], [`exit_script`].

use axion_config::{Reader, ReaderFont, ReaderTheme};
use serde::{Deserialize, Serialize};

/// Console messages starting with this come from the Reader script.
pub const MARKER: &str = "\u{1}axion:reader:";

const SOURCE: &str = include_str!("reader.js");

/// The script injected into every top-level document.
pub fn page_script(config: &Reader) -> String {
    SOURCE
        .replace("__AXION_MIN_CHARS__", &config.min_chars.to_string())
        .replace("__AXION_DELAY__", &config.detect_delay_ms.to_string())
}

/// What Reader view looks like, with the theme resolved to a concrete one.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Look {
    pub font: ReaderFont,
    pub size: f32,
    pub line_height: f32,
    pub width: f32,
    pub theme: ReaderTheme,
}

impl Look {
    /// The look the settings describe.
    pub fn from_config(config: &Reader) -> Self {
        Self {
            font: config.font,
            size: config.font_size,
            line_height: config.line_height,
            width: config.width,
            theme: config.theme,
        }
    }

    /// Encodes the look for the per-site store: `serif;20;1.6;680;sepia`.
    pub fn encode(&self) -> String {
        format!(
            "{};{};{};{};{}",
            font_name(self.font),
            self.size,
            self.line_height,
            self.width,
            theme_name(self.theme)
        )
    }

    /// Reads [`Look::encode`]'s output (`None` for anything else).
    pub fn decode(text: &str) -> Option<Self> {
        let mut parts = text.split(';');
        let font = match parts.next()? {
            "serif" => ReaderFont::Serif,
            "sans" => ReaderFont::Sans,
            "mono" => ReaderFont::Mono,
            _ => return None,
        };
        let size = parts.next()?.parse().ok()?;
        let line_height = parts.next()?.parse().ok()?;
        let width = parts.next()?.parse().ok()?;
        let theme = match parts.next()? {
            "auto" => ReaderTheme::Auto,
            "light" => ReaderTheme::Light,
            "sepia" => ReaderTheme::Sepia,
            "dark" => ReaderTheme::Dark,
            _ => return None,
        };
        parts.next().is_none().then_some(Self { font, size, line_height, width, theme })
    }

    fn options_json(&self, dark: bool) -> String {
        let theme = match self.theme {
            ReaderTheme::Auto if dark => "dark",
            ReaderTheme::Auto | ReaderTheme::Light => "light",
            ReaderTheme::Sepia => "sepia",
            ReaderTheme::Dark => "dark",
        };
        serde_json::json!({
            "font": font_name(self.font),
            "size": self.size,
            "lineHeight": self.line_height,
            "width": self.width,
            "theme": theme,
        })
        .to_string()
    }
}

fn font_name(font: ReaderFont) -> &'static str {
    match font {
        ReaderFont::Serif => "serif",
        ReaderFont::Sans => "sans",
        ReaderFont::Mono => "mono",
    }
}

fn theme_name(theme: ReaderTheme) -> &'static str {
    match theme {
        ReaderTheme::Auto => "auto",
        ReaderTheme::Light => "light",
        ReaderTheme::Sepia => "sepia",
        ReaderTheme::Dark => "dark",
    }
}

/// Swaps the page for its article (`dark`: Axion's chrome is dark, for the Auto theme).
pub fn enter_script(look: &Look, dark: bool) -> String {
    format!("window.__axionReader && window.__axionReader.enter({});", look.options_json(dark))
}

/// Changes the look of the open Reader view.
pub fn apply_script(look: &Look, dark: bool) -> String {
    format!("window.__axionReader && window.__axionReader.apply({});", look.options_json(dark))
}

/// Brings the original page back.
pub fn exit_script() -> &'static str {
    "window.__axionReader && window.__axionReader.exit();"
}

/// Asks the page to check for an article again.
pub fn probe_script() -> &'static str {
    "window.__axionReader && window.__axionReader.probe();"
}

/// What the page tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    /// The page was checked: `readable` when it has an article; `active` while Reader view shows.
    Probe { readable: bool, active: bool },
    /// Reader view opened or closed.
    State { active: bool },
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum Wire {
    Probe {
        #[serde(default)]
        readable: bool,
        #[serde(default)]
        active: bool,
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
        Wire::Probe { readable, active } => Message::Probe { readable, active },
        Wire::State { active } => Message::State { active },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_round_trip() {
        let look = Look {
            font: ReaderFont::Sans,
            size: 22.0,
            line_height: 1.7,
            width: 760.0,
            theme: ReaderTheme::Sepia,
        };
        assert_eq!(Look::decode(&look.encode()), Some(look));
        assert_eq!(Look::decode("serif;20"), None);
        assert_eq!(Look::decode("fancy;20;1.6;680;auto"), None);
    }

    #[test]
    fn auto_theme_follows_chrome() {
        let look = Look::from_config(&Reader::default());
        assert!(enter_script(&look, true).contains(r#""theme":"dark""#));
        assert!(enter_script(&look, false).contains(r#""theme":"light""#));
    }

    #[test]
    fn parses_messages() {
        let m = format!(r#"{MARKER}{{"t":"probe","readable":true,"active":false}}"#);
        assert_eq!(parse(&m), Some(Message::Probe { readable: true, active: false }));
        let m = format!(r#"{MARKER}{{"t":"state","active":true}}"#);
        assert_eq!(parse(&m), Some(Message::State { active: true }));
        assert_eq!(parse("\u{1}axion:{}"), None);
        assert_eq!(parse(&format!("{MARKER}nonsense")), None);
    }

    #[test]
    fn script_gets_its_limits() {
        let script = page_script(&Reader::default());
        assert!(!script.contains("__AXION_"));
        assert!(script.contains("600"));
    }
}
