//! Site permissions: what to do with a page's request for the microphone, camera, screen,
//! notifications, … (allow, block, or ask the user), the decisions remembered per site, and what
//! each tab captures right now.
//!
//! Rules (see `docs/features/site-permissions.md`):
//! - Remembered decisions (normal tabs: in the profile's database; private tabs: in memory until
//!   Axion quits) answer without asking. Screen sharing is never remembered: every share asks.
//! - Otherwise a kind whose default is Block is refused without asking.
//! - "Allow While Visiting" (microphone, camera) lasts while the tab stays on the site.
//! - Closing the prompt (Esc, navigation) decides nothing.
//!
//! This module is the model only; the browser applies its results to the engine, the database
//! and the prompt (`crate::permission_prompt`).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use axion_config::{Config, PermissionDefault};
use axion_engine::{PermissionId, PermissionKind, ViewId};
use axion_i18n::tr_mark;

/// How kinds are grouped for defaults (Settings › Privacy › Site Permissions).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Microphone,
    Camera,
    ScreenSharing,
    Notifications,
    Location,
    Clipboard,
    Other,
}

impl Group {
    pub const ALL: [Group; 7] = [
        Group::Microphone,
        Group::Camera,
        Group::ScreenSharing,
        Group::Notifications,
        Group::Location,
        Group::Clipboard,
        Group::Other,
    ];

    pub fn of(kind: PermissionKind) -> Group {
        match kind {
            PermissionKind::Microphone => Group::Microphone,
            PermissionKind::Camera => Group::Camera,
            PermissionKind::ScreenShare | PermissionKind::ScreenAudio => Group::ScreenSharing,
            PermissionKind::Notifications => Group::Notifications,
            PermissionKind::Geolocation => Group::Location,
            PermissionKind::Clipboard => Group::Clipboard,
            _ => Group::Other,
        }
    }

    /// Its name in Settings (English source text; translate with `tr`).
    pub fn name(self) -> &'static str {
        match self {
            Group::Microphone => tr_mark("Microphone"),
            Group::Camera => tr_mark("Camera"),
            Group::ScreenSharing => tr_mark("Screen sharing"),
            Group::Notifications => tr_mark("Notifications"),
            Group::Location => tr_mark("Location"),
            Group::Clipboard => tr_mark("Clipboard"),
            Group::Other => tr_mark("Other permissions"),
        }
    }

    pub fn default_of(self, config: &Config) -> PermissionDefault {
        let p = &config.permissions;
        match self {
            Group::Microphone => p.microphone,
            Group::Camera => p.camera,
            Group::ScreenSharing => p.screen_sharing,
            Group::Notifications => p.notifications,
            Group::Location => p.location,
            Group::Clipboard => p.clipboard,
            Group::Other => p.other,
        }
    }

    pub fn set_default(self, config: &mut Config, value: PermissionDefault) {
        let p = &mut config.permissions;
        let slot = match self {
            Group::Microphone => &mut p.microphone,
            Group::Camera => &mut p.camera,
            Group::ScreenSharing => &mut p.screen_sharing,
            Group::Notifications => &mut p.notifications,
            Group::Location => &mut p.location,
            Group::Clipboard => &mut p.clipboard,
            Group::Other => &mut p.other,
        };
        *slot = value;
    }
}

/// What a kind is called in a list of decisions (English source text).
pub fn kind_name(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Microphone => tr_mark("Microphone"),
        PermissionKind::Camera => tr_mark("Camera"),
        PermissionKind::ScreenShare | PermissionKind::ScreenAudio => tr_mark("Screen sharing"),
        PermissionKind::Notifications => tr_mark("Notifications"),
        PermissionKind::Geolocation => tr_mark("Location"),
        PermissionKind::Clipboard => tr_mark("Clipboard"),
        PermissionKind::Midi => tr_mark("MIDI devices"),
        PermissionKind::LocalNetwork => tr_mark("Local network"),
        PermissionKind::WindowManagement => tr_mark("Window placement"),
        PermissionKind::LocalFonts => tr_mark("Fonts"),
        PermissionKind::IdleDetection => tr_mark("Idle detection"),
        PermissionKind::PointerLock => tr_mark("Pointer lock"),
        PermissionKind::KeyboardLock => tr_mark("Keyboard lock"),
        PermissionKind::Sensors => tr_mark("Motion sensors"),
        PermissionKind::AutomaticDownloads => tr_mark("Automatic downloads"),
    }
}

