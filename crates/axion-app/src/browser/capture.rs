//! Print / Save as PDF and the screenshot tool, as the browser drives them: the dialogs'
//! clicks, choosing where to save (the same rules as downloads: a file chooser when
//! "Ask where to save each file" is on, else the folder), and the engine's answers.

use std::path::Path;
use std::sync::Mutex;

use axion_config::ShotAfter;
use axion_engine::download::{sanitize_file_name, unique_path};
use axion_i18n::tr;

use super::*;
use crate::downloads::{choose_save_portal, file_uri, spawn};
use crate::print_dialog::{PrintAction, pdf_options};
use crate::screenshot::{Phase, ShotAction, ShotMode};

/// Answers from background threads.
enum Mail {
    /// The PDF's path from the file chooser (`None`: cancelled).
    PdfPath(Option<PathBuf>),
    /// The screenshot's path from the file chooser.
    ShotPath(Option<PathBuf>),
    /// A screenshot file was written (or not).
    Written(bool),
}

struct ShotJob {
    id: u32,
    view: ViewId,
    /// The selected part of the page, in the view's logical px.
    crop: Option<Rect>,
    /// A tile of a full page screenshot: the scroll offset it was taken at (CSS px).
    tile_y: Option<f64>,
}

/// A full page screenshot: the page is scrolled a screen at a time, each screen captured, and
/// the screens put together (off-screen Chromium cannot capture beyond its viewport).
struct Stitch {
    view: ViewId,
    /// Until the page reported its size.
    metrics: bool,
    /// Gives up waiting for the page's answer at this time.
    deadline: Instant,
    doc_height: f64,
    viewport_height: f64,
    scroll_before: f64,
    /// Width of the page's scrollbar (CSS px), left out of the result.
    scrollbar: f64,
    next_y: f64,
    /// Capture once this time has passed (the page has scrolled and drawn).
    capture_at: Option<Instant>,
    tiles: Vec<(f64, image::RgbaImage)>,
}

/// How long the page may take to report its size.
const METRICS_TIMEOUT: Duration = Duration::from_secs(3);
const METRICS_PREFIX: &str = "\u{1}axion:shot-metrics:";
const METRICS_SCRIPT: &str = "(function(){var d=document.documentElement,b=document.body;\
var h=Math.max(d.scrollHeight,b?b.scrollHeight:0);\
console.log('\\u0001axion:shot-metrics:'+h+','+window.innerHeight+','+window.scrollY+','+(window.innerWidth-d.clientWidth));})()";

fn scroll_script(y: f64) -> String {
    format!("window.scrollTo({{top:{y},left:0,behavior:'instant'}})")
}

#[derive(Default)]
pub(super) struct CaptureState {
    mailbox: Arc<Mutex<Vec<Mail>>>,
    next_id: u32,
    pdf_view: Option<ViewId>,
    job: Option<ShotJob>,
    stitch: Option<Stitch>,
    result: Option<image::RgbaImage>,
    /// A file chooser is open.
    choosing: bool,
}

impl CaptureState {
    /// When a full page screenshot needs attention next.
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        let stitch = self.stitch.as_ref()?;
        Some(stitch.capture_at.unwrap_or(stitch.deadline))
    }

    fn post(mailbox: &Arc<Mutex<Vec<Mail>>>, mail: Mail) {
        if let Ok(mut mailbox) = mailbox.lock() {
            mailbox.push(mail);
        }
    }
}

fn printable(url: &str) -> bool {
    ["http://", "https://", "file:", "data:", "blob:"].iter().any(|p| url.starts_with(p))
}

/// The folder for `configured` (empty: the download folder).
fn target_folder(config: &Config, configured: &str) -> PathBuf {
    let configured = configured.trim();
    if configured.is_empty() {
        return downloads::download_dir(config);
    }
    match (configured.strip_prefix("~/"), directories::BaseDirs::new()) {
        (Some(rest), Some(dirs)) => dirs.home_dir().join(rest),
        _ => PathBuf::from(configured),
    }
}

impl Browser {
    // ---- print / PDF ----

