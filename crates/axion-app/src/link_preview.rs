//! The link bubble: a small glass pill in a bottom corner of the page that says where the link
//! under the pointer (or the keyboard focus) leads, like Chrome's and Safari's status bar.
//!
//! The engine reports the address ([`axion_engine::EngineEvent::LinkHover`]); this module
//! decides when it shows ([`Visibility`]: a short delay in, a shorter one out so moving between
//! neighbouring links does not flicker), what it shows ([`split_url`] and [`fit`]: the host
//! stands out, the scheme and path are dim, a long address loses the middle of its path), and
//! where it sits ([`choose_side`]: bottom left, or bottom right when the pointer comes near
//! the left one or something else floats there). After `link_preview.expand_after_ms` it
//! grows to show the whole address on two lines. The pointer passes through it.
//!
//! The address is shown exactly as the engine reports it (canonical, so internationalised
//! host names are punycode and lookalike tricks stay visible); the host after any `user@` is
//! the emphasised part, so `https://google.com@evil.com/` reads as evil.com.

use axion_compositor::Rect as Frame;
use axion_config::Config;
use axion_engine::ViewId;
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{Background, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

const RADIUS: f32 = 16.0;
const PADDING_X: f32 = 14.0;
const PADDING_Y: f32 = 6.0;
/// Kept free at the page's edges at least, whatever the page margin.
const MIN_INSET: f32 = 8.0;
/// Rough advance of one character (logical px at text scale 1); a little generous so a
/// shortened address always fits its bubble.
const CHAR_WIDTH: f32 = 7.4;
/// The bubble grows from this scale to full size as it appears.
const MIN_SCALE: f32 = 0.9;
/// Below this opacity (and heading away) the bubble is taken out of the layout.
const HIDDEN_BELOW: f32 = 0.01;
/// The least of a path worth showing next to the host before the rest is dropped to "…".
const MIN_REST: usize = 6;
/// Addresses longer than this are cut in the middle before anything else (data: URLs).
const MAX_URL_CHARS: usize = 4096;
/// What a `data:` URL shows of its payload.
const DATA_PREVIEW_CHARS: usize = 40;
/// The scheme and user name are drawn this strong, the path a little stronger.
const DIM_OPACITY: f32 = 0.6;
const REST_OPACITY: f32 = 0.82;
const ELLIPSIS: char = '\u{2026}';

/// An address taken apart for display.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UrlParts {
    /// `https://`, `mailto:` … (dim).
    pub prefix: String,
    /// `user:pw@` (dim): never mistaken for the host.
    pub userinfo: String,
    /// The host with its port (an IPv6 literal keeps its brackets); the emphasised part.
    pub host: String,
    /// Path, query and fragment (secondary).
    pub rest: String,
}