/// What the prompt's line for a kind says, after "<site> wants to" (English source text).
pub fn kind_request(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Microphone => tr_mark("Use your microphone"),
        PermissionKind::Camera => tr_mark("Use your camera"),
        PermissionKind::ScreenShare => tr_mark("Share your screen or a window"),
        PermissionKind::ScreenAudio => tr_mark("Share the sound of your computer"),
        PermissionKind::Notifications => tr_mark("Show notifications"),
        PermissionKind::Geolocation => tr_mark("Know your location"),
        PermissionKind::Clipboard => tr_mark("See what you copied"),
        PermissionKind::Midi => tr_mark("Use your MIDI devices"),
        PermissionKind::LocalNetwork => tr_mark("Connect to devices on your network"),
        PermissionKind::WindowManagement => tr_mark("Place windows on all your displays"),
        PermissionKind::LocalFonts => tr_mark("Use the fonts on this computer"),
        PermissionKind::IdleDetection => tr_mark("Know when you are away"),
        PermissionKind::PointerLock => tr_mark("Hide and lock the pointer"),
        PermissionKind::KeyboardLock => tr_mark("Use system keys such as Esc"),
        PermissionKind::Sensors => tr_mark("Use motion and light sensors"),
        PermissionKind::AutomaticDownloads => tr_mark("Download several files"),
    }
}

/// Whether a decision for `kind` may be remembered: everything but screen sharing.
pub fn rememberable(kind: PermissionKind) -> bool {
    Group::of(kind) != Group::ScreenSharing
}

/// Whether the prompt offers "Allow While Visiting" for these kinds: only microphone and
/// camera (Chromium itself remembers what the other prompts decide).
pub fn offers_visit(kinds: &[PermissionKind]) -> bool {
    kinds.iter().all(|k| matches!(k, PermissionKind::Microphone | PermissionKind::Camera))
}

/// `scheme://host[:port]` of a URL or origin, lower case, without a path (`https://a.com/` and
/// `https://A.com/x` are the same origin).
pub fn origin_of(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.trim_end_matches('/').to_owned() };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    format!("{}://{}", scheme.to_ascii_lowercase(), authority.to_ascii_lowercase())
}

/// What happens with a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Block,
    Ask,
}

/// The user's answer in the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Allowed and remembered (private tabs: until Axion quits). Screen sharing: just this once.
    Allow,
    /// Microphone / camera: allowed while the tab stays on the site.
    AllowWhileVisiting,
    /// Refused and remembered (screen sharing: just this once).
    Block,
    /// Closed without deciding.
    Dismiss,
}

/// A decision to write down (database for normal tabs, the engine's own store for both).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remember {
    pub origin: String,
    pub kind: PermissionKind,
    pub allow: bool,
    pub private: bool,
}

/// A page's request waiting for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub view: ViewId,
    pub id: PermissionId,
    pub origin: String,
    pub kinds: Vec<PermissionKind>,
    pub private: bool,
}

/// What a tab records now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capture {
    pub microphone: bool,
    pub camera: bool,
    pub screen: bool,
}

impl Capture {
    pub fn any(self) -> bool {
        self.microphone || self.camera || self.screen
    }

    /// The kinds to stop for all of it.
    pub fn kinds(self) -> Vec<PermissionKind> {
        let mut kinds = Vec::new();
        if self.microphone {
            kinds.push(PermissionKind::Microphone);
        }
        if self.camera {
            kinds.push(PermissionKind::Camera);
        }
        if self.screen {
            kinds.push(PermissionKind::ScreenShare);
        }
        kinds
    }
}

