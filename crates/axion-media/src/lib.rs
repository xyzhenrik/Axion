//! The media bridge: a script injected into every frame that finds the page's `<video>` and
//! `<audio>` elements (also in shadow DOM, iframes and never-attached `new Audio()`), reports
//! their state, and plays, seeks or fullscreens them on command. This crate holds the script
//! and both directions of its protocol; the browser draws the controls.
//!
//! Axion's player only takes over videos in fullscreen; inline, pages keep their own controls,
//! and trusted sites (YouTube, Twitch…) keep their player also in fullscreen.
//!
//! Page → browser: a console message `"\u{1}axion:" + JSON` ([`Message`]) from the top frame.
//! Browser → page: JavaScript from [`Command::script`], run in the top frame.

use serde::{Deserialize, Serialize};

/// Console messages starting with this come from the bridge.
pub const MARKER: &str = "\u{1}axion:";

const SOURCE: &str = include_str!("bridge.js");

/// How the bridge treats the page's players.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeOptions {
    /// Axion's player takes over videos in fullscreen (the built-in controls of a fullscreen
    /// `<video>` are hidden).
    pub own_fullscreen: bool,
    /// Sites (and their subdomains) whose players are kept as they are, also in fullscreen.
    pub trusted_sites: Vec<String>,
}

/// The bridge script. `secret` keeps the frames' postMessage traffic apart from the page's.
pub fn bridge_script(secret: &str, options: &BridgeOptions) -> String {
    let secret: String = secret.chars().filter(char::is_ascii_alphanumeric).collect();
    let trusted: Vec<String> = options.trusted_sites.iter().map(|s| s.trim().to_ascii_lowercase()).collect();
    let trusted = serde_json::to_string(&trusted).unwrap_or_else(|_| "[]".into());
    SOURCE
        .replace("__AXION_SECRET__", &secret)
        .replace("__AXION_TRUSTED__", &trusted)
        .replace("__AXION_OWN_FULLSCREEN__", if options.own_fullscreen { "true" } else { "false" })
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
    /// The video's own size in pixels (0 while unknown, and for audio).
    pub vw: f32,
    pub vh: f32,
    /// The element's CSS `object-fit` ("contain" for a video unless the page changes it).
    pub fit: String,
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
    /// It has the browser's built-in controls (`<video controls>`).
    pub native: bool,
    /// It plays on a trusted site, whose player stays as it is (also in fullscreen).
    pub trusted: bool,
    /// It is (inside) the page's fullscreen element.
    pub fullscreen: bool,
    /// The address the element currently plays (`currentSrc`): a plain file, or `blob:` for
    /// streams the page assembles itself.
    pub src: String,
    /// The `poster` picture of a `<video>`.
    pub poster: String,
}

impl Player {
    pub fn is_playing(&self) -> bool {
        !self.paused && !self.ended
    }

    /// Playing and audible.
    pub fn is_audible(&self) -> bool {
        self.is_playing() && !self.muted && self.volume > 0.0
    }

    /// Width / height of the picture: the video's own, else its element's.
    pub fn aspect(&self) -> f32 {
        if self.vw > 0.0 && self.vh > 0.0 {
            self.vw / self.vh
        } else if self.w > 0.0 && self.h > 0.0 {
            self.w / self.h
        } else {
            16.0 / 9.0
        }
    }

    /// Where the picture is drawn inside the element (CSS px, like `x`/`y`/`w`/`h`): the
    /// element is often bigger than the video, which then sits letterboxed in it (`contain`,
    /// the default). `cover`/`fill`, or an unknown video size, use the whole element. Assumes
    /// the default, centred `object-position`.
    pub fn picture_rect(&self) -> (f32, f32, f32, f32) {
        let whole = (self.x, self.y, self.w, self.h);
        let contained = matches!(self.fit.as_str(), "" | "contain" | "scale-down" | "none");
        if !contained || self.vw <= 0.0 || self.vh <= 0.0 || self.w <= 0.0 || self.h <= 0.0 {
            return whole;
        }
        let aspect = self.vw / self.vh;
        let (mut w, mut h) =
            if self.w / self.h > aspect { (self.h * aspect, self.h) } else { (self.w, self.w / aspect) };
        // `scale-down` and `none` never scale the video up.
        if self.fit != "contain" && !self.fit.is_empty() && w > self.vw {
            (w, h) = (self.vw, self.vh);
        }
        (self.x + (self.w - w) / 2.0, self.y + (self.h - h) / 2.0, w, h)
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
    /// The page's preview picture (`og:image`), reported while it has players.
    pub image: String,
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
    /// A player's loading state, asked for by [`Action::Probe`] (a debugging aid): its frame,
    /// source, ready/network state, error, and the frame's codec support, as JSON.
    Probe(String),
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
    Probe(serde_json::Map<String, serde_json::Value>),
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
        Wire::Probe(fields) => Some(Message::Probe(serde_json::Value::Object(fields).to_string())),
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
    /// Log the element's loading state from its own frame (debugging).
    Probe,
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
        !matches!(self.action, Action::Takeover(_) | Action::Pause | Action::Probe)
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
            Action::Probe => ("probe", serde_json::Value::Null),
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
    fn picture_rect_skips_the_elements_black_bars() {
        // A 16:9 video in a 900×600 element: bars above and below.
        let p = Player { x: 10.0, y: 20.0, w: 900.0, h: 600.0, vw: 1280.0, vh: 720.0, ..Default::default() };
        assert_eq!(p.picture_rect(), (10.0, 20.0 + (600.0 - 506.25) / 2.0, 900.0, 506.25));
        // Bars left and right for a tall element's 4:3 video.
        let p = Player { w: 400.0, h: 400.0, vw: 400.0, vh: 300.0, fit: "contain".into(), ..p };
        assert_eq!(p.picture_rect(), (10.0, 20.0 + 50.0, 400.0, 300.0));
        // `cover` fills the element; so does a video of unknown size.
        let cover = Player { fit: "cover".into(), ..p.clone() };
        assert_eq!(cover.picture_rect(), (10.0, 20.0, 400.0, 400.0));
        let unknown = Player { vw: 0.0, ..p };
        assert_eq!(unknown.picture_rect(), (10.0, 20.0, 400.0, 400.0));
    }

    #[test]
    fn script_is_filled_in() {
        let options = BridgeOptions { own_fullscreen: true, trusted_sites: vec!["YouTube.com".into()] };
        let s = bridge_script("abc'; alert(1)//", &options);
        assert!(s.contains(r#"})(location.hostname, ["youtube.com"]);"#));
        assert!(s.contains("var SECRET = 'abcalert1';"), "secret must be sanitised");
        assert!(s.contains("var OWN_FULLSCREEN = true;"));
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
        let probe = format!("{MARKER}{}", r#"{"t":"probe","ready":0,"h264":false}"#);
        let Ok(Some(Message::Probe(state))) = parse(&probe) else { panic!("{probe}") };
        assert!(state.contains("\"h264\":false"));
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