/// Splits `url` into scheme prefix, userinfo, host and the rest. Without a host (`data:`,
/// `javascript:`) the payload is the rest; for `mailto:` and `tel:` the address is the host.
pub fn split_url(url: &str) -> UrlParts {
    let url = url.trim();
    let scheme_end = url.find(':').filter(|&i| {
        i > 0
            && url[..i].starts_with(|c: char| c.is_ascii_alphabetic())
            && url[..i].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    let Some(colon) = scheme_end else {
        return UrlParts { host: url.to_owned(), ..Default::default() };
    };
    let scheme = &url[..colon];
    let after = &url[colon + 1..];
    if let Some(authority_and_rest) = after.strip_prefix("//") {
        let end = authority_and_rest.find(['/', '?', '#', '\\']).unwrap_or(authority_and_rest.len());
        let (authority, rest) = authority_and_rest.split_at(end);
        // The host starts after the last `@` of the authority: everything before it is
        // userinfo, however much it looks like a host.
        let (userinfo, host) = match authority.rfind('@') {
            Some(at) => authority.split_at(at + 1),
            None => ("", authority),
        };
        return UrlParts {
            prefix: format!("{scheme}://"),
            userinfo: userinfo.to_owned(),
            host: host.to_owned(),
            rest: rest.to_owned(),
        };
    }
    let lower = scheme.to_ascii_lowercase();
    match lower.as_str() {
        "mailto" | "tel" | "sms" => {
            let end = after.find(['?', '#']).unwrap_or(after.len());
            UrlParts {
                prefix: format!("{scheme}:"),
                host: after[..end].to_owned(),
                rest: after[end..].to_owned(),
                ..Default::default()
            }
        }
        // `blob:https://example.com/uuid`: the origin inside is what matters.
        "blob" if after.contains("://") => {
            let mut inner = split_url(after);
            inner.prefix = format!("{scheme}:{}", inner.prefix);
            inner
        }
        "data" => UrlParts {
            prefix: format!("{scheme}:"),
            rest: truncate_end(after, DATA_PREVIEW_CHARS),
            ..Default::default()
        },
        _ => UrlParts { prefix: format!("{scheme}:"), rest: after.to_owned(), ..Default::default() },
    }
}

/// Whether the bubble shows nothing for `url` (a `javascript:` link, compared without case or
/// leading white space).
pub fn is_javascript(url: &str) -> bool {
    let url = url.trim_start();
    url.get(..11).is_some_and(|s| s.eq_ignore_ascii_case("javascript:"))
}

/// `text` cut to `max` characters with the middle replaced by "…" (both ends stay readable).
pub fn middle_ellipsis(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return ELLIPSIS.to_string();
    }
    let keep = max - 1;
    // The start gets the extra character: it names the page, the end only its variant.
    let tail = keep / 2;
    let head = keep - tail;
    let mut out: String = text.chars().take(head).collect();
    out.push(ELLIPSIS);
    out.extend(text.chars().skip(count - tail));
    out
}

/// `text` cut to `max` characters, ending in "…".
pub fn truncate_end(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push(ELLIPSIS);
    out
}

/// What the bubble's labels show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lines {
    pub prefix: String,
    pub userinfo: String,
    pub host: String,
    /// The rest of the address after the host, on the first line.
    pub rest1: String,
    /// The second line (only when `second` was asked for in [`fit`]).
    pub rest2: String,
}

impl Lines {
    pub fn second_line(&self) -> bool {
        !self.rest2.is_empty()
    }
}

/// Fits `parts` into `first` characters on the first line and, if `second > 0`, `second` on a
/// second one. One line: the middle of a long path goes. Two lines: the path continues on the
/// second, and only an address longer than both loses its middle. The host always stays; a
/// user name or host that alone fills the line is shortened too (user name first).
pub fn fit(parts: &UrlParts, first: usize, second: usize) -> Lines {
    let count = |s: &str| s.chars().count();
    let mut lines = Lines {
        prefix: parts.prefix.clone(),
        userinfo: parts.userinfo.clone(),
        host: parts.host.clone(),
        ..Default::default()
    };
    let mut head = count(&lines.prefix) + count(&lines.userinfo) + count(&lines.host);
    if head + MIN_REST > first && !parts.rest.is_empty() || head > first {
        // Make room for a little path: the user name gives way first, then the host.
        let room = first.saturating_sub(MIN_REST.min(count(&parts.rest)));
        let mut over = head.saturating_sub(room);
        if over > 0 && !lines.userinfo.is_empty() {
            let keep = count(&lines.userinfo).saturating_sub(over).max(2);
            let name = lines.userinfo.trim_end_matches('@');
            lines.userinfo = format!("{}@", middle_ellipsis(name, keep - 1));
            head = count(&lines.prefix) + count(&lines.userinfo) + count(&lines.host);
            over = head.saturating_sub(room);
        }
        if over > 0 {
            let keep = count(&lines.host).saturating_sub(over).max(4);
            lines.host = middle_ellipsis(&lines.host, keep);
            head = count(&lines.prefix) + count(&lines.userinfo) + count(&lines.host);
        }
    }
    let budget1 = first.saturating_sub(head);
    let rest = &parts.rest;
    if second == 0 || count(rest) <= budget1 {
        lines.rest1 = middle_ellipsis(rest, budget1.max(1).min(count(rest)));
        return lines;
    }
    lines.rest1 = rest.chars().take(budget1).collect();
    let remaining: String = rest.chars().skip(budget1).collect();
    lines.rest2 = middle_ellipsis(&remaining, second);
    lines
}

/// Which bottom corner the bubble sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    fn other(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// The bubble's frame (`size`) in the `side` bottom corner of `page`, `inset` from its edges.
pub fn frame_at(side: Side, page: Frame, size: Vec2, inset: f32) -> Frame {
    let x = match side {
        Side::Left => page.origin.x + inset,
        Side::Right => page.origin.x + page.size.x - inset - size.x,
    };
    Frame { origin: Vec2::new(x, page.origin.y + page.size.y - inset - size.y), size }
}