/// Decisions, the waiting requests, and what each tab is granted and captures.
#[derive(Default)]
pub struct SitePermissions {
    /// Remembered decisions of normal tabs (a copy of the database's), by origin and kind.
    remembered: BTreeMap<(String, PermissionKind), bool>,
    /// Remembered decisions of private tabs (never written anywhere).
    private: HashMap<(String, PermissionKind), bool>,
    /// "Allow While Visiting": by tab, the (origin, kind) it may use until it leaves the site.
    visits: HashMap<ViewId, HashSet<(String, PermissionKind)>>,
    /// The site each tab is on (its top-level origin), to end visits.
    sites: HashMap<ViewId, String>,
    /// Capture kinds granted to each tab on its current site (to tell a camera from a screen).
    granted: HashMap<ViewId, HashSet<PermissionKind>>,
    /// The engine's last report of each tab's capture (audio, video).
    reported: HashMap<ViewId, (bool, bool)>,
    /// Requests waiting for the user, oldest first.
    queue: VecDeque<Request>,
}

impl SitePermissions {
    /// Starts from the database's remembered decisions (`(origin, kind key, allowed)`).
    pub fn new(remembered: impl IntoIterator<Item = (String, String, bool)>) -> Self {
        let mut this = Self::default();
        for (origin, key, allowed) in remembered {
            if let Some(kind) = PermissionKind::from_key(&key).filter(|k| rememberable(*k)) {
                this.remembered.insert((origin, kind), allowed);
            }
        }
        this
    }

    /// The remembered decisions of normal tabs, sorted by origin then kind.
    pub fn remembered(&self) -> Vec<(String, PermissionKind, bool)> {
        self.remembered.iter().map(|((o, k), a)| (o.clone(), *k, *a)).collect()
    }

    /// Forgets one remembered decision (normal tabs).
    pub fn forget(&mut self, origin: &str, kind: PermissionKind) {
        self.remembered.remove(&(origin.to_owned(), kind));
    }

    /// Forgets every remembered decision of normal tabs, and returns them.
    pub fn forget_all(&mut self) -> Vec<(String, PermissionKind, bool)> {
        let all = self.remembered();
        self.remembered.clear();
        all
    }

    /// What to do with a request of `view` (`private`: a private tab) from `origin`.
    pub fn decide(
        &self,
        config: &Config,
        view: ViewId,
        origin: &str,
        kinds: &[PermissionKind],
        private: bool,
    ) -> Decision {
        let origin = origin_of(origin);
        let mut all_allowed = !kinds.is_empty();
        for &kind in kinds {
            let key = (origin.clone(), kind);
            let remembered =
                if private { self.private.get(&key) } else { self.remembered.get(&key) }.copied();
            // A site's own decision wins over the default (as Chromium's site exceptions do).
            match remembered.filter(|_| rememberable(kind)) {
                Some(false) => return Decision::Block,
                Some(true) => {}
                None if Group::of(kind).default_of(config).blocks() => return Decision::Block,
                None => {
                    let visiting =
                        rememberable(kind) && self.visits.get(&view).is_some_and(|v| v.contains(&key));
                    all_allowed &= visiting;
                }
            }
        }
        if all_allowed { Decision::Allow } else { Decision::Ask }
    }

    /// Queues a request for the prompt. A newer request of the same tab for the same kinds
    /// replaces an older one still waiting (the page asked again); the replaced one is returned
    /// (to be dismissed).
    pub fn enqueue(&mut self, request: Request) -> Option<Request> {
        let older = self
            .queue
            .iter()
            .position(|r| r.view == request.view && r.origin == request.origin && r.kinds == request.kinds);
        let replaced = older.and_then(|i| self.queue.remove(i));
        self.queue.push_back(request);
        replaced
    }

    /// The oldest waiting request of `view` (the shown tab: background tabs' requests wait).
    pub fn next_for(&self, view: Option<ViewId>) -> Option<&Request> {
        let view = view?;
        self.queue.iter().find(|r| r.view == view)
    }

    /// Takes a waiting request out of the queue.
    pub fn take(&mut self, view: ViewId, id: PermissionId) -> Option<Request> {
        let i = self.queue.iter().position(|r| r.view == view && r.id == id)?;
        self.queue.remove(i)
    }

    /// Takes every waiting request of `view` (it left the site or closed).
    pub fn take_all(&mut self, view: ViewId) -> Vec<Request> {
        let (gone, kept): (Vec<Request>, Vec<Request>) =
            std::mem::take(&mut self.queue).into_iter().partition(|r| r.view == view);
        self.queue = kept.into();
        gone
    }

