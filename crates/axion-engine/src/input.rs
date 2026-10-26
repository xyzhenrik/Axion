use glam::Vec2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Super / Windows / Command.
    pub logo: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// Input for a view. Positions are in the view's logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    MouseMove {
        pos: Vec2,
        modifiers: Modifiers,
    },
    MouseLeave {
        modifiers: Modifiers,
    },
    MouseButton {
        pos: Vec2,
        button: MouseButton,
        pressed: bool,
        click_count: u32,
        modifiers: Modifiers,
    },
    /// Scroll by `delta` logical pixels (positive y scrolls content down… i.e. wheel up).
    Wheel {
        pos: Vec2,
        delta: Vec2,
        modifiers: Modifiers,
    },
    /// A physical key. `key_code` is the Windows virtual-key code (the DOM's legacy
    /// `keyCode`); `native_code` the platform scancode (XKB keycode on Linux).
    Key {
        pressed: bool,
        key_code: i32,
        native_code: i32,
        modifiers: Modifiers,
    },
    /// Text produced by a key press or an input method.
    Text {
        text: String,
        modifiers: Modifiers,
    },
}

/// Mouse cursor shapes a page can ask for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorKind {
    #[default]
    Default,
    Pointer,
    Text,
    Crosshair,
    Wait,
    Progress,
    Help,
    Move,
    Grab,
    Grabbing,
    NotAllowed,
    /// Dropping here copies (a drag).
    Copy,
    /// Dropping here links (a drag).
    Alias,
    ResizeHorizontal,
    ResizeVertical,
    Hidden,
}