    pub(super) fn open_print(&mut self) {
        let Some(view) = self.active_view().filter(|_| !self.is_locked()) else { return };
        if self.overview.is_open() || self.print_dialog.is_open() {
            return;
        }
        let Some(tab) = self.tabs.active() else { return };
        if !printable(&tab.url) {
            self.show_toast(Icon::Close, tr("This page can’t be printed"));
            return;
        }
        let title = tab.label();
        self.menu.close(&mut self.ui, &self.config);
        self.close_find();
        self.snap.pdf_view = Some(view);
        self.print_dialog.show(&mut self.ui, &self.config, &title);
    }

    pub(super) fn close_print(&mut self) {
        self.print_dialog.close();
    }

    /// A click on the print dialog; `false` when it isn't the dialog's.
    pub(super) fn print_click(&mut self, id: axion_ui::ActionId) -> bool {
        if !self.print_dialog.is_open() {
            return false;
        }
        let Some(action) = self.print_dialog.action(id, &mut self.config) else { return false };
        match action {
            PrintAction::Changed => {
                self.config_dirty_since.get_or_insert_with(Instant::now);
            }
            PrintAction::Save => {
                self.config_dirty_since.get_or_insert_with(Instant::now);
                self.save_pdf();
            }
            PrintAction::Cancel => self.close_print(),
            PrintAction::Backdrop => {
                if !self.pointer.is_some_and(|p| self.print_dialog.contains(&self.ui, p)) {
                    self.close_print();
                }
            }
        }
        true
    }

    fn save_pdf(&mut self) {
        if self.snap.choosing {
            return;
        }
        let Some(tab) = self.tabs.active() else { return };
        let name = format!("{}.pdf", sanitize_file_name(&tab.label()));
        let folder = target_folder(&self.config, &self.config.print.directory);
        if self.config.downloads.ask_where_to_save {
            self.snap.choosing = true;
            let mailbox = Arc::clone(&self.snap.mailbox);
            spawn("choose-pdf", move || {
                let chosen = choose_save_portal(tr("Save as PDF"), &name, &folder).unwrap_or_else(|e| {
                    tracing::warn!("save dialog unavailable: {e}");
                    None
                });
                CaptureState::post(&mailbox, Mail::PdfPath(chosen));
            });
        } else {
            if let Err(e) = std::fs::create_dir_all(&folder) {
                tracing::warn!("creating {} failed: {e}", folder.display());
            }
            self.start_pdf(unique_path(&folder, &name));
        }
    }

    fn start_pdf(&mut self, path: PathBuf) {
        let Some(view) = self.snap.pdf_view.take() else { return };
        self.close_print();
        tracing::info!(path = %path.display(), "printing to PDF");
        self.engine.print_to_pdf(view, path, &pdf_options(&self.config));
        self.show_toast(Icon::Download, tr("Saving PDF…"));
    }

    pub(super) fn pdf_saved(&mut self, path: &Path, ok: bool) {
        if !ok {
            self.show_toast(Icon::Close, tr("Couldn’t save the PDF"));
            return;
        }
        self.show_toast(Icon::Check, tr("PDF saved"));
        if self.config.print.open_after_saving
            && let Err(e) = self.open_tab(&file_uri(path), true)
        {
            tracing::error!("opening the PDF failed: {e:#}");
        }
    }

    // ---- screenshots ----

    pub(super) fn start_screenshot(&mut self) {
        if self.active_view().is_none() || self.is_locked() || self.overview.is_open() {
            return;
        }
        if self.shot.is_active() {
            return self.close_screenshot();
        }
        self.menu.close(&mut self.ui, &self.config);
        self.close_find();
        let page = self.page_rect();
        self.shot.start(&mut self.ui, page);
    }

    pub(super) fn close_screenshot(&mut self) {
        self.snap.stitch = None;
        self.shot.close(&mut self.ui);
        self.snap.job = None;
        self.snap.result = None;
    }

