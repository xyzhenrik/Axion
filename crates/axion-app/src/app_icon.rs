//! The Axion app icon as a window icon (X11 `_NET_WM_ICON`, Windows/macOS where winit supports
//! it). On Wayland the compositor takes the icon from `axion.desktop` via the app id instead
//! (`assets/axion.desktop`, `assets/icons/hicolor/`).

use winit::window::Icon;

/// 256 px PNG of the default icon (the user's glass-compass artwork, `assets/icons/axion-user.png`; see plan.md "App icon").
static ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/hicolor/256x256/apps/axion.png");

/// The window icon, or `None` if the embedded PNG cannot be decoded.
pub fn window_icon() -> Option<Icon> {
    let img = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
        .inspect_err(|e| tracing::warn!("app icon: {e}"))
        .ok()?
        .into_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).inspect_err(|e| tracing::warn!("app icon: {e}")).ok()
}

/// The same artwork as pixels (the About page shows it).
pub fn image() -> Option<image::RgbaImage> {
    image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
        .inspect_err(|e| tracing::warn!("app icon: {e}"))
        .ok()
        .map(|img| img.into_rgba8())
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_icon_decodes() {
        assert!(super::window_icon().is_some());
    }
}
