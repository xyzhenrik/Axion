//! The Now Playing card at the bottom of the sidebar: a compact music player. Artwork (what
//! the page offers, else the site's icon on a neutral tile), title and site or artist, a
//! progress bar with elapsed and remaining time that seeks when dragged, and previous /
//! play-pause / next. It rises out of the account button when something plays and sinks back
//! when it stops, on springs; while the account menu opens it is lifted out of the menu's way
//! (the card's glass is a pass of its own, so it never merges with the account button or the
//! menu). Clicking the artwork or title shows the tab.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_i18n::tr;
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::chrome::ChromeAction;
use crate::favicons::SiteIcon;

const PADDING: f32 = 12.0;
const ROW_GAP: f32 = 6.0;
const RADIUS: f32 = 22.0;
const ART_RADIUS: f32 = 11.0;
const SLIDER_HEIGHT: f32 = 18.0;
const BUTTON: f32 = 40.0;
const BUTTON_GAP: f32 = 22.0;
/// Pause at either end of a scrolling title (seconds).
const MARQUEE_PAUSE: f32 = 1.6;
/// The card counts as in or out below this distance of its spring from its target.
const SETTLED: f32 = 0.004;
/// After a seek, reports still carry the old time for a moment.
const SCRUB_SETTLE: std::time::Duration = std::time::Duration::from_millis(600);
/// The site icon on the artwork tile, relative to the tile.
const FAVICON_RATIO: f32 = 0.5;

/// What the card shows.
#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub subtitle: String,
    pub playing: bool,
    /// Seconds.
    pub time: f64,
    /// Seconds; 0 while unknown.
    pub duration: f64,
    pub live: bool,
    /// The page handles previous / next track.
    pub can_prev: bool,
    pub can_next: bool,
}

/// The picture on the artwork tile.
pub enum Art {
    /// The page's artwork, cover-fitted.
    Image(SiteIcon),
    /// The site's icon, on a neutral tile.
    Favicon(SiteIcon),
    /// Neither yet.
    None,
}

pub struct NowPlayingCard {
    holder: NodeId,
    root: NodeId,
    art: NodeId,
    favicon: NodeId,
    placeholder: NodeId,
    title_box: NodeId,
    title: NodeId,
    subtitle_box: NodeId,
    subtitle: NodeId,
    elapsed: NodeId,
    remaining: NodeId,
    scrubber: NodeId,
    next: NodeId,
    play_icon: NodeId,
    /// 0 hidden, 1 shown.
    present: Animated<f32>,
    /// Gap to the account button below (config), logical px.
    gap: f32,
    art_side: f32,
    marquee: bool,
    marquee_speed: f32,
    /// Seconds into the scroll of the title and the subtitle.
    scroll: [f32; 2],
    texts: [String; 2],
    scrubbed_at: Option<std::time::Instant>,
    duration: f64,
}

fn centered_row(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER,
        gap: Size { width: length(gap), height: length(0.0) },
        flex_shrink: 0.0,
        ..Default::default()
    }
}

fn square(side: f32) -> Size<Dimension> {
    Size { width: length(side), height: length(side) }
}

/// Height of the card for an artwork tile of `art` px.
fn card_height(art: f32) -> f32 {
    2.0 * PADDING + art + 2.0 * ROW_GAP + SLIDER_HEIGHT + BUTTON
}

/// The crop (uv) of a `size` picture that fills a square.
fn square_crop(size: (u32, u32)) -> Frame {
    let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
    let visible = Vec2::new((h / w).min(1.0), (w / h).min(1.0));
    Frame { origin: (Vec2::ONE - visible) / 2.0, size: visible }
}

fn time_label(ui: &mut Ui, parent: NodeId) -> NodeId {
    let label = ui.add_label(
        parent,
        "0:00",
        TextStyle::Caption,
        Style {
            flex_shrink: 0.0,
            min_size: Size { width: length(34.0), height: auto() },
            ..Default::default()
        },
    );
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
    label
}

fn control(ui: &mut Ui, parent: NodeId, action: ChromeAction, icon: Icon, size: f32) -> (NodeId, NodeId) {
    let button = ui.add(
        parent,
        Widget::container().with_action(action.id()).with_radius(CAPSULE),
        Style { size: square(BUTTON), justify_content: JustifyContent::CENTER, ..centered_row(0.0) },
    );
    let glyph = ui.add(button, Widget::icon(icon, size), Style::default());
    (button, glyph)
}

