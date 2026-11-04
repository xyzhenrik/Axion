//! The window's own frame (`window.decorations = "custom"`): the window buttons' actions, moving
//! the window by its toolbar, resizing it from its edges, the rounded corners and the focus
//! state the buttons show. The pure parts (edge hit test, button look) live in `crate::window_frame`.

use axion_compositor::SurfaceAlpha;
use axion_config::WindowDecorations;
use winit::window::ResizeDirection;

use super::*;
use crate::window_frame::{self, resize_cursor, resize_hit};

/// What the browser keeps for the window's frame.
pub(super) struct FrameState {
    /// The window is in front (has the keyboard focus).
    pub focused: bool,
    /// Capture aid: the window counts as unfocused / maximized whatever it really is.
    pub focus_override: Option<bool>,
    pub maximized_override: Option<bool>,
    /// Presses on the toolbar's free space, to tell a double click (maximize) from a drag.
    pub clicks: ClickCounter,
    /// How the surface treats alpha: anything but `Opaque` can show transparent corners.
    pub alpha: SurfaceAlpha,
    /// The corner radius the compositor was last given (physical px, for the frame skip).
    pub shape: f32,
}

impl FrameState {
    pub fn new(alpha: SurfaceAlpha, capturing: bool) -> Self {
        Self {
            focused: true,
            // A capture run's window is minimized and never focused: the lights show in front
            // unless an aid (`unfocused`) says otherwise.
            focus_override: capturing.then_some(true),
            maximized_override: None,
            clicks: ClickCounter::default(),
            alpha,
            shape: 0.0,
        }
    }
}

/// The surface alpha mode for the window and what the compositor can do with it: with custom
/// decorations a pre-multiplied (else post-multiplied) mode makes the corners transparent; any
/// other case, or a surface with neither (XWayland, most X11 drivers), stays opaque with square
/// corners.
pub(super) fn pick_alpha_mode(
    modes: &[wgpu::CompositeAlphaMode],
    custom: bool,
) -> (wgpu::CompositeAlphaMode, SurfaceAlpha) {
    use wgpu::CompositeAlphaMode as Mode;
    let first = modes.first().copied().unwrap_or(Mode::Auto);
    if !custom {
        return (first, SurfaceAlpha::Opaque);
    }
    if modes.contains(&Mode::PreMultiplied) {
        (Mode::PreMultiplied, SurfaceAlpha::PreMultiplied)
    } else if modes.contains(&Mode::PostMultiplied) {
        (Mode::PostMultiplied, SurfaceAlpha::PostMultiplied)
    } else {
        tracing::info!(?modes, "the surface has no alpha mode: the window keeps square corners");
        (first, SurfaceAlpha::Opaque)
    }
}

impl Browser {
    pub(super) fn custom_frame(&self) -> bool {
        self.config.window.decorations == WindowDecorations::Custom
    }

    /// Whether the window is maximized (a capture aid can say it is).
    pub(super) fn window_maximized(&self) -> bool {
        self.frame.maximized_override.unwrap_or_else(|| self.window.is_maximized())
    }

    /// Whether the window is fullscreen (F11, a page's or the player's fullscreen).
    pub(super) fn window_fullscreen(&self) -> bool {
        self.chrome.is_fullscreen() || self.window.fullscreen().is_some()
    }

    /// The resize direction a press at `p` (logical px) starts, if it is on the window's edge.
    pub(super) fn resize_edge_at(&self, p: Vec2) -> Option<ResizeDirection> {
        let enabled =
            window_frame::resize_enabled(&self.config, self.window_maximized(), self.window_fullscreen());
        resize_hit(p, self.logical_size(), self.config.window.resize_border_px, enabled)
    }

    /// The pointer cursor over the window's edges (`None` elsewhere).
    pub(super) fn resize_cursor_at(&self, p: Vec2) -> Option<winit::window::CursorIcon> {
        self.resize_edge_at(p).map(resize_cursor)
    }

