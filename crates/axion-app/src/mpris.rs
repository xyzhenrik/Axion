//! MPRIS: tells the desktop what plays in Axion (media widgets, lock screen) and receives its
//! commands — which is also how media keys reach a Linux app.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use zbus::blocking::connection;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::{block_on, interface};

const PATH: &str = "/org/mpris/MediaPlayer2";
const TRACK_ID: &str = "/org/axion/Track";

/// What the desktop asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MprisCommand {
    Raise,
    Play,
    Pause,
    PlayPause,
    /// Relative, seconds.
    Seek(f64),
    /// Absolute, seconds.
    SetPosition(f64),
    Volume(f64),
    Rate(f64),
}

/// What is playing, as the desktop sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: String,
    /// Seconds (0: unknown or live).
    pub length: f64,
    pub position: f64,
    pub volume: f64,
    pub rate: f64,
}

struct Root {
    commands: Sender<MprisCommand>,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        let _ = self.commands.send(MprisCommand::Raise);
    }

    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        "Axion".into()
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        "axion".into()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct PlayerIface {
    commands: Sender<MprisCommand>,
    track: Option<Track>,
}

impl PlayerIface {
    fn send(&self, command: MprisCommand) {
        let _ = self.commands.send(command);
    }
}

fn micros(seconds: f64) -> i64 {
    (seconds * 1_000_000.0) as i64
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerIface {
    fn play(&self) {
        self.send(MprisCommand::Play);
    }

    fn pause(&self) {
        self.send(MprisCommand::Pause);
    }

    fn play_pause(&self) {
        self.send(MprisCommand::PlayPause);
    }

    fn stop(&self) {
        self.send(MprisCommand::Pause);
    }

    fn next(&self) {}

    fn previous(&self) {}

    fn seek(&self, offset: i64) {
        self.send(MprisCommand::Seek(offset as f64 / 1_000_000.0));
    }

    fn set_position(&self, _track_id: ObjectPath<'_>, position: i64) {
        self.send(MprisCommand::SetPosition(position as f64 / 1_000_000.0));
    }

    fn open_uri(&self, _uri: String) {}

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        match &self.track {
            Some(t) if t.playing => "Playing",
            Some(_) => "Paused",
            None => "Stopped",
        }
        .into()
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let mut map = HashMap::new();
        let Some(t) = &self.track else { return map };
        let mut put = |key: &str, value: Value<'_>| {
            if let Ok(v) = value.try_into() {
                map.insert(key.to_owned(), v);
            }
        };
        if let Ok(path) = ObjectPath::try_from(TRACK_ID) {
            put("mpris:trackid", Value::from(path));
        }
        put("xesam:title", Value::from(t.title.clone()));
        if !t.artist.is_empty() {
            put("xesam:artist", Value::from(vec![t.artist.clone()]));
        }
        if !t.album.is_empty() {
            put("xesam:album", Value::from(t.album.clone()));
        }
        if !t.art_url.is_empty() {
            put("mpris:artUrl", Value::from(t.art_url.clone()));
        }
        if t.length > 0.0 {
            put("mpris:length", Value::from(micros(t.length)));
        }
        map
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.track.as_ref().map_or(0, |t| micros(t.position))
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        self.track.as_ref().map_or(1.0, |t| t.rate)
    }

    #[zbus(property)]
    fn set_rate(&mut self, rate: f64) {
        if rate > 0.0 {
            self.send(MprisCommand::Rate(rate));
        }
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        0.25
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        4.0
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.track.as_ref().map_or(1.0, |t| t.volume)
    }

    #[zbus(property)]
    fn set_volume(&mut self, volume: f64) {
        self.send(MprisCommand::Volume(volume.clamp(0.0, 1.0)));
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.track.is_some()
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.track.is_some()
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.track.as_ref().is_some_and(|t| t.length > 0.0)
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

pub struct Mpris {
    connection: zbus::blocking::Connection,
    commands: Receiver<MprisCommand>,
    last: Option<Track>,
}

/// Position jumps bigger than this between two updates are reported as seeks.
const SEEK_TOLERANCE: f64 = 1.5;

impl Mpris {
    /// Registers on the session bus; `None` without one.
    pub fn start() -> Option<Self> {
        let (tx, rx) = channel();
        let name = format!("org.mpris.MediaPlayer2.axion.instance{}", std::process::id());
        let connection = connection::Builder::session()
            .and_then(|b| b.name(name))
            .and_then(|b| b.serve_at(PATH, Root { commands: tx.clone() }))
            .and_then(|b| b.serve_at(PATH, PlayerIface { commands: tx, track: None }))
            .and_then(|b| b.build())
            .inspect_err(|e| tracing::warn!("media keys and desktop media controls unavailable: {e}"))
            .ok()?;
        Some(Self { connection, commands: rx, last: None })
    }

    /// Commands received since the last call.
    pub fn commands(&self) -> Vec<MprisCommand> {
        self.commands.try_iter().collect()
    }

    /// Publishes what plays now (`None`: nothing).
    pub fn update(&mut self, track: Option<Track>) {
        if track == self.last {
            return;
        }
        let Ok(iface) = self.connection.object_server().interface::<_, PlayerIface>(PATH) else { return };
        let previous = std::mem::replace(&mut self.last, track.clone());
        iface.get_mut().track = track.clone();
        let emitter = iface.signal_emitter();
        let changed = |a: &Option<Track>, b: &Option<Track>, f: fn(&Track) -> (bool, String)| {
            a.as_ref().map(f) != b.as_ref().map(f)
        };
        let result = block_on(async {
            let player = iface.get();
            if changed(&previous, &track, |t| (t.playing, String::new())) {
                player.playback_status_changed(emitter).await?;
                player.can_play_changed(emitter).await?;
                player.can_pause_changed(emitter).await?;
            }
            if changed(&previous, &track, |t| {
                (true, format!("{}|{}|{}|{}|{}", t.title, t.artist, t.album, t.art_url, t.length))
            }) {
                player.metadata_changed(emitter).await?;
                player.can_seek_changed(emitter).await?;
            }
            if changed(&previous, &track, |t| (true, format!("{}|{}", t.volume, t.rate))) {
                player.volume_changed(emitter).await?;
                player.rate_changed(emitter).await?;
            }
            // Updates arrive a few times a second, so playback alone moves the position far less.
            if let (Some(p), Some(t)) = (&previous, &track)
                && (t.position - p.position).abs() > SEEK_TOLERANCE
            {
                PlayerIface::seeked(emitter, micros(t.position)).await?;
            }
            zbus::Result::Ok(())
        });
        if let Err(e) = result {
            tracing::debug!("MPRIS update failed: {e}");
        }
    }
}