/// Whether `p` is within `clearance` of `frame`.
fn near(frame: Frame, p: Vec2, clearance: f32) -> bool {
    let grown = Frame {
        origin: frame.origin - Vec2::splat(clearance),
        size: frame.size + Vec2::splat(2.0 * clearance),
    };
    grown.contains(p)
}

fn overlaps(a: Frame, b: Frame) -> bool {
    a.origin.x < b.origin.x + b.size.x
        && a.origin.x + a.size.x > b.origin.x
        && a.origin.y < b.origin.y + b.size.y
        && a.origin.y + a.size.y > b.origin.y
}

/// The corner for the bubble. It stays where it is (`current`) as long as that corner is clear;
/// it moves to the other one when the pointer comes within `clearance` of it (so it never
/// covers what the user points at) or something in `occupied` (a toast, a popover) floats
/// there. Where neither corner is clear it keeps clear of the pointer first.
pub fn choose_side(
    current: Side,
    page: Frame,
    size: Vec2,
    inset: f32,
    pointer: Option<Vec2>,
    clearance: f32,
    occupied: &[Frame],
) -> Side {
    let pointer_clear =
        |side: Side| pointer.is_none_or(|p| !near(frame_at(side, page, size, inset), p, clearance));
    let free = |side: Side| {
        let frame = frame_at(side, page, size, inset);
        !occupied.iter().any(|&o| overlaps(frame, o))
    };
    [current, current.other()]
        .into_iter()
        .find(|&s| pointer_clear(s) && free(s))
        .or_else(|| [current, current.other()].into_iter().find(|&s| pointer_clear(s)))
        .unwrap_or(current)
}

/// When the bubble is up: it appears once a link has been under the pointer for `show` seconds,
/// stays `hide` seconds after the pointer left, and counts how long it has shown one address.
#[derive(Clone, Copy, Debug, Default)]
pub struct Visibility {
    visible: bool,
    /// Seconds a link has been waiting to be shown / the pointer has been off it.
    waited: f32,
    /// Seconds the current address has been up.
    pub up_for: f32,
}

impl Visibility {
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// A different address: the wait for the expanded form starts over.
    pub fn restart_expansion(&mut self) {
        self.up_for = 0.0;
    }

    pub fn hide_now(&mut self) {
        *self = Self::default();
    }

    /// Advances by `dt` with `wanted` (a link is hovered); returns whether it is visible.
    pub fn update(&mut self, wanted: bool, dt: f32, show: f32, hide: f32) -> bool {
        if wanted {
            if self.visible {
                self.waited = 0.0;
                self.up_for += dt;
            } else {
                self.waited += dt;
                if self.waited >= show {
                    self.visible = true;
                    self.waited = 0.0;
                    self.up_for = 0.0;
                }
            }
        } else if self.visible {
            self.waited += dt;
            if self.waited >= hide {
                self.hide_now();
            }
        } else {
            self.waited = 0.0;
        }
        self.visible
    }
}

/// What the bubble is asked to show.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Wanted {
    view: ViewId,
    url: String,
}

pub struct LinkPreview {
    plate: NodeId,
    second_line: NodeId,
    prefix: NodeId,
    userinfo: NodeId,
    host: NodeId,
    rest1: NodeId,
    rest2: NodeId,
    /// The address the engine reported (kept while the pointer lingers off it).
    wanted: Option<Wanted>,
    /// The address on show, and the text set for it (to touch the labels only on a change).
    shown: Option<Wanted>,
    laid_out: Option<(Lines, bool)>,
    visibility: Visibility,
    /// 0: hidden, 1: shown.
    presence: Animated<f32>,
    /// The bubble's left edge as drawn (logical px): slides between the corners.
    slide: Animated<f32>,
    side: Side,
    /// The slide has a position (it snaps to the first one instead of flying in).
    slide_ready: bool,
    /// Where the layout puts the bubble now (to relayout only when it moved).
    placed: Option<Vec2>,
}

