//! Writes a fake home directory with browser profiles (the fixtures of the tests) for
//! captures of Axion's import UI:
//!
//! ```text
//! cargo run -p axion-import --example fake_home -- /tmp/fake-home
//! AXION_IMPORT_HOME=/tmp/fake-home axion --capture=out.png --open=settings:data
//! ```

fn main() {
    let Some(dir) = std::env::args_os().nth(1) else {
        eprintln!("usage: fake_home <directory>");
        std::process::exit(2);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1_800_000_000, |d| d.as_secs() as i64);
    let dir = std::path::Path::new(&dir);
    axion_import::fixtures::fake_home(dir, now);
    // Synthetic site icons (coloured discs) so a capture shows imported icons.
    use axion_import::icon_fixtures::{ChromiumIcon, chromium_favicons, firefox_favicons};
    let disc = |side: u32, rgb: [u8; 3]| {
        let mut img = image::RgbaImage::new(side, side);
        let c = f64::from(side) / 2.0;
        for (x, y, p) in img.enumerate_pixels_mut() {
            let d = ((f64::from(x) + 0.5 - c).powi(2) + (f64::from(y) + 0.5 - c).powi(2)).sqrt();
            if d <= c - 0.5 {
                *p = image::Rgba([rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        let mut out = Vec::new();
        let _ = image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png);
        out
    };
    chromium_favicons(
        &dir.join(".config/google-chrome/Default"),
        &[
            ChromiumIcon {
                pages: &["https://www.rust-lang.org/"],
                bitmaps: &[(16, 16, disc(16, [222, 90, 40])), (32, 32, disc(32, [222, 90, 40]))],
            },
            ChromiumIcon {
                pages: &["https://news.ycombinator.com/"],
                bitmaps: &[(32, 32, disc(32, [255, 102, 0]))],
            },
            ChromiumIcon {
                pages: &["https://de.wikipedia.org/"],
                bitmaps: &[(64, 64, disc(64, [70, 70, 70]))],
            },
        ],
    );
    firefox_favicons(
        &dir.join(".mozilla/firefox/abcd1234.default-release"),
        &[
            ("https://www.mozilla.org/a.png", 32, disc(32, [30, 120, 230]), &["https://www.mozilla.org/"]),
            ("https://lwn.net/favicon.ico", 16, disc(16, [40, 160, 80]), &["https://lwn.net/"]),
        ],
    );
    println!("fake home written to {}", dir.display());
}
