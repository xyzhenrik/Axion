//! The Axion app icon as a window icon (X11 `_NET_WM_ICON`, Windows/macOS where winit supports
//! it). On Wayland the compositor takes the icon from `axion.desktop` via the app id instead
//! (`assets/axion.desktop`, `assets/icons/hicolor/`).

use winit::window::Icon;

/// 256 px PNG of the app icon (the Liquid Glass globe tile, `assets/icons/axion.png`; regenerate the set with `tools/make-icons.sh`).
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

/// The same artwork as pixels (the first-run introduction and the About page show it).
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

    /// The logo is a transparent-cornered tile: opaque centre, clear corners. Set
    /// `AXION_ICON_DUMP=path.png` to write the pixels the window icon gets.
    #[test]
    fn icon_pixels_are_the_transparent_tile() {
        let img = super::image().expect("icon");
        let (w, h) = img.dimensions();
        assert_eq!((w, h), (256, 256));
        assert_eq!(img.get_pixel(0, 0)[3], 0);
        assert_eq!(img.get_pixel(w - 1, h - 1)[3], 0);
        assert!(img.get_pixel(w / 2, h / 2)[3] > 250);
        if let Some(path) = std::env::var_os("AXION_ICON_DUMP") {
            img.save(path).expect("dump");
        }
    }
}