impl LinkPreview {
    /// `parent` is the root, so the bubble lies above the page and the chrome built before it.
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
        let plate = ui.add(
            parent,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Regular))
                // Tinted as densely as thick glass: legible on a white page too.
                .with_glass_density(GlassLevel::Thick)
                .with_radius(RADIUS),
            Style {
                position: Position::Absolute,
                size: Size { width: auto(), height: auto() },
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::CENTER,
                padding: Rect {
                    left: length(PADDING_X),
                    right: length(PADDING_X),
                    top: length(PADDING_Y),
                    bottom: length(PADDING_Y),
                },
                ..Default::default()
            },
        );
        let row = Style {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::CENTER,
            flex_shrink: 0.0,
            ..Default::default()
        };
        let first_line = ui.add(plate, Widget::container(), row.clone());
        let second_line = ui.add(plate, Widget::container(), row);
        let label = |ui: &mut Ui, parent: NodeId, style: TextStyle, paint: Paint| {
            let node = ui.add_label(parent, "", style, Style { flex_shrink: 0.0, ..Default::default() });
            if let Some(w) = ui.widget_mut(node) {
                w.set_paint(paint);
            }
            node
        };
        let prefix = label(ui, first_line, TextStyle::Footnote, Paint::Label);
        let userinfo = label(ui, first_line, TextStyle::Footnote, Paint::Label);
        let host = label(ui, first_line, TextStyle::Callout, Paint::Label);
        let rest1 = label(ui, first_line, TextStyle::Footnote, Paint::Label);
        let rest2 = label(ui, second_line, TextStyle::Footnote, Paint::Label);
        // Dimmer than the host, but still legible on any backdrop: the same label colour at
        // less strength rather than the (fainter) secondary colour.
        ui.set_opacity(prefix, DIM_OPACITY);
        ui.set_opacity(userinfo, DIM_OPACITY);
        ui.set_opacity(rest1, REST_OPACITY);
        ui.set_opacity(rest2, REST_OPACITY);
        ui.set_passthrough(plate, true);
        ui.claim_space(plate);
        ui.set_visible(second_line, false);
        ui.set_visible(plate, false);
        ui.set_opacity(plate, 0.0);
        Self {
            plate,
            second_line,
            prefix,
            userinfo,
            host,
            rest1,
            rest2,
            wanted: None,
            shown: None,
            laid_out: None,
            visibility: Visibility::default(),
            presence: Animated::new(0.0, Spring::SNAPPY),
            slide: Animated::new(0.0, Spring::SNAPPY),
            side: Side::Left,
            slide_ready: false,
            placed: None,
        }
    }

    /// The engine reported the link under the pointer (or none) in `view`.
    pub fn set_hover(&mut self, config: &Config, view: ViewId, url: Option<String>) {
        let url = url.filter(|u| {
            config.link_preview.enabled && !(config.link_preview.hide_javascript_urls && is_javascript(u))
        });
        self.wanted = url.map(|url| Wanted { view, url: middle_ellipsis(&url, MAX_URL_CHARS) });
    }

    /// Whether it shows the whole address on two lines now.
    pub fn is_expanded(&self, config: &Config) -> bool {
        self.visibility.is_visible()
            && self.visibility.up_for >= config.link_preview.expand_after_ms as f32 / 1000.0
    }

    fn hide_now(&mut self, ui: &mut Ui) {
        self.visibility.hide_now();
        self.shown = None;
        self.laid_out = None;
        self.placed = None;
        self.slide_ready = false;
        self.presence.snap(0.0);
        ui.set_opacity(self.plate, 0.0);
        ui.set_visual_frame(self.plate, None);
        ui.set_visible(self.plate, false);
    }

    /// Runs the timers and springs the bubble; call every frame. `page` is the page card,
    /// `pointer` where the pointer is, `active` the shown tab, `blocked` whether a menu,
    /// popover or sheet has the pointer.
    #[allow(clippy::too_many_arguments)]
    pub fn animate(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        page: Frame,
        pointer: Option<Vec2>,
        active: Option<ViewId>,
        blocked: bool,
        dt: f32,
    ) {
        let cfg = &config.link_preview;
        if !cfg.enabled || blocked || self.wanted.as_ref().is_some_and(|w| Some(w.view) != active) {
            self.wanted = None;
            if self.shown.is_some() {
                self.hide_now(ui);
            }
            return;
        }
        let wanted = self.wanted.is_some();
        let was_visible = self.visibility.is_visible();
        let visible = self.visibility.update(
            wanted,
            dt,
            cfg.show_delay_ms as f32 / 1000.0,
            cfg.hide_delay_ms as f32 / 1000.0,
        );
        if let Some(w) = &self.wanted
            && visible
            && self.shown.as_ref() != Some(w)
        {
            // A new address (the pointer moved to another link): in place, no new fade.
            if self.shown.is_some() {
                self.visibility.restart_expansion();
            }
            self.shown = Some(w.clone());
            self.laid_out = None;
        }
        if !visible && self.shown.is_none() {
            return;
        }
        if visible && !was_visible {
            ui.set_visible(self.plate, true);
            self.presence.snap(0.0);
            self.placed = None;
            self.slide_ready = false;
        }
        self.presence.target = if visible { 1.0 } else { 0.0 };
        self.presence.spring = crate::drip::spring(config, config.motion.snappy);
        self.slide.spring = crate::drip::spring(config, config.motion.smooth);
        self.presence.step(dt);
        self.fill(ui, config, page);
        self.place(ui, config, page, pointer, visible && !was_visible, dt);
        if self.presence.value < HIDDEN_BELOW && !visible {
            self.hide_now(ui);
        }
    }

    /// Sets the labels for the address on show, as wide as the page allows.
    fn fill(&mut self, ui: &mut Ui, config: &Config, page: Frame) {
        let Some(shown) = self.shown.clone() else { return };
        let expanded = self.is_expanded(config);
        let scale = config.appearance.text_scale.max(0.5);
        let inset = config.effective_layout().page_margin.max(MIN_INSET);
        let chars = |width: f32| ((width - 2.0 * PADDING_X) / (CHAR_WIDTH * scale)).floor().max(4.0) as usize;
        let short = chars(page.size.x * config.link_preview.max_width_fraction.clamp(0.1, 1.0));
        let full = chars(page.size.x - 2.0 * inset);
        let parts = split_url(&shown.url);
        let lines = if expanded {
            let two = fit(&parts, full, full);
            // Whole address fits the short bubble: nothing to expand.
            if two.second_line() { two } else { fit(&parts, short, 0) }
        } else {
            fit(&parts, short, 0)
        };
        let key = (lines, expanded);
        if self.laid_out.as_ref() == Some(&key) {
            return;
        }
        let (lines, _) = &key;
        ui.set_text(self.prefix, &lines.prefix);
        ui.set_text(self.userinfo, &lines.userinfo);
        ui.set_text(self.host, &lines.host);
        ui.set_text(self.rest1, &lines.rest1);
        ui.set_text(self.rest2, &lines.rest2);
        ui.set_visible(self.userinfo, !lines.userinfo.is_empty());
        ui.set_visible(self.second_line, lines.second_line());
        let mut style = ui.style(self.plate);
        style.max_size.width = length(page.size.x - 2.0 * inset);
        ui.set_style(self.plate, style);
        self.laid_out = Some(key);
        // Content (and so size) changed: the next layout decides the position.
        self.placed = None;
    }

    /// Puts the bubble in a bottom corner of the page and draws it at its animated place.
    fn place(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        page: Frame,
        pointer: Option<Vec2>,
        first: bool,
        dt: f32,
    ) {
        let size = ui.frame(self.plate).size;
        if size.x < 1.0 {
            // Not laid out yet.
            ui.set_opacity(self.plate, 0.0);
            return;
        }
        let inset = config.effective_layout().page_margin.max(MIN_INSET);
        let occupied = ui.claimed_areas(&[self.plate]);
        let side = choose_side(
            self.side,
            page,
            size,
            inset,
            pointer,
            config.link_preview.pointer_clearance.max(0.0),
            &occupied,
        );
        let target = frame_at(side, page, size, inset);
        self.side = side;
        if first || !self.slide_ready {
            self.slide.snap(target.origin.x);
            self.slide_ready = true;
        }
        self.slide.target = target.origin.x;
        self.slide.step(dt);
        let at = target.origin;
        if self.placed != Some(at) {
            self.placed = Some(at);
            let mut style = ui.style(self.plate);
            style.inset = Rect { left: length(at.x), right: auto(), top: length(at.y), bottom: auto() };
            ui.set_style(self.plate, style);
        }
        let t = self.presence.value;
        ui.set_opacity(self.plate, t.clamp(0.0, 1.0));
        // Grows out of its corner; Reduce Motion only fades.
        let grow = if config.glass.reduce_motion { 1.0 } else { MIN_SCALE + (1.0 - MIN_SCALE) * t.max(0.0) };
        let (w, h) = (size.x * grow, size.y * grow);
        let anchor_x = match side {
            Side::Left => self.slide.value,
            Side::Right => self.slide.value + size.x - w,
        };
        ui.set_visual_frame(self.plate, Some(Frame::new(anchor_x, at.y + size.y - h, w, h)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(url: &str) -> (String, String, String, String) {
        let p = split_url(url);
        (p.prefix, p.userinfo, p.host, p.rest)
    }

    fn s(a: &str, b: &str, c: &str, d: &str) -> (String, String, String, String) {
        (a.into(), b.into(), c.into(), d.into())
    }

    #[test]
    fn splits_scheme_host_and_rest() {
        assert_eq!(
            parts("https://example.com/a/b?x=1#top"),
            s("https://", "", "example.com", "/a/b?x=1#top")
        );
        assert_eq!(parts("http://example.com"), s("http://", "", "example.com", ""));
        assert_eq!(parts("https://example.com:8080/x"), s("https://", "", "example.com:8080", "/x"));
        assert_eq!(parts("https://example.com?q=1"), s("https://", "", "example.com", "?q=1"));
    }

    #[test]
    fn ipv6_hosts_keep_their_brackets_and_port() {
        assert_eq!(parts("http://[::1]:3000/path"), s("http://", "", "[::1]:3000", "/path"));
        assert_eq!(parts("http://[2001:db8::1]/"), s("http://", "", "[2001:db8::1]", "/"));
    }

    #[test]
    fn the_host_after_userinfo_is_the_emphasised_one() {
        assert_eq!(parts("https://google.com@evil.com/"), s("https://", "google.com@", "evil.com", "/"));
        assert_eq!(
            parts("https://user:pw@evil.com:81/x@y"),
            s("https://", "user:pw@", "evil.com:81", "/x@y")
        );
        // An `@` further on (path, query) is not userinfo.
        assert_eq!(parts("https://evil.com/google.com@x"), s("https://", "", "evil.com", "/google.com@x"));
        // The last `@` of the authority counts.
        assert_eq!(parts("https://a@b@evil.com/"), s("https://", "a@b@", "evil.com", "/"));
    }

    #[test]
    fn urls_without_a_host() {
        assert_eq!(
            parts("mailto:me@example.com?subject=Hi"),
            s("mailto:", "", "me@example.com", "?subject=Hi")
        );
        assert_eq!(parts("tel:+4912345"), s("tel:", "", "+4912345", ""));
        assert_eq!(
            parts("blob:https://example.com/0b1c-22"),
            s("blob:https://", "", "example.com", "/0b1c-22")
        );
        assert_eq!(parts("about:blank"), s("about:", "", "", "blank"));
        assert_eq!(parts("not a url"), s("", "", "not a url", ""));
    }

    #[test]
    fn data_urls_show_only_the_start_of_their_payload() {
        let p = split_url(&format!("data:text/html;base64,{}", "A".repeat(5000)));
        assert_eq!(p.prefix, "data:");
        assert_eq!(p.rest.chars().count(), DATA_PREVIEW_CHARS);
        assert!(p.rest.ends_with(ELLIPSIS));
        assert!(p.rest.starts_with("text/html"));
    }

    #[test]
    fn javascript_links_are_recognised() {
        assert!(is_javascript("javascript:void(0)"));
        assert!(is_javascript("  JavaScript:alert(1)"));
        assert!(!is_javascript("https://example.com/javascript:x"));
        assert!(!is_javascript("java"));
        assert!(!is_javascript("javascript"));
    }

    #[test]
    fn javascript_is_dropped_by_set_hover_unless_allowed() {
        let mut config = Config::default();
        let mut ui = Ui::new(axion_ui::Theme::new(&config, true), 1.0);
        let root = ui.root();
        let mut bubble = LinkPreview::build(&mut ui, root);
        bubble.set_hover(&config, ViewId(1), Some("javascript:void(0)".into()));
        assert!(bubble.wanted.is_none());
        bubble.set_hover(&config, ViewId(1), Some("https://example.com/".into()));
        assert!(bubble.wanted.is_some());
        bubble.set_hover(&config, ViewId(1), None);
        assert!(bubble.wanted.is_none());
        config.link_preview.hide_javascript_urls = false;
        bubble.set_hover(&config, ViewId(1), Some("javascript:void(0)".into()));
        assert!(bubble.wanted.is_some());
        config.link_preview.enabled = false;
        bubble.set_hover(&config, ViewId(1), Some("https://example.com/".into()));
        assert!(bubble.wanted.is_none());
    }

    #[test]
    fn middle_ellipsis_keeps_both_ends() {
        assert_eq!(middle_ellipsis("short", 10), "short");
        assert_eq!(middle_ellipsis("exactly10!", 10), "exactly10!");
        let cut = middle_ellipsis("/a/very/long/path/to/file.html", 15);
        assert_eq!(cut.chars().count(), 15);
        assert!(cut.starts_with("/a/very"));
        assert!(cut.ends_with(".html"));
        assert!(cut.contains(ELLIPSIS));
        assert_eq!(middle_ellipsis("abcdef", 1), "\u{2026}");
        assert_eq!(middle_ellipsis("abcdef", 0), "");
        // Characters, not bytes.
        assert_eq!(middle_ellipsis("äöüäöüäöü", 5).chars().count(), 5);
    }

    #[test]
    fn truncate_end_ends_in_an_ellipsis() {
        assert_eq!(truncate_end("abcdef", 4), "abc\u{2026}");
        assert_eq!(truncate_end("abc", 4), "abc");
    }

    #[test]
    fn a_short_address_is_shown_whole() {
        let l = fit(&split_url("https://example.com/a?x=1"), 60, 0);
        assert_eq!((l.host.as_str(), l.rest1.as_str()), ("example.com", "/a?x=1"));
        assert!(!l.second_line());
    }

    #[test]
    fn a_long_path_loses_its_middle_but_the_host_stays() {
        let url = format!("https://example.com/{}/end.html", "segment/".repeat(30));
        let l = fit(&split_url(&url), 50, 0);
        assert_eq!(l.host, "example.com");
        assert_eq!(l.prefix, "https://");
        assert!(l.rest1.contains(ELLIPSIS));
        assert!(l.rest1.ends_with("end.html"));
        let total = l.prefix.chars().count() + l.host.chars().count() + l.rest1.chars().count();
        assert_eq!(total, 50);
    }

    #[test]
    fn a_long_user_name_gives_way_before_the_host() {
        let url = format!("https://{}@evil.com/login", "google.com.".repeat(8));
        let l = fit(&split_url(&url), 40, 0);
        assert_eq!(l.host, "evil.com");
        assert!(l.userinfo.ends_with('@'));
        assert!(l.userinfo.contains(ELLIPSIS));
        let total = l.prefix.chars().count()
            + l.userinfo.chars().count()
            + l.host.chars().count()
            + l.rest1.chars().count();
        assert!(total <= 40, "{total}");
    }

    #[test]
    fn expanded_the_path_continues_on_a_second_line() {
        let url = format!("https://example.com/{}", "abcdefghij".repeat(10));
        let l = fit(&split_url(&url), 40, 80);
        assert!(l.second_line());
        let one: String = format!("{}{}", l.rest1, l.rest2);
        assert!(one.contains(&"abcdefghij".repeat(5)));
        assert!(l.prefix.chars().count() + l.host.chars().count() + l.rest1.chars().count() <= 40);
        // Longer than both lines: the middle of the second goes.
        let huge = format!("https://example.com/{}", "x".repeat(500));
        let l = fit(&split_url(&huge), 40, 60);
        assert!(l.rest2.chars().count() <= 60 && l.rest2.contains(ELLIPSIS));
        // Fits one line: no second.
        assert!(!fit(&split_url("https://example.com/a"), 40, 80).second_line());
    }

    const PAGE: Frame = Frame::new(100.0, 80.0, 1000.0, 700.0);
    const SIZE: Vec2 = Vec2::new(300.0, 30.0);

    #[test]
    fn sits_bottom_left_with_an_inset() {
        let f = frame_at(Side::Left, PAGE, SIZE, 10.0);
        assert_eq!((f.origin.x, f.origin.y), (110.0, 80.0 + 700.0 - 10.0 - 30.0));
        let r = frame_at(Side::Right, PAGE, SIZE, 10.0);
        assert_eq!(r.origin.x + r.size.x, 1090.0);
    }

    #[test]
    fn stays_put_while_the_pointer_is_far() {
        let far = Some(Vec2::new(600.0, 300.0));
        assert_eq!(choose_side(Side::Left, PAGE, SIZE, 10.0, far, 36.0, &[]), Side::Left);
        assert_eq!(choose_side(Side::Right, PAGE, SIZE, 10.0, far, 36.0, &[]), Side::Right);
        assert_eq!(choose_side(Side::Left, PAGE, SIZE, 10.0, None, 36.0, &[]), Side::Left);
    }

    #[test]
    fn moves_right_when_the_pointer_nears_the_left_corner() {
        let bubble = frame_at(Side::Left, PAGE, SIZE, 10.0);
        let near_it = Some(bubble.center() + Vec2::new(0.0, -30.0));
        assert_eq!(choose_side(Side::Left, PAGE, SIZE, 10.0, near_it, 36.0, &[]), Side::Right);
        // Just outside the clearance: it stays.
        let outside = Some(Vec2::new(bubble.origin.x + 10.0, bubble.origin.y - 37.0));
        assert_eq!(choose_side(Side::Left, PAGE, SIZE, 10.0, outside, 36.0, &[]), Side::Left);
    }

    #[test]
    fn moves_back_left_when_the_pointer_nears_the_right_corner() {
        let bubble = frame_at(Side::Right, PAGE, SIZE, 10.0);
        assert_eq!(choose_side(Side::Right, PAGE, SIZE, 10.0, Some(bubble.center()), 36.0, &[]), Side::Left);
        // On the right it does not hop back while the pointer is still near the left corner.
        let left = frame_at(Side::Left, PAGE, SIZE, 10.0);
        assert_eq!(choose_side(Side::Right, PAGE, SIZE, 10.0, Some(left.center()), 36.0, &[]), Side::Right);
    }

    #[test]
    fn yields_to_a_toast_or_popover_in_its_corner() {
        let right = frame_at(Side::Right, PAGE, SIZE, 10.0);
        let toast = Frame::new(right.origin.x + 20.0, right.origin.y - 5.0, 200.0, 44.0);
        assert_eq!(choose_side(Side::Right, PAGE, SIZE, 10.0, None, 36.0, &[toast]), Side::Left);
        // Both corners taken: it keeps clear of the pointer, else stays.
        let left = frame_at(Side::Left, PAGE, SIZE, 10.0);
        assert_eq!(choose_side(Side::Right, PAGE, SIZE, 10.0, None, 36.0, &[toast, left]), Side::Right);
        let p = Some(right.center());
        assert_eq!(choose_side(Side::Right, PAGE, SIZE, 10.0, p, 36.0, &[toast, left]), Side::Left);
    }

    #[test]
    fn appears_after_the_delay_and_lingers_briefly() {
        let mut v = Visibility::default();
        assert!(!v.update(true, 0.05, 0.12, 0.08));
        assert!(v.update(true, 0.08, 0.12, 0.08));
        // Gone only after the hide delay, and back at once when the pointer returns.
        assert!(v.update(false, 0.05, 0.12, 0.08));
        assert!(v.update(true, 0.01, 0.12, 0.08));
        assert!(v.update(false, 0.05, 0.12, 0.08));
        assert!(!v.update(false, 0.05, 0.12, 0.08));
    }

    #[test]
    fn a_link_passed_over_quickly_never_shows() {
        let mut v = Visibility::default();
        v.update(true, 0.1, 0.12, 0.08);
        assert!(!v.update(false, 0.016, 0.12, 0.08));
        assert!(!v.update(true, 0.1, 0.12, 0.08));
    }

    #[test]
    fn counts_how_long_an_address_has_been_up() {
        let mut v = Visibility::default();
        v.update(true, 0.2, 0.12, 0.08);
        v.update(true, 0.5, 0.12, 0.08);
        v.update(true, 0.5, 0.12, 0.08);
        assert!((v.up_for - 1.0).abs() < 1e-4);
        v.restart_expansion();
        assert_eq!(v.up_for, 0.0);
    }
}
