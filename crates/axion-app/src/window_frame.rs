//! Client-side window decorations (`window.decorations = "custom"`): the window has no title
//! bar from the desktop, so Axion draws the window buttons itself (macOS-style "traffic
//! lights" in the toolbar), moves the window by dragging empty toolbar space and resizes it
//! from a strip along its edges.
//!
//! This module holds the logic that needs no window: where a press on the edge resizes
//! ([`resize_hit`]), which cursor goes with it, what the buttons look like in each state
//! ([`LightsState`]) and how much room they take ([`lights_width`]). `chrome.rs` draws them,
//! `browser.rs` routes the pointer to the winit calls.

use axion_compositor::Icon;
use axion_config::{Color, Config, WindowDecorations};
use glam::Vec2;
use winit::window::{CursorIcon, ResizeDirection};

/// How much darker a pressed light is.
const PRESSED_DARKEN: f32 = 0.78;
/// How much darker the ring around a light is than its disc.
const RING_DARKEN: f32 = 0.78;
/// How much darker a light is while the pointer is over the lights.
const HOVER_DARKEN: f32 = 0.94;

/// The colours the lights are drawn in (`window.light_*`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightColors {
    pub close: Color,
    pub minimize: Color,
    pub zoom: Color,
    pub unfocused: Color,
    pub glyph: Color,
}

impl LightColors {
    pub fn from_config(config: &Config) -> Self {
        let w = &config.window;
        Self {
            close: w.light_close,
            minimize: w.light_minimize,
            zoom: w.light_zoom,
            unfocused: w.light_unfocused,
            glyph: w.light_glyph,
        }
    }
}

/// One of the window buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Light {
    Close,
    Minimize,
    Zoom,
}

impl Light {
    /// In the order they sit in (left to right).
    pub const ALL: [Light; 3] = [Light::Close, Light::Minimize, Light::Zoom];

    pub fn index(self) -> usize {
        match self {
            Light::Close => 0,
            Light::Minimize => 1,
            Light::Zoom => 2,
        }
    }

    fn color(self, colors: &LightColors) -> Color {
        match self {
            Light::Close => colors.close,
            Light::Minimize => colors.minimize,
            Light::Zoom => colors.zoom,
        }
    }
}

/// What a light looks like right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightLook {
    /// The flat colour of its disc (never glass).
    pub color: Color,
    /// The slightly darker ring around the disc.
    pub ring: Color,
    /// The glyph inside (x, -, arrows), shown while the pointer is over the lights.
    pub glyph: Icon,
    /// Its colour: the same on all three lights.
    pub glyph_color: Color,
    pub glyph_shown: bool,
}

/// The state the lights' look comes from: pointer over them, a held press, whether the window
/// is in front and whether it is maximized (the zoom light then shows the restore arrows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightsState {
    hovered: Option<Light>,
    pressed: Option<Light>,
    focused: bool,
    maximized: bool,
}

impl Default for LightsState {
    fn default() -> Self {
        Self { hovered: None, pressed: None, focused: true, maximized: false }
    }
}

impl LightsState {
    /// The pointer is over `light` (`None`: not over any). Returns whether the look changed.
    pub fn set_hovered(&mut self, light: Option<Light>) -> bool {
        let before = *self;
        self.hovered = light;
        // The pointer left while a light was held: the press is over (the click is cancelled).
        if light != self.pressed {
            self.pressed = None;
        }
        *self != before
    }

    /// `light` is held down by the pointer (`None`: nothing is).
    pub fn set_pressed(&mut self, light: Option<Light>) -> bool {
        let before = *self;
        self.pressed = light;
        *self != before
    }

    /// The window gained or lost the keyboard focus.
    pub fn set_focused(&mut self, focused: bool) -> bool {
        let before = *self;
        self.focused = focused;
        *self != before
    }

    pub fn set_maximized(&mut self, maximized: bool) -> bool {
        let before = *self;
        self.maximized = maximized;
        *self != before
    }

    /// The glyphs show on all three lights while the pointer is over any of them.
    pub fn glyphs_shown(&self) -> bool {
        self.hovered.is_some()
    }