    /// A click on the screenshot tool; `false` when it isn't the tool's.
    pub(super) fn shot_click(&mut self, id: axion_ui::ActionId) -> bool {
        if !self.shot.is_active() {
            return false;
        }
        let Some(action) = self.shot.action(id) else { return false };
        match action {
            ShotAction::Mode(ShotMode::Selection) => self.shot.set_mode(ShotMode::Selection),
            ShotAction::Mode(ShotMode::Visible) => self.begin_capture(None, None),
            ShotAction::Mode(ShotMode::FullPage) => self.begin_full_page(),
            ShotAction::Close => self.close_screenshot(),
            ShotAction::Dismiss => {
                // The backdrop's action also fires for clicks on the card's background.
                let on_card = self.pointer.is_some_and(|p| self.shot.review_contains(&self.ui, p));
                if !on_card {
                    self.close_screenshot();
                }
            }
            ShotAction::Copy => {
                self.copy_shot();
                self.close_screenshot();
            }
            ShotAction::Save => self.save_shot(),
        }
        true
    }

    /// The drag over the page ended with this rectangle (window coordinates).
    pub(super) fn shot_selected(&mut self, rect: Rect) {
        let page = self.page_rect();
        let crop = Rect { origin: rect.origin - page.origin, size: rect.size };
        self.begin_capture(Some(crop), None);
    }

    fn begin_capture(&mut self, crop: Option<Rect>, tile_y: Option<f64>) {
        let Some(tab) = self.tabs.active() else { return };
        let view = tab.view;
        self.snap.next_id += 1;
        let id = self.snap.next_id;
        self.shot.capturing(&mut self.ui);
        self.snap.job = Some(ShotJob { id, view, crop, tile_y });
        self.engine.capture_screenshot(view, id);
    }

    fn begin_full_page(&mut self) {
        let Some(view) = self.tabs.active().map(|t| t.view) else { return };
        self.shot.capturing(&mut self.ui);
        self.snap.stitch = Some(Stitch {
            view,
            metrics: false,
            deadline: Instant::now() + METRICS_TIMEOUT,
            doc_height: 0.0,
            viewport_height: 0.0,
            scroll_before: 0.0,
            scrollbar: 0.0,
            next_y: 0.0,
            capture_at: None,
            tiles: Vec::new(),
        });
        self.engine.run_script(view, METRICS_SCRIPT, false);
    }

    /// The page reported its size (see [`METRICS_SCRIPT`]); `true` when the message was one.
    pub(super) fn shot_metrics(&mut self, view: ViewId, message: &str) -> bool {
        let Some(text) = message.strip_prefix(METRICS_PREFIX) else { return false };
        let max = f64::from(self.config.screenshot.full_page_max_height);
        let Some(stitch) = self.snap.stitch.as_mut().filter(|s| s.view == view && !s.metrics) else {
            return true;
        };
        let numbers: Vec<f64> = text.split(',').filter_map(|n| n.trim().parse().ok()).collect();
        let [height, viewport, scroll, scrollbar] = numbers[..] else {
            self.close_screenshot();
            return true;
        };
        stitch.metrics = true;
        stitch.doc_height = height.min(max).max(1.0);
        stitch.viewport_height = viewport.max(1.0);
        stitch.scroll_before = scroll;
        stitch.scrollbar = scrollbar.max(0.0);
        self.stitch_scroll(0.0);
        true
    }

    /// Scrolls the page to `y` and captures it a moment later (at once with a settle time of 0:
    /// the scroll goes through DevTools like the capture, so they stay in order).
    fn stitch_scroll(&mut self, y: f64) {
        let settle = Duration::from_millis(u64::from(self.config.screenshot.scroll_settle_ms));
        let Some(stitch) = self.snap.stitch.as_mut() else { return };
        stitch.next_y = y;
        let view = stitch.view;
        self.engine.run_script(view, &scroll_script(y), true);
        if settle.is_zero() {
            stitch.capture_at = None;
            self.begin_capture(None, Some(y));
        } else {
            stitch.capture_at = Some(Instant::now() + settle);
        }
    }

    /// Puts the tiles of a full page screenshot together.
    fn stitch_finish(&mut self) {
        let Some(stitch) = self.snap.stitch.take() else { return };
        self.engine.run_script(stitch.view, &scroll_script(stitch.scroll_before), true);
        let Some((_, first)) = stitch.tiles.first() else { return self.close_screenshot() };
        let k = f64::from(first.height()) / stitch.viewport_height;
        let height = ((stitch.doc_height * k).round() as u32).max(1);
        let width = first.width().saturating_sub((stitch.scrollbar * k).round() as u32).max(1);
        let mut canvas = image::RgbaImage::new(width, height);
        for (y, tile) in &stitch.tiles {
            let tile = image::imageops::crop_imm(tile, 0, 0, width.min(tile.width()), tile.height());
            image::imageops::replace(&mut canvas, &tile.to_image(), 0, (y * k).round() as i64);
        }
        self.snap.result = Some(canvas);
        self.after_capture();
    }