    /// A button press at `p` that belongs to the window's frame: an edge starts a resize, free
    /// toolbar space starts a move (a double click maximizes), a right click there opens the
    /// system window menu. Returns whether the press was taken.
    pub(super) fn frame_press(&mut self, button: winit::event::MouseButton, p: Vec2) -> bool {
        use winit::event::MouseButton;
        if !self.custom_frame() {
            return false;
        }
        match button {
            MouseButton::Left => {
                if let Some(direction) = self.resize_edge_at(p) {
                    match self.window.drag_resize_window(direction) {
                        Ok(()) => return true,
                        Err(e) => tracing::debug!("the window cannot be resized by dragging: {e}"),
                    }
                    return false;
                }
                if !self.chrome.is_drag_region(&self.ui, &self.config, p) {
                    return false;
                }
                let count = self.frame.clicks.press(MouseButton::Left, p);
                if count >= 2 && count % 2 == 0 {
                    self.run(Action::ToggleMaximize);
                } else if let Err(e) = self.window.drag_window() {
                    tracing::debug!("the window cannot be moved by dragging: {e}");
                }
                true
            }
            MouseButton::Right if self.chrome.is_drag_region(&self.ui, &self.config, p) => {
                self.window
                    .show_window_menu(winit::dpi::LogicalPosition::new(f64::from(p.x), f64::from(p.y)));
                true
            }
            _ => false,
        }
    }

    /// The window buttons' and the matching shortcuts' actions.
    pub(super) fn window_action(&mut self, action: Action) {
        match action {
            Action::CloseWindow => self.quit_at = Some(Instant::now()),
            Action::MinimizeWindow => self.window.set_minimized(true),
            Action::ToggleMaximize if self.window_fullscreen() => {}
            Action::ToggleMaximize => {
                let maximized = self.window_maximized();
                self.window.set_maximized(!maximized);
            }
            _ => {}
        }
    }

    /// The window gained or lost the keyboard focus.
    pub(super) fn frame_focus(&mut self, focused: bool) {
        // A capture run's window is minimized and never focused: its look is the capture's call.
        if self.frame.focus_override.is_none() {
            self.frame.focused = focused;
        }
    }

    /// Once per frame: the window buttons' look (pointer, focus, maximized) and the window's
    /// corners (cut away while maximized or fullscreen). Returns the corner radius in physical
    /// px, for the frame skip.
    pub(super) fn update_frame(&mut self) -> f32 {
        let maximized = self.window_maximized();
        let focused = self.frame.focus_override.unwrap_or(self.frame.focused);
        let hovered = if self.tab_drag.is_dragging() { None } else { self.ui.hovered() };
        let pressed = self.ui.pressed_action();
        self.chrome.update_lights(&mut self.ui, hovered, pressed, focused, maximized);
        let radius = window_frame::window_corner_radius(
            &self.config,
            maximized,
            self.window_fullscreen(),
            self.frame.alpha != SurfaceAlpha::Opaque,
        ) * self.scale();
        self.compositor.set_window_shape(radius, self.frame.alpha);
        self.frame.shape = radius;
        radius
    }

    /// The settings changed: the desktop's frame comes or goes (the surface's transparency is
    /// fixed when the window is made, so corners need a restart to appear).
    pub(super) fn apply_frame_config(&mut self) {
        let custom = self.custom_frame();
        if self.window.is_decorated() == custom {
            self.window.set_decorations(!custom);
        }
    }

    /// Capture aids: `unfocused` / `focused` and `window-maximized` / `window-restored` set the
    /// window buttons' look without touching the window.
    pub(super) fn frame_capture_aid(&mut self, what: &str) -> bool {
        match what {
            "unfocused" => self.frame.focus_override = Some(false),
            "focused" => self.frame.focus_override = Some(true),
            "window-maximized" => self.frame.maximized_override = Some(true),
            "window-restored" => self.frame.maximized_override = Some(false),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::CompositeAlphaMode as Mode;

    #[test]
    fn corners_need_a_surface_with_alpha() {
        assert_eq!(
            pick_alpha_mode(&[Mode::Opaque, Mode::PreMultiplied], true),
            (Mode::PreMultiplied, SurfaceAlpha::PreMultiplied)
        );
        assert_eq!(
            pick_alpha_mode(&[Mode::Opaque, Mode::PostMultiplied, Mode::PreMultiplied], true),
            (Mode::PreMultiplied, SurfaceAlpha::PreMultiplied),
            "pre-multiplied is the one Wayland compositors take"
        );
        assert_eq!(
            pick_alpha_mode(&[Mode::Opaque, Mode::PostMultiplied], true),
            (Mode::PostMultiplied, SurfaceAlpha::PostMultiplied)
        );
        // XWayland / most X11 drivers: opaque, square corners.
        assert_eq!(
            pick_alpha_mode(&[Mode::Opaque, Mode::Inherit], true),
            (Mode::Opaque, SurfaceAlpha::Opaque)
        );
        // The desktop's frame: never transparent.
        assert_eq!(
            pick_alpha_mode(&[Mode::PreMultiplied, Mode::Opaque], false),
            (Mode::PreMultiplied, SurfaceAlpha::Opaque)
        );
        assert_eq!(pick_alpha_mode(&[], true), (Mode::Auto, SurfaceAlpha::Opaque));
    }
}