    /// The look of `light`: coloured while the window is in front or the pointer is over the
    /// lights, grey otherwise (like macOS); a little darker while the pointer is over the lights
    /// and darker still while held.
    pub fn look(&self, light: Light, colors: &LightColors) -> LightLook {
        let coloured = self.focused || self.hovered.is_some();
        let base = if coloured { light.color(colors) } else { colors.unfocused };
        let color = if self.pressed == Some(light) {
            darken(base, PRESSED_DARKEN)
        } else if self.hovered.is_some() {
            darken(base, HOVER_DARKEN)
        } else {
            base
        };
        let glyph = match light {
            Light::Close => Icon::Close,
            Light::Minimize => Icon::Minus,
            Light::Zoom if self.maximized => Icon::RestoreArrows,
            Light::Zoom => Icon::ZoomArrows,
        };
        LightLook {
            color,
            ring: darken(color, RING_DARKEN),
            glyph,
            glyph_color: colors.glyph,
            glyph_shown: self.glyphs_shown(),
        }
    }
}

fn darken(color: Color, by: f32) -> Color {
    let scale = |c: u8| (f32::from(c) * by).round().clamp(0.0, 255.0) as u32;
    Color::rgb(scale(color.r) << 16 | scale(color.g) << 8 | scale(color.b))
}

/// The row of three lights: `3 * size + 2 * gap`.
pub fn lights_width(size: f32, gap: f32) -> f32 {
    3.0 * size + 2.0 * gap
}

/// How much of the toolbar's left end the lights take, with the gap to the group next to them.
pub fn lights_inset(size: f32, gap: f32, margin: f32) -> f32 {
    lights_width(size, gap) + margin
}

/// Whether the lights are drawn at all: only with custom decorations, and never in fullscreen.
pub fn lights_wanted(config: &Config, fullscreen: bool) -> bool {
    config.window.decorations == WindowDecorations::Custom && !fullscreen
}

/// Where a press at `p` in a window of `size` (logical px) starts a resize: the strip along
/// the edges is `border` wide; the corners reach `2 * border` along each edge. `None` in the
/// middle, outside the window, or when resizing is off (`enabled` is false while maximized or
/// fullscreen, and with the system's own frame).
pub fn resize_hit(p: Vec2, size: Vec2, border: f32, enabled: bool) -> Option<ResizeDirection> {
    if !enabled || border <= 0.0 || p.x < 0.0 || p.y < 0.0 || p.x >= size.x || p.y >= size.y {
        return None;
    }
    let corner = 2.0 * border;
    let left = p.x < border;
    let right = p.x >= size.x - border;
    let top = p.y < border;
    let bottom = p.y >= size.y - border;
    let near_left = p.x < corner;
    let near_right = p.x >= size.x - corner;
    let near_top = p.y < corner;
    let near_bottom = p.y >= size.y - corner;
    Some(match () {
        _ if (top && near_left) || (left && near_top) => ResizeDirection::NorthWest,
        _ if (top && near_right) || (right && near_top) => ResizeDirection::NorthEast,
        _ if (bottom && near_left) || (left && near_bottom) => ResizeDirection::SouthWest,
        _ if (bottom && near_right) || (right && near_bottom) => ResizeDirection::SouthEast,
        _ if top => ResizeDirection::North,
        _ if bottom => ResizeDirection::South,
        _ if left => ResizeDirection::West,
        _ if right => ResizeDirection::East,
        _ => return None,
    })
}

/// The pointer shape over a resize edge.
pub fn resize_cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::NResize,
        ResizeDirection::South => CursorIcon::SResize,
        ResizeDirection::East => CursorIcon::EResize,
        ResizeDirection::West => CursorIcon::WResize,
        ResizeDirection::NorthEast => CursorIcon::NeResize,
        ResizeDirection::NorthWest => CursorIcon::NwResize,
        ResizeDirection::SouthEast => CursorIcon::SeResize,
        ResizeDirection::SouthWest => CursorIcon::SwResize,
    }
}

/// Whether the edges resize the window: custom frame, and the window is a free-floating one.
pub fn resize_enabled(config: &Config, maximized: bool, fullscreen: bool) -> bool {
    config.window.decorations == WindowDecorations::Custom && !maximized && !fullscreen
}