    pub(super) fn screenshot_ready(&mut self, view: ViewId, id: u32, png: &[u8]) {
        let Some(job) = self.snap.job.take_if(|j| j.view == view && j.id == id) else { return };
        let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png);
        let Ok(decoded) = decoded.map(|d| d.to_rgba8()) else {
            self.close_screenshot();
            self.show_toast(Icon::Close, tr("Couldn’t take the screenshot"));
            return;
        };
        if let Some(y) = job.tile_y {
            self.tile_ready(y, decoded);
            return;
        }
        let image = match job.crop {
            Some(crop) => {
                // The visible shot has the view's size at device resolution.
                let page = self.page_rect();
                let k = decoded.width() as f32 / page.size.x.max(1.0);
                let x = ((crop.origin.x * k).round().max(0.0) as u32).min(decoded.width().saturating_sub(1));
                let y = ((crop.origin.y * k).round().max(0.0) as u32).min(decoded.height().saturating_sub(1));
                let w = ((crop.size.x * k).round().max(1.0) as u32).min(decoded.width() - x);
                let h = ((crop.size.y * k).round().max(1.0) as u32).min(decoded.height() - y);
                image::imageops::crop_imm(&decoded, x, y, w, h).to_image()
            }
            None => decoded,
        };
        tracing::info!(width = image.width(), height = image.height(), "screenshot taken");
        self.snap.result = Some(image);
        self.after_capture();
    }

    fn tile_ready(&mut self, y: f64, tile: image::RgbaImage) {
        let Some(stitch) = self.snap.stitch.as_mut() else { return };
        stitch.tiles.push((y, tile));
        let bottom = y + stitch.viewport_height;
        if bottom >= stitch.doc_height {
            return self.stitch_finish();
        }
        // The last screen is aligned to the bottom (it overlaps the one before).
        let next = bottom.min((stitch.doc_height - stitch.viewport_height).max(0.0)).max(y + 1.0);
        self.stitch_scroll(next);
    }

    fn after_capture(&mut self) {
        match self.config.screenshot.after_capture {
            ShotAfter::Copy => {
                self.copy_shot();
                self.close_screenshot();
            }
            ShotAfter::Save => self.save_shot(),
            ShotAfter::Ask => self.review_shot(),
        }
    }

    pub(super) fn screenshot_failed(&mut self, view: ViewId, id: u32) {
        if self.snap.job.take_if(|j| j.view == view && j.id == id).is_some() {
            self.close_screenshot();
            self.show_toast(Icon::Close, tr("Couldn’t take the screenshot"));
        }
    }

    fn review_shot(&mut self) {
        let Some(image) = &self.snap.result else { return };
        let (w, h) = image.dimensions();
        // A preview is small; huge full page shots are scaled down for it.
        let limit = 1600;
        let preview = if w.max(h) > limit {
            let k = limit as f32 / w.max(h) as f32;
            let size = ((w as f32 * k).round().max(1.0) as u32, (h as f32 * k).round().max(1.0) as u32);
            image::imageops::thumbnail(image, size.0, size.1)
        } else {
            image.clone()
        };
        let texture = self.favicons.upload(preview.width(), preview.height(), preview.as_raw());
        let format = match self.config.screenshot.format {
            axion_config::ShotFormat::Png => "PNG",
            axion_config::ShotFormat::Jpeg => "JPEG",
        };
        let info = format!("{w} \u{00D7} {h} px \u{00B7} {format}");
        self.shot.show_review(&mut self.ui, &self.config, texture, info);
    }

    fn copy_shot(&mut self) {
        let Some(image) = self.snap.result.clone() else { return };
        let Some(clipboard) = &mut self.clipboard else {
            self.show_toast(Icon::Close, tr("Couldn’t copy the screenshot"));
            return;
        };
        let data = arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: std::borrow::Cow::Owned(image.into_raw()),
        };
        match clipboard.set_image(data) {
            Ok(()) => self.show_toast(Icon::Check, tr("Screenshot copied")),
            Err(e) => {
                tracing::warn!("copying the screenshot failed: {e}");
                self.show_toast(Icon::Close, tr("Couldn’t copy the screenshot"));
            }
        }
    }

    fn save_shot(&mut self) {
        if self.snap.choosing || self.snap.result.is_none() {
            return;
        }
        let extension = match self.config.screenshot.format {
            axion_config::ShotFormat::Png => "png",
            axion_config::ShotFormat::Jpeg => "jpg",
        };
        let stamp = jiff::Zoned::now().strftime("%Y-%m-%d at %H.%M.%S");
        let name = format!("{} {stamp}.{extension}", tr("Screenshot"));
        let folder = target_folder(&self.config, &self.config.screenshot.directory);
        if self.config.downloads.ask_where_to_save {
            self.snap.choosing = true;
            let mailbox = Arc::clone(&self.snap.mailbox);
            spawn("choose-screenshot", move || {
                let chosen = choose_save_portal(tr("Save Screenshot"), &name, &folder).unwrap_or_else(|e| {
                    tracing::warn!("save dialog unavailable: {e}");
                    None
                });
                CaptureState::post(&mailbox, Mail::ShotPath(chosen));
            });
        } else {
            if let Err(e) = std::fs::create_dir_all(&folder) {
                tracing::warn!("creating {} failed: {e}", folder.display());
            }
            self.write_shot(unique_path(&folder, &name));
        }
    }

    fn write_shot(&mut self, path: PathBuf) {
        let Some(image) = self.snap.result.take() else { return };
        self.shot.close(&mut self.ui);
        let jpeg_quality = self.config.screenshot.jpeg_quality.clamp(1, 100);
        let mailbox = Arc::clone(&self.snap.mailbox);
        spawn("write-screenshot", move || {
            let jpeg = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
            let result = if jpeg {
                std::fs::File::create(&path).map_err(|e| e.to_string()).and_then(|file| {
                    let rgb = image::DynamicImage::ImageRgba8(image).to_rgb8();
                    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                        std::io::BufWriter::new(file),
                        jpeg_quality,
                    );
                    rgb.write_with_encoder(encoder).map_err(|e| e.to_string())
                })
            } else {
                image.save_with_format(&path, image::ImageFormat::Png).map_err(|e| e.to_string())
            };
            if let Err(e) = &result {
                tracing::warn!("writing {} failed: {e}", path.display());
            }
            CaptureState::post(&mailbox, Mail::Written(result.is_ok()));
        });
    }

    // ---- background answers ----

    /// Applies what the file choosers and writers reported. Call every frame.
    pub(super) fn poll_capture(&mut self) {
        let now = Instant::now();
        if let Some(stitch) = self.snap.stitch.as_mut() {
            if !stitch.metrics && now >= stitch.deadline {
                self.close_screenshot();
                self.show_toast(Icon::Close, tr("Couldn’t take the screenshot"));
            } else if self.snap.job.is_none()
                && let Some(at) = stitch.capture_at
                && now >= at
            {
                stitch.capture_at = None;
                let y = stitch.next_y;
                self.begin_capture(None, Some(y));
            }
        }
        let mail: Vec<Mail> = match self.snap.mailbox.lock() {
            Ok(mut mailbox) => std::mem::take(&mut *mailbox),
            Err(_) => return,
        };
        for mail in mail {
            match mail {
                Mail::PdfPath(path) => {
                    self.snap.choosing = false;
                    if let Some(path) = path {
                        self.start_pdf(path);
                    }
                }
                Mail::ShotPath(path) => {
                    self.snap.choosing = false;
                    match path {
                        Some(path) => self.write_shot(path),
                        // Cancelled: the review stays (or, saving straight away, ends).
                        None if self.shot.phase() != Phase::Review => self.close_screenshot(),
                        None => {}
                    }
                }
                Mail::Written(true) => self.show_toast(Icon::Check, tr("Screenshot saved")),
                Mail::Written(false) => self.show_toast(Icon::Close, tr("Couldn’t save the screenshot")),
            }
        }
    }
}
