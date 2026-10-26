//! What a page's context menu is about (the right click's target) and what Axion's own menu
//! can ask the engine to do with it. The engine never shows a menu: off-screen Chromium has no
//! native one, so it reports [`ContextMenuParams`] and the browser draws its own.

use glam::Vec2;

/// The media element (or image) that was right-clicked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContextMedia {
    #[default]
    None,
    Image,
    Video,
    Audio,
    Canvas,
    /// A plugin or file embed: nothing Axion offers anything for.
    Other,
}

/// Which editing commands the focused field currently allows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditFlags {
    pub undo: bool,
    pub redo: bool,
    pub cut: bool,
    pub copy: bool,
    pub paste: bool,
    pub delete: bool,
    pub select_all: bool,
}

/// What was right-clicked, as reported with [`crate::EngineEvent::ContextMenu`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContextMenuParams {
    /// Where the click was (view coordinates, logical px).
    pub pos: Vec2,
    pub media: ContextMedia,
    /// The media element's `src` (an image's or video's address); empty if none.
    pub source_url: String,
    /// The image can be copied (decoded by the page).
    pub has_image_contents: bool,
    /// The link under the pointer (resolved `href`); empty if none.
    pub link_url: String,
    pub selection_text: String,
    pub editable: bool,
    pub edit: EditFlags,
    /// The address of the page, and of the frame the click was in (differs in an iframe).
    pub page_url: String,
    pub frame_url: String,
}

/// An editing command run in the focused frame of a view (see `Engine::context_command`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextCommand {
    Undo,
    Redo,
    Cut,
    Copy,
    /// Pastes from the engine's own clipboard (only where it can reach the desktop's; see
    /// `Engine::has_system_clipboard`).
    Paste,
    /// Deletes the selection (without touching the clipboard).
    Delete,
    SelectAll,
}
