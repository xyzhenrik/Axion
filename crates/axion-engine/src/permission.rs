//! What pages ask permission for (microphone, camera, screen sharing, notifications, …) and
//! the browser's answers. The engine reports each request as an
//! [`EngineEvent::PermissionRequest`](crate::EngineEvent::PermissionRequest) and waits: nothing
//! is granted until the browser answers with [`Engine::answer_permission`](crate::Engine).

/// One thing a page can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PermissionKind {
    /// `getUserMedia({ audio })`.
    Microphone,
    /// `getUserMedia({ video })` (with pan, tilt and zoom).
    Camera,
    /// `getDisplayMedia()`: a screen or window. The desktop (portal) picks which.
    ScreenShare,
    /// The sound of a shared screen (`getDisplayMedia({ audio })`). Always asked together with
    /// [`PermissionKind::ScreenShare`].
    ScreenAudio,
    Notifications,
    Geolocation,
    /// Reading the clipboard (`navigator.clipboard.read`).
    Clipboard,
    /// MIDI devices with system-exclusive messages.
    Midi,
    /// Devices on the local network or this computer (Local Network Access).
    LocalNetwork,
    /// Placing windows on other screens (Window Management API).
    WindowManagement,
    /// The fonts installed on this computer.
    LocalFonts,
    /// Whether the user is idle (Idle Detection API).
    IdleDetection,
    /// Hiding and capturing the mouse pointer (games).
    PointerLock,
    /// Capturing system keys such as Esc (games, remote desktops).
    KeyboardLock,
    /// Motion and light sensors.
    Sensors,
    /// Downloading several files at once.
    AutomaticDownloads,
}

impl PermissionKind {
    pub const ALL: [PermissionKind; 16] = [
        PermissionKind::Microphone,
        PermissionKind::Camera,
        PermissionKind::ScreenShare,
        PermissionKind::ScreenAudio,
        PermissionKind::Notifications,
        PermissionKind::Geolocation,
        PermissionKind::Clipboard,
        PermissionKind::Midi,
        PermissionKind::LocalNetwork,
        PermissionKind::WindowManagement,
        PermissionKind::LocalFonts,
        PermissionKind::IdleDetection,
        PermissionKind::PointerLock,
        PermissionKind::KeyboardLock,
        PermissionKind::Sensors,
        PermissionKind::AutomaticDownloads,
    ];

    /// A stable name for storing decisions.
    pub fn key(self) -> &'static str {
        match self {
            PermissionKind::Microphone => "microphone",
            PermissionKind::Camera => "camera",
            PermissionKind::ScreenShare => "screen",
            PermissionKind::ScreenAudio => "screen_audio",
            PermissionKind::Notifications => "notifications",
            PermissionKind::Geolocation => "geolocation",
            PermissionKind::Clipboard => "clipboard",
            PermissionKind::Midi => "midi",
            PermissionKind::LocalNetwork => "local_network",
            PermissionKind::WindowManagement => "window_management",
            PermissionKind::LocalFonts => "local_fonts",
            PermissionKind::IdleDetection => "idle_detection",
            PermissionKind::PointerLock => "pointer_lock",
            PermissionKind::KeyboardLock => "keyboard_lock",
            PermissionKind::Sensors => "sensors",
            PermissionKind::AutomaticDownloads => "automatic_downloads",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key() == key)
    }

    /// Microphone, camera or screen: the page records something while granted (the browser
    /// shows that it does).
    pub fn is_capture(self) -> bool {
        matches!(
            self,
            PermissionKind::Microphone
                | PermissionKind::Camera
                | PermissionKind::ScreenShare
                | PermissionKind::ScreenAudio
        )
    }
}

/// Identifies one pending permission request for the lifetime of the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PermissionId(pub u64);

/// The browser's answer to a permission request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionAnswer {
    /// Grants everything the request asked for.
    Allow,
    Deny,
    /// Closed without a decision (Esc, the page navigated away): the page sees a refusal.
    Dismiss,
}