/// A line of text in a clipped box; the text scrolls when it doesn't fit (see `animate`).
fn scrolling_line(ui: &mut Ui, parent: NodeId, style: TextStyle, secondary: bool) -> (NodeId, NodeId) {
    let line = ui.add(
        parent,
        Widget::container().with_clip(),
        Style {
            flex_shrink: 0.0,
            overflow: axion_ui::taffy::Point {
                x: axion_ui::taffy::Overflow::Hidden,
                y: axion_ui::taffy::Overflow::Hidden,
            },
            ..Default::default()
        },
    );
    let text = ui.add_label(
        line,
        "",
        style,
        Style { flex_shrink: 0.0, position: Position::Relative, ..Default::default() },
    );
    if secondary && let Some(w) = ui.widget_mut(text) {
        w.set_paint(Paint::Secondary);
    }
    (line, text)
}

impl NowPlayingCard {
    pub fn build(ui: &mut Ui, footer: NodeId, config: &Config) -> Self {
        let media = &config.media;
        let art_side = media.now_playing_artwork_size.clamp(24.0, 120.0);
        let gap = media.now_playing_gap.max(0.0);
        let height = card_height(art_side);
        // The holder takes the card's room in the footer (growing as the card appears); the
        // card itself floats in it, anchored above the account button.
        let holder = ui.add(
            footer,
            Widget::container(),
            Style {
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::END,
                size: Size { width: auto(), height: length(0.0) },
                ..Default::default()
            },
        );
        let root = ui.add(
            holder,
            Widget::container().with_background(Background::Glass(GlassLevel::Regular)).with_radius(RADIUS),
            Style {
                position: Position::Relative,
                inset: Rect { left: auto(), right: auto(), top: auto(), bottom: length(0.0) },
                margin: Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(gap) },
                flex_shrink: 0.0,
                size: Size { width: auto(), height: length(height) },
                padding: Rect {
                    left: length(PADDING),
                    right: length(PADDING),
                    top: length(PADDING),
                    bottom: length(PADDING),
                },
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(ROW_GAP) },
                ..Default::default()
            },
        );
        // Artwork and titles: one button that shows the tab.
        let header = ui.add(
            root,
            Widget::container().with_action(ChromeAction::NowPlaying.id()).with_radius(ART_RADIUS + 2.0),
            Style { size: Size { width: auto(), height: length(art_side) }, ..centered_row(12.0) },
        );
        let tile = ui.add(
            header,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.1)).with_radius(ART_RADIUS),
            Style {
                size: square(art_side),
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                ..centered_row(0.0)
            },
        );
        let art = ui.add(
            tile,
            Widget::image().with_radius(ART_RADIUS),
            Style { size: square(art_side), flex_shrink: 0.0, ..Default::default() },
        );
        let favicon_side = (art_side * FAVICON_RATIO).round();
        let favicon = ui.add(
            tile,
            Widget::image().with_radius(favicon_side * 0.22),
            Style { size: square(favicon_side), flex_shrink: 0.0, ..Default::default() },
        );
        let placeholder = ui.add(
            tile,
            Widget::icon(Icon::Media, favicon_side).with_paint(Paint::Accent),
            Style { flex_shrink: 0.0, ..Default::default() },
        );
        ui.set_visible(art, false);
        ui.set_visible(favicon, false);
        let titles = ui.add(
            header,
            Widget::container(),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(2.0) },
                ..Default::default()
            },
        );
        let (title_box, title) = scrolling_line(ui, titles, TextStyle::Callout, false);
        let (subtitle_box, subtitle) = scrolling_line(ui, titles, TextStyle::Footnote, true);

        let progress = ui.add(
            root,
            Widget::container(),
            Style { size: Size { width: auto(), height: length(SLIDER_HEIGHT) }, ..centered_row(8.0) },
        );
        let elapsed = time_label(ui, progress);
        let scrubber = ui.add(
            progress,
            Widget::line_slider(0.0),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(20.0), height: auto() },
                size: Size { width: auto(), height: length(SLIDER_HEIGHT) },
                ..Default::default()
            },
        );
        let remaining = time_label(ui, progress);

        let controls = ui.add(
            root,
            Widget::container(),
            Style {
                size: Size { width: auto(), height: length(BUTTON) },
                justify_content: JustifyContent::CENTER,
                ..centered_row(BUTTON_GAP)
            },
        );
        control(ui, controls, ChromeAction::NowPlayingPrevious, Icon::Previous, 22.0);
        let (_, play_icon) = control(ui, controls, ChromeAction::NowPlayingToggle, Icon::Pause, 28.0);
        let (next, _) = control(ui, controls, ChromeAction::NowPlayingNext, Icon::Next, 22.0);
        ui.set_visible(holder, false);
        Self {
            holder,
            root,
            art,
            favicon,
            placeholder,
            title_box,
            title,
            subtitle_box,
            subtitle,
            elapsed,
            remaining,
            scrubber,
            next,
            play_icon,
            present: Animated::new(0.0, Spring::SMOOTH),
            gap,
            art_side,
            marquee: media.now_playing_marquee,
            marquee_speed: media.now_playing_marquee_speed.max(1.0),
            scroll: [0.0; 2],
            texts: [String::new(), String::new()],
            scrubbed_at: None,
            duration: 0.0,
        }
    }

    pub fn scrubber(&self) -> NodeId {
        self.scrubber
    }

    /// The slider moved: the second to seek to.
    pub fn seek_to(&mut self, value: f32) -> Option<f64> {
        if self.duration <= 0.0 {
            return None;
        }
        self.scrubbed_at = Some(std::time::Instant::now());
        Some(f64::from(value) * self.duration)
    }

    /// Shows what plays (`None` hides the card).
    pub fn set(&mut self, ui: &mut Ui, config: &Config, now: Option<&NowPlaying>) {
        let media = &config.media;
        let gap = media.now_playing_gap.max(0.0);
        if gap != self.gap {
            self.gap = gap;
            let mut style = ui.style(self.root);
            style.margin.bottom = length(gap);
            ui.set_style(self.root, style);
        }
        self.marquee = media.now_playing_marquee;
        self.marquee_speed = media.now_playing_marquee_speed.max(1.0);
        self.present.spring = crate::drip::spring(config, config.motion.smooth);
        let wanted = if now.is_some() { 1.0 } else { 0.0 };
        if self.present.target != wanted {
            self.present.target = wanted;
            if wanted > 0.5 {
                ui.set_visible(self.holder, true);
            }
        }
        let Some(now) = now else { return };
        for (node, i, text) in [(self.title, 0, &now.title), (self.subtitle, 1, &now.subtitle)] {
            if self.texts[i] != *text {
                self.texts[i].clone_from(text);
                self.scroll[i] = 0.0;
                ui.set_text(node, text);
            }
        }
        ui.set_icon(self.play_icon, if now.playing { Icon::Pause } else { Icon::Play });
        self.duration = if now.live { 0.0 } else { now.duration };
        let scrubbing = ui.dragging() == Some(self.scrubber)
            || self.scrubbed_at.is_some_and(|t| t.elapsed() < SCRUB_SETTLE);
        if self.duration > 0.0 {
            if !scrubbing {
                ui.set_value(self.scrubber, (now.time / self.duration).clamp(0.0, 1.0) as f32);
                ui.set_text(self.elapsed, &axion_media::format_time(now.time));
                ui.set_text(
                    self.remaining,
                    &format!("\u{2212}{}", axion_media::format_time(self.duration - now.time)),
                );
            }
        } else {
            ui.set_value(self.scrubber, 0.0);
            ui.set_text(self.elapsed, &axion_media::format_time(now.time));
            ui.set_text(self.remaining, if now.live { tr("Live") } else { "\u{2212}:\u{2212}\u{2212}" });
        }
        let enabled = |ui: &mut Ui, node: NodeId, on: bool| {
            if let Some(w) = ui.widget_mut(node) {
                w.enabled = on;
            }
        };
        enabled(ui, self.scrubber, self.duration > 0.0);
        // Without a track handler, previous restarts the track and next has nowhere to go.
        enabled(ui, self.next, now.can_next);
    }

    /// Puts the picture on the artwork tile.
    pub fn set_art(&mut self, ui: &mut Ui, art: Art) {
        let (image, favicon, placeholder) = match art {
            Art::Image(icon) => {
                ui.set_image_crop(self.art, Some(square_crop(icon.1)));
                ui.set_image(self.art, Some(icon));
                (true, false, false)
            }
            Art::Favicon(icon) => {
                ui.set_image(self.favicon, Some(icon));
                (false, true, false)
            }
            Art::None => (false, false, true),
        };
        ui.set_visible(self.art, image);
        ui.set_visible(self.favicon, favicon);
        ui.set_visible(self.placeholder, placeholder);
    }

    /// Lifts the card by `lift` px (the account menu is opening below it).
    pub fn set_lift(&self, ui: &mut Ui, lift: f32) {
        let mut style = ui.style(self.root);
        let bottom = length(lift.max(0.0));
        if style.inset.bottom != bottom {
            style.inset.bottom = bottom;
            ui.set_style(self.root, style);
        }
    }

    /// Advances the spring that brings the card in and out, and the scrolling titles.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        if !self.present.is_settled(SETTLED) {
            self.present.step(dt);
        }
        let k = self.present.value.clamp(0.0, 1.0);
        let room = (card_height(self.art_side) + self.gap) * k;
        let mut style = ui.style(self.holder);
        if style.size.height != length(room) {
            style.size.height = length(room);
            ui.set_style(self.holder, style);
        }
        ui.set_opacity(self.root, k);
        if self.present.target < 0.5 && self.present.is_settled(SETTLED) {
            ui.set_visible(self.holder, false);
            return;
        }
        for (i, (line, text)) in
            [(self.title_box, self.title), (self.subtitle_box, self.subtitle)].into_iter().enumerate()
        {
            let overflow = (ui.frame(text).size.x - ui.frame(line).size.x).max(0.0);
            let offset = if self.marquee && overflow > 0.5 {
                self.scroll[i] += dt;
                let travel = overflow / self.marquee_speed;
                let t = self.scroll[i] % (2.0 * (travel + MARQUEE_PAUSE));
                // Pause, scroll to the end, pause, scroll back.
                let run = |t: f32| ((t - MARQUEE_PAUSE) / travel).clamp(0.0, 1.0);
                let f =
                    if t < travel + MARQUEE_PAUSE { run(t) } else { 1.0 - run(t - travel - MARQUEE_PAUSE) };
                f * overflow
            } else {
                0.0
            };
            let mut style = ui.style(text);
            let wanted = length(-offset.round());
            if style.inset.left != wanted {
                style.inset.left = wanted;
                ui.set_style(text, style);
            }
        }
    }
}