    /// The user answered `request`: notes it and returns what to remember.
    pub fn record(&mut self, request: &Request, choice: Choice) -> Vec<Remember> {
        let origin = origin_of(&request.origin);
        let allow = match choice {
            Choice::Dismiss => return Vec::new(),
            Choice::Allow | Choice::AllowWhileVisiting => true,
            Choice::Block => false,
        };
        if allow {
            self.grant(request.view, &request.kinds);
        }
        let kinds = request.kinds.iter().copied().filter(|k| rememberable(*k));
        if choice == Choice::AllowWhileVisiting {
            let visits = self.visits.entry(request.view).or_default();
            for kind in kinds {
                visits.insert((origin.clone(), kind));
            }
            return Vec::new();
        }
        let mut out = Vec::new();
        for kind in kinds {
            let key = (origin.clone(), kind);
            if request.private {
                self.private.insert(key, allow);
            } else {
                self.remembered.insert(key, allow);
            }
            out.push(Remember { origin: origin.clone(), kind, allow, private: request.private });
        }
        out
    }

    /// The tab was given these capture kinds (by the user, or a remembered decision).
    pub fn grant(&mut self, view: ViewId, kinds: &[PermissionKind]) {
        self.granted.entry(view).or_default().extend(kinds.iter().copied().filter(|k| k.is_capture()));
    }

    /// The tab is on `url` now. Leaving a site ends its visit: "Allow While Visiting" grants
    /// lapse and requests still waiting are returned (to be dismissed).
    pub fn navigated(&mut self, view: ViewId, url: &str) -> Vec<Request> {
        let origin = origin_of(url);
        let previous = self.sites.insert(view, origin.clone());
        if previous.is_none_or(|p| p == origin) {
            return Vec::new();
        }
        self.visits.remove(&view);
        self.granted.remove(&view);
        self.take_all(view)
    }

    /// The user stopped `kinds` in the tab: a visit's grant for them ends too (asking again).
    pub fn stopped(&mut self, view: ViewId, kinds: &[PermissionKind]) {
        if let Some(visits) = self.visits.get_mut(&view) {
            visits.retain(|(_, k)| !kinds.contains(k));
        }
    }

    /// The tab closed.
    pub fn closed(&mut self, view: ViewId) -> Vec<Request> {
        self.visits.remove(&view);
        self.sites.remove(&view);
        self.granted.remove(&view);
        self.reported.remove(&view);
        self.take_all(view)
    }

    /// The engine reported the tab's capture: what it records, by kind.
    pub fn capture_changed(&mut self, view: ViewId, audio: bool, video: bool) -> Capture {
        self.reported.insert(view, (audio, video));
        self.capture(view)
    }

