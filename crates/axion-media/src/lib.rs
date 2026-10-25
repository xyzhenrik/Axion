//! The media bridge: a script injected into every frame that finds the page's `<video>` and
//! `<audio>` elements (also in shadow DOM, iframes and never-attached `new Audio()`), hides
//! their native controls, reports their state, and plays, seeks or fullscreens them on
//! command. This crate holds the script and both directions of its protocol; the browser
//! draws the controls.
//!
//! Page → browser: a console message `"\u{1}axion:" + JSON` ([`Message`]) from the top frame.
//! Browser → page: JavaScript from [`Command::script`], run in the top frame.

use serde::{Deserialize, Serialize};

/// Console messages starting with this come from the bridge.
pub const MARKER: &str = "\u{1}axion:";

const SOURCE: &str = include_str!("bridge.js");

/// Which page controls Axion's replace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Replace {
    /// `<video controls>`: the browser's built-in controls.
    pub native_controls: bool,
    /// Sites' own players (YouTube…): their UI is hidden around the video.
    pub site_players: bool,
}

/// The bridge script. `secret` keeps the frames' postMessage traffic apart from the page's.
pub fn bridge_script(secret: &str, replace: Replace) -> String {
    let secret: String = secret.chars().filter(char::is_ascii_alphanumeric).collect();
    let flag = |on: bool| if on { "true" } else { "false" };
    SOURCE
        .replace("__AXION_SECRET__", &secret)
        .replace("__AXION_REPLACE_SITE__", flag(replace.site_players))
        .replace("__AXION_REPLACE__", flag(replace.native_controls))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Video,
    Audio,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Caption {
    /// Index into the element's text tracks.
    pub index: i32,
    pub label: String,
}

/// One media element, as the page last reported it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Player {
    pub id: String,
    pub kind: Kind,
    /// Position and size in CSS pixels of the top frame's viewport.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub paused: bool,
    pub ended: bool,
    /// Seconds.
    pub time: f64,
    /// Seconds; 0 while unknown.
    pub duration: f64,
    pub live: bool,
    pub volume: f64,
    pub muted: bool,
    pub rate: f64,
    /// End of the buffered range around the current time (seconds).
    pub buffered: f64,
    pub captions: Vec<Caption>,
    /// The showing caption track, -1 for none.
    pub caption: i32,
    /// It had native controls.
    pub native: bool,
    /// Axion's controls replace the page's (native ones or a site player's UI).
    pub own: bool,
    /// It is (inside) the page's fullscreen element.
    pub fullscreen: bool,
}

impl Player {
    pub fn is_playing(&self) -> bool {
        !self.paused && !self.ended
    }

    /// Playing and audible.
    pub fn is_audible(&self) -> bool {
        self.is_playing() && !self.muted && self.volume > 0.0
    }

    pub fn area(&self) -> f32 {
        self.w.max(0.0) * self.h.max(0.0)
    }
}

/// What the page says about what is playing (`navigator.mediaSession.metadata`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub artwork: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PageMedia {
    pub players: Vec<Player>,
    /// CSS pixel → physical pixel (includes page zoom).
    pub dpr: f32,
    pub session: Option<SessionInfo>,
}

impl PageMedia {
    /// The player the browser's controls are about: the playing one with the biggest area,
    /// else the biggest video, else any audio.
    pub fn primary(&self) -> Option<&Player> {
        let biggest = |filter: &dyn Fn(&Player) -> bool| {
            self.players.iter().filter(|p| filter(p)).max_by(|a, b| a.area().total_cmp(&b.area()))
        };
        biggest(&|p| p.is_playing())
            .or_else(|| biggest(&|p| p.kind == Kind::Video && p.area() > 0.0))
            .or_else(|| self.players.first())
    }

    pub fn get(&self, id: &str) -> Option<&Player> {
        self.players.iter().find(|p| p.id == id)
    }

    pub fn is_audible(&self) -> bool {
        self.players.iter().any(Player::is_audible)
    }
}

/// Something the page tells the browser.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    State(PageMedia),
    /// The user double-clicked a video whose controls are ours.
    DoubleClick {
        id: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("not a media bridge message")]
    NotBridge,
    #[error("malformed media bridge message: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum Wire {
    State(PageMedia),
    Event { event: WireEvent },
}

#[derive(Deserialize)]
struct WireEvent {
    e: String,
    #[serde(default)]
    id: String,
}

/// Parses a console message; [`ParseError::NotBridge`] for the page's own messages.
pub fn parse(console_message: &str) -> Result<Option<Message>, ParseError> {
    let json = console_message.strip_prefix(MARKER).ok_or(ParseError::NotBridge)?;
    Ok(match serde_json::from_str::<Wire>(json)? {
        Wire::State(state) => Some(Message::State(state)),
        Wire::Event { event } if event.e == "dblclick" => Some(Message::DoubleClick { id: event.id }),
        Wire::Event { .. } => None,
    })
}