/// The corner radius (logical px) the window is cut to: none while maximized or fullscreen,
/// with the system frame, or where the surface cannot show transparency.
pub fn window_corner_radius(config: &Config, maximized: bool, fullscreen: bool, transparent: bool) -> f32 {
    if config.window.decorations == WindowDecorations::Custom && !maximized && !fullscreen && transparent {
        config.layout.window_corner_radius.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axion_config::ControlsSide;

    const SIZE: Vec2 = Vec2::new(1000.0, 700.0);
    const BORDER: f32 = 6.0;

    fn hit(x: f32, y: f32) -> Option<ResizeDirection> {
        resize_hit(Vec2::new(x, y), SIZE, BORDER, true)
    }

    #[test]
    fn every_edge_and_corner_resizes_in_its_direction() {
        assert_eq!(hit(500.0, 2.0), Some(ResizeDirection::North));
        assert_eq!(hit(500.0, 697.0), Some(ResizeDirection::South));
        assert_eq!(hit(2.0, 350.0), Some(ResizeDirection::West));
        assert_eq!(hit(997.0, 350.0), Some(ResizeDirection::East));
        assert_eq!(hit(2.0, 2.0), Some(ResizeDirection::NorthWest));
        assert_eq!(hit(997.0, 2.0), Some(ResizeDirection::NorthEast));
        assert_eq!(hit(2.0, 697.0), Some(ResizeDirection::SouthWest));
        assert_eq!(hit(997.0, 697.0), Some(ResizeDirection::SouthEast));
    }

    #[test]
    fn corners_reach_twice_the_border_along_the_edges() {
        // On the top edge, 11 px from the left: still the corner; 13 px: the edge.
        assert_eq!(hit(11.0, 2.0), Some(ResizeDirection::NorthWest));
        assert_eq!(hit(13.0, 2.0), Some(ResizeDirection::North));
        // The same down the left edge.
        assert_eq!(hit(2.0, 11.0), Some(ResizeDirection::NorthWest));
        assert_eq!(hit(2.0, 13.0), Some(ResizeDirection::West));
        assert_eq!(hit(997.0, 690.0), Some(ResizeDirection::SouthEast));
        assert_eq!(hit(988.0, 697.0), Some(ResizeDirection::SouthEast));
        assert_eq!(hit(986.0, 697.0), Some(ResizeDirection::South));
        assert_eq!(hit(988.0, 690.0), None, "the corner is the strips' L, not a square");
    }

    #[test]
    fn the_inside_and_the_outside_do_not_resize() {
        assert_eq!(hit(500.0, 350.0), None);
        assert_eq!(hit(BORDER, 350.0), None);
        assert_eq!(hit(500.0, BORDER), None);
        assert_eq!(hit(-1.0, 10.0), None);
        assert_eq!(hit(1000.0, 10.0), None);
        assert_eq!(hit(10.0, 700.0), None);
    }

    #[test]
    fn nothing_resizes_when_it_is_off() {
        assert_eq!(resize_hit(Vec2::new(2.0, 2.0), SIZE, BORDER, false), None);
        assert_eq!(resize_hit(Vec2::new(2.0, 2.0), SIZE, 0.0, true), None);
        let mut config = Config::default();
        assert!(resize_enabled(&config, false, false));
        assert!(!resize_enabled(&config, true, false), "maximized");
        assert!(!resize_enabled(&config, false, true), "fullscreen");
        config.window.decorations = WindowDecorations::System;
        assert!(!resize_enabled(&config, false, false), "the desktop's frame resizes");
    }

    #[test]
    fn every_direction_has_its_own_cursor() {
        let all = [
            ResizeDirection::North,
            ResizeDirection::South,
            ResizeDirection::East,
            ResizeDirection::West,
            ResizeDirection::NorthEast,
            ResizeDirection::NorthWest,
            ResizeDirection::SouthEast,
            ResizeDirection::SouthWest,
        ];
        let cursors: Vec<CursorIcon> = all.iter().map(|&d| resize_cursor(d)).collect();
        for (i, a) in cursors.iter().enumerate() {
            assert!(!cursors[i + 1..].contains(a), "{a:?} is shared");
        }
        assert_eq!(resize_cursor(ResizeDirection::NorthWest), CursorIcon::NwResize);
        assert_eq!(resize_cursor(ResizeDirection::East), CursorIcon::EResize);
    }

    #[test]
    fn the_lights_take_room_from_the_toolbar() {
        assert_eq!(lights_width(18.0, 8.0), 70.0);
        assert_eq!(lights_inset(18.0, 8.0, 14.0), 84.0);
        let c = Config::default();
        assert_eq!(
            lights_inset(
                c.layout.traffic_light_size,
                c.layout.traffic_light_gap,
                c.layout.traffic_light_margin
            ),
            84.0
        );
    }

    #[test]
    fn lights_show_only_with_custom_decorations_and_never_in_fullscreen() {
        let mut config = Config::default();
        assert!(lights_wanted(&config, false));
        assert!(!lights_wanted(&config, true));
        config.window.decorations = WindowDecorations::System;
        assert!(!lights_wanted(&config, false));
        assert_eq!(config.window.controls_side, ControlsSide::Left);
    }

    #[test]
    fn the_corners_are_square_unless_a_free_custom_window_can_be_transparent() {
        let mut config = Config::default();
        assert_eq!(window_corner_radius(&config, false, false, true), config.layout.window_corner_radius);
        assert_eq!(window_corner_radius(&config, true, false, true), 0.0, "maximized");
        assert_eq!(window_corner_radius(&config, false, true, true), 0.0, "fullscreen");
        assert_eq!(window_corner_radius(&config, false, false, false), 0.0, "opaque surface");
        config.window.decorations = WindowDecorations::System;
        assert_eq!(window_corner_radius(&config, false, false, true), 0.0, "the desktop's frame");
    }

    #[test]
    fn lights_are_coloured_in_front_grey_behind_and_darker_when_held() {
        let c = LightColors::from_config(&Config::default());
        let (red, yellow, green, grey) = (c.close, c.minimize, c.zoom, c.unfocused);
        let mut s = LightsState::default();
        assert_eq!(s.look(Light::Close, &c).color, red);
        assert_eq!(s.look(Light::Minimize, &c).color, yellow);
        assert_eq!(s.look(Light::Zoom, &c).color, green);
        assert!(!s.look(Light::Close, &c).glyph_shown);

        // Not in front: grey.
        assert!(s.set_focused(false));
        for light in Light::ALL {
            assert_eq!(s.look(light, &c).color, grey);
        }
        // The pointer over them brings the colours (and glyphs) back, even behind.
        assert!(s.set_hovered(Some(Light::Minimize)));
        assert_ne!(s.look(Light::Close, &c).color, grey, "coloured again");
        assert!(s.look(Light::Zoom, &c).glyph_shown);
        assert!(s.set_hovered(None));
        assert_eq!(s.look(Light::Close, &c).color, grey);

        // Held: darker, and only the held one.
        assert!(s.set_focused(true));
        assert!(s.set_hovered(Some(Light::Close)));
        assert!(s.set_pressed(Some(Light::Close)));
        let held = s.look(Light::Close, &c).color;
        assert!(held.r < red.r, "darker while held");
        assert_ne!(s.look(Light::Minimize, &c).color, yellow, "the others only darken on hover");
        // The pointer slides off the held light: the press is over.
        assert!(s.set_hovered(Some(Light::Zoom)));
        assert!(s.look(Light::Close, &c).color.r > held.r, "released");
        // Nothing changes when nothing changes.
        assert!(!s.set_hovered(Some(Light::Zoom)));
        assert!(!s.set_focused(true));
    }

    #[test]
    fn glyphs_share_one_colour_and_the_ring_is_darker() {
        let c = LightColors::from_config(&Config::default());
        let mut s = LightsState::default();
        s.set_hovered(Some(Light::Close));
        for light in Light::ALL {
            let look = s.look(light, &c);
            assert_eq!(look.glyph_color, c.glyph);
            assert!(
                look.ring.r <= look.color.r && look.ring.g <= look.color.g && look.ring.b <= look.color.b
            );
            assert_ne!(look.ring, look.color);
        }
    }

    #[test]
    fn the_zoom_light_shows_the_restore_arrows_while_maximized() {
        let c = LightColors::from_config(&Config::default());
        let mut s = LightsState::default();
        assert_eq!(s.look(Light::Zoom, &c).glyph, Icon::ZoomArrows);
        assert!(s.set_maximized(true));
        assert_eq!(s.look(Light::Zoom, &c).glyph, Icon::RestoreArrows);
        assert_eq!(s.look(Light::Close, &c).glyph, Icon::Close);
        assert_eq!(s.look(Light::Minimize, &c).glyph, Icon::Minus);
        assert!(!s.set_maximized(true));
    }
}