/// Largest side (px) the artwork is kept at.
const ART_FETCH_SIDE: u32 = 256;
const ART_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

struct Fetched {
    url: String,
    image: Option<image::RgbaImage>,
}

/// The artwork the page offers, fetched in the background (one picture at a time: the one of
/// what plays now).
#[derive(Default)]
pub struct ArtStore {
    wanted: Option<String>,
    image: Option<SiteIcon>,
    failed: std::collections::HashSet<String>,
    incoming: Option<std::sync::mpsc::Receiver<Fetched>>,
}

impl ArtStore {
    /// The artwork of what plays now (`None`: none to show; the site's icon is used).
    pub fn want(&mut self, url: Option<&str>) {
        if self.wanted.as_deref() == url {
            return;
        }
        self.wanted = url.map(str::to_owned);
        self.image = None;
        self.incoming = None;
        let Some(url) = url.filter(|u| !self.failed.contains(*u)) else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        let url = url.to_owned();
        let spawned = std::thread::Builder::new().name("now-playing-art".into()).spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(ART_TIMEOUT))
                .user_agent(concat!("Axion/", env!("CARGO_PKG_VERSION")))
                .build()
                .into();
            let image = agent
                .get(&url)
                .call()
                .ok()
                .and_then(|mut r| r.body_mut().read_to_vec().ok())
                .and_then(|bytes| image::load_from_memory(&bytes).ok())
                .map(|i| i.thumbnail(ART_FETCH_SIDE, ART_FETCH_SIDE).to_rgba8());
            let _ = tx.send(Fetched { url, image });
        });
        match spawned {
            Ok(_) => self.incoming = Some(rx),
            Err(e) => tracing::warn!("fetching artwork failed: {e}"),
        }
    }

    /// Takes what arrived; whether the picture changed.
    pub fn poll(&mut self, favicons: &crate::favicons::Favicons) -> bool {
        let Some(rx) = &self.incoming else { return false };
        let Ok(done) = rx.try_recv() else { return false };
        self.incoming = None;
        match done.image {
            Some(image) if self.wanted.as_deref() == Some(done.url.as_str()) => {
                let (w, h) = image.dimensions();
                self.image = Some(favicons.upload(w, h, image.as_raw()));
                true
            }
            Some(_) => false,
            None => {
                tracing::debug!("no artwork at {}", done.url);
                self.failed.insert(done.url);
                false
            }
        }
    }

    pub fn image(&self) -> Option<SiteIcon> {
        self.image.clone()
    }
}

/// The first address (http or https) of `candidates`.
pub fn first_web_url<'a>(candidates: &[&'a str]) -> Option<&'a str> {
    candidates.iter().copied().find(|u| u.starts_with("https://") || u.starts_with("http://"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_crop_centres_the_longer_side() {
        let wide = square_crop((200, 100));
        assert_eq!((wide.origin, wide.size), (Vec2::new(0.25, 0.0), Vec2::new(0.5, 1.0)));
        let tall = square_crop((100, 400));
        assert_eq!((tall.origin, tall.size), (Vec2::new(0.0, 0.375), Vec2::new(1.0, 0.25)));
        assert_eq!(square_crop((64, 64)).size, Vec2::ONE);
    }

    #[test]
    fn artwork_must_be_a_web_address() {
        assert_eq!(first_web_url(&["", "blob:x", "https://a/b.jpg", "http://c"]), Some("https://a/b.jpg"));
        assert_eq!(first_web_url(&["data:image/png;base64,AA", ""]), None);
    }
}