    /// What the tab records now. The engine only tells audio from video, so what the tab was
    /// granted says whether video is the camera or a screen (both if it has both).
    pub fn capture(&self, view: ViewId) -> Capture {
        let (audio, video) = self.reported.get(&view).copied().unwrap_or_default();
        let granted = self.granted.get(&view);
        let has = |k: PermissionKind| granted.is_some_and(|g| g.contains(&k));
        let screen_video = has(PermissionKind::ScreenShare);
        let camera = video && (has(PermissionKind::Camera) || !screen_video);
        let screen = video && screen_video;
        // A screen's sound alone isn't the microphone.
        let screen_sound_only = has(PermissionKind::ScreenAudio) && !has(PermissionKind::Microphone);
        Capture {
            microphone: audio && !screen_sound_only,
            camera,
            screen: screen || (audio && screen_sound_only),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAB: ViewId = ViewId(1);
    const DISCORD: &str = "https://discord.com/";

    fn request(kinds: &[PermissionKind], private: bool) -> Request {
        Request { view: TAB, id: PermissionId(1), origin: DISCORD.into(), kinds: kinds.to_vec(), private }
    }

    #[test]
    fn origins_ignore_paths_and_case() {
        assert_eq!(origin_of("https://Discord.com/channels/1"), "https://discord.com");
        assert_eq!(origin_of("https://discord.com/"), "https://discord.com");
        assert_eq!(origin_of("http://localhost:8000/x?y"), "http://localhost:8000");
        assert_eq!(origin_of("https://user@a.com/"), "https://a.com");
    }

    #[test]
    fn everything_asks_by_default_and_block_defaults_refuse() {
        let mut config = Config::default();
        let p = SitePermissions::default();
        for kind in PermissionKind::ALL {
            assert_eq!(p.decide(&config, TAB, DISCORD, &[kind], false), Decision::Ask, "{kind:?}");
        }
        config.permissions.camera = PermissionDefault::Block;
        let both = [PermissionKind::Microphone, PermissionKind::Camera];
        assert_eq!(p.decide(&config, TAB, DISCORD, &both, false), Decision::Block);
        config.permissions.other = PermissionDefault::Block;
        assert_eq!(p.decide(&config, TAB, DISCORD, &[PermissionKind::Midi], true), Decision::Block);
        // A site allowed before keeps its permission.
        let mut p = p;
        p.record(&request(&[PermissionKind::Camera], false), Choice::Allow);
        assert_eq!(p.decide(&config, TAB, DISCORD, &[PermissionKind::Camera], false), Decision::Allow);
        assert_eq!(
            p.decide(&config, TAB, "https://other.com", &[PermissionKind::Camera], false),
            Decision::Block
        );
        assert_eq!(Group::of(PermissionKind::ScreenAudio), Group::ScreenSharing);
    }

    #[test]
    fn remembered_decisions_answer_without_asking() {
        let config = Config::default();
        let mut p = SitePermissions::default();
        let mic = request(&[PermissionKind::Microphone], false);
        let remember = p.record(&mic, Choice::Allow);
        assert_eq!(
            remember,
            [Remember {
                origin: "https://discord.com".into(),
                kind: PermissionKind::Microphone,
                allow: true,
                private: false
            }]
        );
        // Another tab, another path of the site.
        let other = ViewId(2);
        assert_eq!(
            p.decide(&config, other, "https://discord.com/channels", &[PermissionKind::Microphone], false),
            Decision::Allow
        );
        // Microphone and camera together: the camera still asks.
        let both = [PermissionKind::Microphone, PermissionKind::Camera];
        assert_eq!(p.decide(&config, other, DISCORD, &both, false), Decision::Ask);
        // A block wins over everything.
        p.record(&request(&[PermissionKind::Camera], false), Choice::Block);
        assert_eq!(p.decide(&config, other, DISCORD, &both, false), Decision::Block);
        // Forgotten (Settings): asks again.
        p.forget("https://discord.com", PermissionKind::Camera);
        assert_eq!(p.decide(&config, other, DISCORD, &both, false), Decision::Ask);
        assert_eq!(p.forget_all().len(), 1);
        assert_eq!(p.decide(&config, other, DISCORD, &[PermissionKind::Microphone], false), Decision::Ask);
    }

    #[test]
    fn decisions_load_from_the_database() {
        let p = SitePermissions::new([
            ("https://a.com".into(), "notifications".into(), true),
            ("https://a.com".into(), "screen".into(), true),
            ("https://a.com".into(), "unknown".into(), true),
        ]);
        assert_eq!(p.remembered(), [("https://a.com".into(), PermissionKind::Notifications, true)]);
    }

    #[test]
    fn screen_sharing_is_never_remembered() {
        let config = Config::default();
        let mut p = SitePermissions::default();
        let share = request(&[PermissionKind::ScreenShare, PermissionKind::ScreenAudio], false);
        assert!(p.record(&share, Choice::Allow).is_empty());
        assert!(p.record(&share, Choice::Block).is_empty());
        assert!(p.record(&share, Choice::AllowWhileVisiting).is_empty());
        assert_eq!(p.decide(&config, TAB, DISCORD, &share.kinds, false), Decision::Ask);
        assert!(p.remembered().is_empty());
    }

    #[test]
    fn private_decisions_stay_in_memory_and_apart() {
        let config = Config::default();
        let mut p = SitePermissions::default();
        let remember = p.record(&request(&[PermissionKind::Notifications], true), Choice::Allow);
        assert!(remember.iter().all(|r| r.private));
        assert!(p.remembered().is_empty(), "nothing for the database");
        let kinds = [PermissionKind::Notifications];
        assert_eq!(p.decide(&config, TAB, DISCORD, &kinds, true), Decision::Allow);
        // Normal tabs don't see private decisions, and the reverse.
        assert_eq!(p.decide(&config, TAB, DISCORD, &kinds, false), Decision::Ask);
        p.record(&request(&[PermissionKind::Geolocation], false), Choice::Allow);
        assert_eq!(p.decide(&config, TAB, DISCORD, &[PermissionKind::Geolocation], true), Decision::Ask);
    }

    #[test]
    fn allow_while_visiting_ends_when_the_tab_leaves_the_site() {
        let config = Config::default();
        let mut p = SitePermissions::default();
        p.navigated(TAB, "https://discord.com/app");
        let kinds = [PermissionKind::Microphone, PermissionKind::Camera];
        assert!(p.record(&request(&kinds, false), Choice::AllowWhileVisiting).is_empty());
        assert_eq!(p.decide(&config, TAB, DISCORD, &kinds, false), Decision::Allow);
        // Only that tab.
        assert_eq!(p.decide(&config, ViewId(2), DISCORD, &kinds, false), Decision::Ask);
        // Same site, another page: still allowed.
        p.navigated(TAB, "https://discord.com/channels/@me");
        assert_eq!(p.decide(&config, TAB, DISCORD, &kinds, false), Decision::Allow);
        // Stopping the microphone takes its grant back.
        p.stopped(TAB, &[PermissionKind::Microphone]);
        assert_eq!(p.decide(&config, TAB, DISCORD, &[PermissionKind::Camera], false), Decision::Allow);
        assert_eq!(p.decide(&config, TAB, DISCORD, &[PermissionKind::Microphone], false), Decision::Ask);
        p.navigated(TAB, "https://example.com/");
        assert_eq!(p.decide(&config, TAB, DISCORD, &[PermissionKind::Camera], false), Decision::Ask);
    }

    #[test]
    fn the_queue_shows_the_shown_tabs_requests_and_drops_them_with_the_site() {
        let mut p = SitePermissions::default();
        p.navigated(TAB, DISCORD);
        let first = request(&[PermissionKind::Microphone], false);
        let background = Request { view: ViewId(2), id: PermissionId(2), ..first.clone() };
        assert!(p.enqueue(first.clone()).is_none());
        p.enqueue(background);
        assert_eq!(p.next_for(Some(TAB)).map(|r| r.id), Some(PermissionId(1)));
        assert_eq!(p.next_for(Some(ViewId(2))).map(|r| r.id), Some(PermissionId(2)));
        assert_eq!(p.next_for(None), None);
        // Asked again: the new request replaces the old one.
        let again = Request { id: PermissionId(3), ..first };
        assert_eq!(p.enqueue(again).map(|r| r.id), Some(PermissionId(1)));
        assert_eq!(p.next_for(Some(TAB)).map(|r| r.id), Some(PermissionId(3)));
        assert_eq!(p.navigated(TAB, "https://example.com/").len(), 1);
        assert_eq!(p.next_for(Some(TAB)), None);
        assert!(p.take(ViewId(2), PermissionId(2)).is_some());
    }

    #[test]
    fn capture_tells_the_camera_from_a_screen() {
        let mut p = SitePermissions::default();
        // Nothing granted (e.g. allowed before Axion knew): video counts as the camera.
        let c = p.capture_changed(TAB, true, true);
        assert_eq!(c, Capture { microphone: true, camera: true, screen: false });
        p.grant(TAB, &[PermissionKind::ScreenShare, PermissionKind::ScreenAudio]);
        let c = p.capture_changed(TAB, true, true);
        assert_eq!(c, Capture { microphone: false, camera: false, screen: true });
        p.grant(TAB, &[PermissionKind::Microphone, PermissionKind::Camera]);
        let c = p.capture(TAB);
        assert_eq!(c, Capture { microphone: true, camera: true, screen: true });
        assert!(!p.capture_changed(TAB, false, false).any());
        assert_eq!(c.kinds().len(), 3);
    }

    #[test]
    fn visit_choice_is_for_microphone_and_camera_only() {
        assert!(offers_visit(&[PermissionKind::Microphone, PermissionKind::Camera]));
        assert!(!offers_visit(&[PermissionKind::Notifications]));
        assert!(!offers_visit(&[PermissionKind::ScreenShare]));
        assert!(!rememberable(PermissionKind::ScreenAudio));
        assert!(rememberable(PermissionKind::Clipboard));
    }
}