/// Something the browser tells a player to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Play,
    Pause,
    Toggle,
    /// To this second.
    Seek(f64),
    /// By this many seconds.
    Skip(f64),
    Volume(f64),
    Mute(bool),
    Rate(f64),
    /// Show this caption track (-1: none).
    Caption(i32),
    /// Real fullscreen; needs to run with a user gesture.
    Fullscreen(bool),
    /// Let the video cover the page (used inside a page-made fullscreen).
    Takeover(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub player: String,
    pub action: Action,
}

impl Command {
    pub fn new(player: &str, action: Action) -> Self {
        Self { player: player.to_owned(), action }
    }

    /// Whether the script must run as if the user had clicked: fullscreen and playback with
    /// sound are refused otherwise. Commands come from the user's clicks and keys, so this
    /// only passes their gesture on.
    pub fn needs_gesture(&self) -> bool {
        !matches!(self.action, Action::Takeover(_) | Action::Pause)
    }

    /// JavaScript for the top frame.
    pub fn script(&self) -> String {
        let (name, value) = match self.action {
            Action::Play => ("play", serde_json::Value::Null),
            Action::Pause => ("pause", serde_json::Value::Null),
            Action::Toggle => ("toggle", serde_json::Value::Null),
            Action::Seek(t) => ("seek", t.into()),
            Action::Skip(t) => ("skip", t.into()),
            Action::Volume(v) => ("volume", v.into()),
            Action::Mute(m) => ("mute", m.into()),
            Action::Rate(r) => ("rate", r.into()),
            Action::Caption(i) => ("caption", i.into()),
            Action::Fullscreen(on) => ("fullscreen", on.into()),
            Action::Takeover(on) => ("takeover", on.into()),
        };
        let command = serde_json::json!({ "id": self.player, "action": name, "value": value });
        format!("window.__axionMedia && window.__axionMedia.cmd({command});")
    }
}

/// "1:05", "1:02:03"; live streams have no duration.
pub fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0).floor() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_is_filled_in() {
        let s = bridge_script("abc'; alert(1)//", Replace { native_controls: true, site_players: false });
        assert!(s.contains("var SECRET = 'abcalert1';"), "secret must be sanitised");
        assert!(s.contains("var REPLACE_NATIVE = true;"));
        assert!(s.contains("var REPLACE_SITE = false;"));
        assert!(!s.contains("__AXION_"));
    }

    #[test]
    fn parses_state_and_events() {
        let msg = format!(
            "{MARKER}{}",
            r#"{"t":"state","dpr":2,"players":[{"id":"a:1","kind":"video","x":10,"y":20,"w":640,"h":360,
            "paused":false,"time":12.5,"duration":100,"volume":1,"rate":1,"captions":[{"index":0,"label":"English"}],
            "caption":-1,"native":true}],"session":{"title":"Song"}}"#
        );
        let Ok(Some(Message::State(state))) = parse(&msg) else { panic!("{msg}") };
        assert_eq!(state.dpr, 2.0);
        let p = state.primary().unwrap();
        assert!(p.is_audible() && p.native);
        assert_eq!(p.captions[0].label, "English");
        assert_eq!(state.session.unwrap().title, "Song");
        let dbl = format!("{MARKER}{}", r#"{"t":"event","event":{"e":"dblclick","id":"a:1"}}"#);
        assert_eq!(parse(&dbl).unwrap(), Some(Message::DoubleClick { id: "a:1".into() }));
        assert!(matches!(parse("hello"), Err(ParseError::NotBridge)));
    }

    #[test]
    fn primary_prefers_playing_then_big_video() {
        let p =
            |id: &str, kind, w, paused| Player { id: id.into(), kind, w, h: w, paused, ..Default::default() };
        let mut m = PageMedia {
            players: vec![p("small", Kind::Video, 10.0, true), p("big", Kind::Video, 100.0, true)],
            ..Default::default()
        };
        assert_eq!(m.primary().unwrap().id, "big");
        m.players.push(p("music", Kind::Audio, 0.0, false));
        assert_eq!(m.primary().unwrap().id, "music");
    }

    #[test]
    fn commands_are_json() {
        let c = Command::new("x\"y", Action::Seek(5.5));
        assert_eq!(
            c.script(),
            r#"window.__axionMedia && window.__axionMedia.cmd({"action":"seek","id":"x\"y","value":5.5});"#
        );
        assert!(Command::new("a", Action::Fullscreen(true)).needs_gesture());
        assert_eq!(format_time(3723.4), "1:02:03");
        assert_eq!(format_time(65.0), "1:05");
    }
}
