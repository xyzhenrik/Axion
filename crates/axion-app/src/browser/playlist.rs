//! The browser's side of Axion Playlist: the Add button and its popover, saving videos with
//! their thumbnails, downloads, the Playlist page and its player, and the saved positions.
//! The model is `playlist`, the page `playlist_page`, the popover `playlist_popover`.

use std::collections::HashMap;

use axion_engine::{DownloadInfo, MediaViewOptions, ResourceKind, ResourceRequest};
use axion_media::{Action as MediaAction, Capture, Command as MediaCommand, MediaSource, PageMedia, Sniffed};
use axion_store::{PlaylistItem, dl};

use super::*;
use crate::media_view;
use crate::menu::MenuItem;
use crate::playlist::{
    self, Added, Candidate, Command, DownloadPlan, Failure, Pending, Playback, Repeat, Resolving, Sleep,
    Source,
};
use crate::playlist_page::{PageAction, PlayerModel, PlaylistPage, RowModel, VideoPicture};
use crate::playlist_popover::{PlaylistPopover, PopoverAction as AddAction, VideoInfo};

type Thumb = (wgpu::TextureView, (u32, u32));

/// Thumbnails as GPU textures, loaded from the profile a few per frame.
pub(super) struct ThumbCache {
    device: wgpu::Device,
    queue: wgpu::Queue,
    loaded: HashMap<i64, Option<Thumb>>,
    budget: u32,
}

impl ThumbCache {
    fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self { device, queue, loaded: HashMap::new(), budget: 0 }
    }

    /// Allows `n` more decodes (once per frame).
    fn refill(&mut self, n: u32) {
        self.budget = n;
    }

    fn forget(&mut self, id: i64) {
        self.loaded.remove(&id);
    }

    /// The thumbnail of item `id` stored at `path`; `None` while it isn't loaded (the budget
    /// of this frame is spent: `pending` is set) or has none.
    fn get(&mut self, id: i64, path: &str, pending: &mut bool) -> Option<Thumb> {
        if path.is_empty() {
            return None;
        }
        if let Some(found) = self.loaded.get(&id) {
            return found.clone();
        }
        if self.budget == 0 {
            *pending = true;
            return None;
        }
        self.budget -= 1;
        let image = image::open(path).ok().map(|i| i.to_rgba8());
        let uploaded = image.map(|img| {
            let (w, h) = img.dimensions();
            let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("playlist thumbnail"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.queue.write_texture(
                texture.as_image_copy(),
                img.as_raw(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * w),
                    rows_per_image: Some(h),
                },
                size,
            );
            (texture.create_view(&Default::default()), (w, h))
        });
        self.loaded.insert(id, uploaded.clone());
        uploaded
    }
}

/// The Playlist's UI state in the browser.
pub(super) struct PlaylistUi {
    pub page: PlaylistPage,
    pub popover: PlaylistPopover,
    thumbs: ThumbCache,
    /// The active tab's videos (what the Add button offers).
    pub candidates: Vec<Candidate>,
    /// What the address field's button last showed.
    button: Option<Option<bool>>,
    /// What the fullscreen player's button last showed.
    player_button: Option<Option<bool>>,
    /// The ids of the rows shown, in order.
    pub shown: Vec<i64>,
    query: String,
    /// The size the playback view was last given (logical px).
    view_size: Vec2,
    /// How far the page's header keeps clear of the sidebar button.
    inset: f32,
    /// The tab that was shown when the page opened (its view rests meanwhile).
    resting: Option<ViewId>,
}

impl PlaylistUi {
    pub fn build(
        ui: &mut Ui,
        config: &Config,
        bar: axion_ui::taffy::NodeId,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Self {
        Self {
            page: PlaylistPage::build(ui, config),
            popover: PlaylistPopover::build(ui, bar),
            thumbs: ThumbCache::new(device, queue),
            candidates: Vec::new(),
            button: None,
            player_button: None,
            shown: Vec::new(),
            query: String::new(),
            view_size: Vec2::ZERO,
            inset: 0.0,
            resting: None,
        }
    }
}

/// What a row says under its title.
fn item_detail(item: &PlaylistItem, progress: Option<Option<f32>>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let length = playlist::duration_label(item.duration);
    if !length.is_empty() {
        parts.push(length);
    }
    match (item.dl_state.as_str(), progress) {
        (dl::DOWNLOADING | dl::QUEUED, Some(Some(p))) => {
            parts.push(format!("Downloading {:.0} %", p * 100.0))
        }
        (dl::DOWNLOADING | dl::QUEUED, _) => parts.push("Downloading".to_owned()),
        (dl::DONE, _) => parts.push(downloads::human_size(item.dl_size)),
        (dl::FAILED, _) => parts.push("Download failed".to_owned()),
        (dl::STREAMING, _) => parts.push(playlist::STATE_NOT_DOWNLOADED.to_owned()),
        _ => {}
    }
    if item.watched {
        parts.push("Watched".to_owned());
    }
    if parts.is_empty() {
        parts.push(display_host(&item.page_url));
    }
    parts.join(" · ")
}

fn downloaded_file(item: &PlaylistItem) -> Option<PathBuf> {
    (item.dl_state == dl::DONE && !item.dl_path.is_empty())
        .then(|| PathBuf::from(&item.dl_path))
        .filter(|p| p.exists())
}

/// What plays in the player area for an item.
enum Target {
    /// A file on this computer (downloaded, or the user's own).
    Local(PathBuf),
    /// A captured source, in the media view.
    Source(MediaSource),
    /// A source exists but can't be played here; the text says why.
    Unplayable(&'static str),
    /// Nothing captured yet: the page is loaded hidden to find it.
    Find,
}

impl Browser {
    // ---- starting up ----

    /// Reads the list from the profile (once the store is open).
    pub(super) fn playlist_start(&mut self) {
        self.migrate_list_name();
        self.playlist.reload(self.store.as_ref(), &self.config);
    }

    /// The old default list name, from before the list was simply "Playlist".
    const OLD_LIST_NAME: &'static str = "Play Later";

    /// Profiles from when the list was called "Play Later": show it as "Playlist" and move the
    /// stored items along, so they stay visible.
    fn migrate_list_name(&mut self) {
        let old = Self::OLD_LIST_NAME;
        let new = axion_config::Playlist::default().list_name;
        if self.config.playlist.list_name == old {
            self.config.playlist.list_name = new.clone();
        }
        if let Some(store) = &self.store
            && let Err(e) = store.rename_playlist(old, &new)
        {
            tracing::warn!("renaming the old Playlist failed: {e}");
        }
    }

    // ---- the Add button ----

    /// Candidates for `tab`: nothing in private tabs or on pages without a video.
    fn tab_candidates(&self, tab: &Tab) -> Vec<Candidate> {
        if tab.private {
            return Vec::new();
        }
        let Some(media) = &tab.media else { return Vec::new() };
        let sniffed = self.playlist.sniffed.get(&tab.view).map_or(&[][..], Vec::as_slice);
        playlist::candidates(
            &tab.url,
            &tab.label(),
            media,
            sniffed,
            self.config.playlist.max_title_chars as usize,
        )
    }

    /// Keeps the address field's Playlist button and the fullscreen player's in step with the
    /// page (called every frame; changes the chrome only when something changed).
    pub(super) fn update_playlist_buttons(&mut self) {
        let shown = !self.playlist_ui.page.is_open() && !self.is_locked();
        let candidates = match self.tabs.active() {
            Some(tab) if shown && self.config.playlist.quick_action && self.store.is_some() => {
                self.tab_candidates(tab)
            }
            _ => Vec::new(),
        };
        let state =
            (!candidates.is_empty()).then(|| candidates.iter().any(|c| self.playlist.contains(c).is_some()));
        self.playlist_ui.candidates = candidates;
        if Some(state) != self.playlist_ui.button {
            self.playlist_ui.button = Some(state);
            self.chrome.set_playlist_button(&mut self.ui, state);
            if self.playlist_ui.popover.is_open() {
                if state.is_none() {
                    self.playlist_ui.popover.close(&mut self.ui, &self.config);
                } else {
                    self.open_playlist_popover();
                }
            }
        }
        // The fullscreen player's own button: for the video it plays, never in private tabs.
        let player_state = self.player.target().and_then(|(view, player)| {
            let tab = find_tab(&self.tabs, &self.parked, view)?;
            if tab.private || self.store.is_none() {
                return None;
            }
            let c = self.tab_candidates(tab).into_iter().find(|c| c.player == player.id)?;
            Some(self.playlist.contains(&c).is_some())
        });
        if Some(player_state) != self.playlist_ui.player_button {
            self.playlist_ui.player_button = Some(player_state);
            self.player.set_playlist_button(&mut self.ui, player_state);
        }
    }

    fn video_infos(&self) -> Vec<VideoInfo> {
        self.playlist_ui
            .candidates
            .iter()
            .map(|c| {
                let length = playlist::duration_label(c.duration);
                let host = display_host(&c.page_url);
                let note = match &c.source {
                    Source::Streaming(why) => Some((*why).to_owned()),
                    _ => None,
                };
                VideoInfo {
                    title: c.title.clone(),
                    subtitle: if length.is_empty() { host } else { format!("{host} · {length}") },
                    can_download: matches!(c.source, Source::Direct(_)),
                    note,
                    added: self.playlist.contains(c).is_some(),
                }
            })
            .collect()
    }

    pub(super) fn toggle_playlist_popover(&mut self) {
        if self.playlist_ui.popover.is_open() {
            self.playlist_ui.popover.close(&mut self.ui, &self.config);
        } else {
            self.close_popups();
            self.open_playlist_popover();
        }
    }

    pub(super) fn open_playlist_popover(&mut self) {
        let infos = self.video_infos();
        if infos.is_empty() {
            return;
        }
        let anchor = self.chrome.playlist_button_frame(&self.ui);
        let width = self.logical_size().x;
        self.playlist_ui.popover.open(&mut self.ui, &self.config, anchor, width, &infos);
    }

    pub(super) fn playlist_popover_action(&mut self, action: AddAction) {
        match action {
            AddAction::Add { video, download } => {
                if let (Some(c), Some(view)) =
                    (self.playlist_ui.candidates.get(video).cloned(), self.active_view())
                {
                    self.playlist_ui.popover.close(&mut self.ui, &self.config);
                    self.add_to_playlist(view, c, download);
                }
            }
            AddAction::Remove(video) => {
                if let Some(id) =
                    self.playlist_ui.candidates.get(video).and_then(|c| self.playlist.contains(c))
                {
                    self.playlist_ui.popover.close(&mut self.ui, &self.config);
                    self.remove_playlist_items(&[id]);
                    self.show_toast(Icon::Trash, "Removed from Playlist");
                }
            }
            AddAction::OpenPlaylist => {
                self.playlist_ui.popover.close(&mut self.ui, &self.config);
                self.open_playlist();
            }
        }
    }

    /// The keymap's `playlist_add`: the page's video, or a choice between several.
    pub(super) fn playlist_add_action(&mut self) {
        let Some(tab) = self.tabs.active() else { return };
        if tab.private {
            self.show_toast(Icon::Lock, "Private tabs can’t add to the Playlist");
            return;
        }
        let Some(view) = self.active_view() else { return };
        match self.playlist_ui.candidates.len() {
            0 => self.show_toast(Icon::Playlist, "No video on this page"),
            1 => {
                let c = self.playlist_ui.candidates[0].clone();
                self.add_to_playlist(view, c, false);
            }
            _ => self.toggle_playlist_popover(),
        }
    }

    /// The fullscreen player's button: its video, or off the list again.
    pub(super) fn playlist_add_player(&mut self, view: ViewId, player: &axion_media::Player) {
        let Some(tab) = find_tab(&self.tabs, &self.parked, view) else { return };
        if tab.private {
            return;
        }
        let Some(c) = self.tab_candidates(tab).into_iter().find(|c| c.player == player.id) else {
            return;
        };
        match self.playlist.contains(&c) {
            Some(id) => {
                self.remove_playlist_items(&[id]);
            }
            None => self.add_to_playlist(view, c, false),
        }
    }

    /// Saves a video. `download` (or `playlist.auto_download`) also fetches the file when it
    /// is a plain one.
    fn add_to_playlist(&mut self, view: ViewId, c: Candidate, download: bool) {
        let Some(store) = &self.store else { return };
        if find_tab(&self.tabs, &self.parked, view).is_some_and(|t| t.private) {
            return;
        }
        match playlist::add(store, &self.config, &c, unix_now()) {
            Added::New(id) => {
                self.playlist.reload(self.store.as_ref(), &self.config);
                self.show_toast(Icon::PlaylistAdd, "Added to Playlist");
                self.start_thumbnail(id, view, &c);
                if download || self.config.playlist.auto_download {
                    self.playlist_download(id, true);
                }
            }
            Added::Existing(id) => {
                // Saved before its source was known: keep it now.
                if let Capture::Found(source) = &c.capture
                    && self.playlist.item(id).is_some_and(|i| i.source.is_empty())
                {
                    self.set_item_source(id, source);
                }
                self.show_toast(Icon::Playlist, "Already in your Playlist");
                if download {
                    self.playlist_download(id, true);
                }
            }
            Added::Full => {
                let text = format!("Playlist is full ({} videos)", self.config.playlist.max_items);
                self.show_toast(Icon::Playlist, &text);
            }
            Added::Failed => self.show_toast(Icon::Playlist, "Couldn’t save to the Playlist"),
        }
        self.playlist_ui.button = None;
    }

    // ---- thumbnails ----

    /// Gets a picture for a new item: the page's own (poster, preview image) through the
    /// engine's network stack, else a frame of the video from the tab's picture.
    fn start_thumbnail(&mut self, id: i64, view: ViewId, c: &Candidate) {
        if let Some(url) = &c.thumb_url {
            self.engine.fetch_favicon(view, url, self.config.playlist.thumbnail_max_px);
            self.playlist.thumb_jobs.push(playlist::ThumbJob {
                item: id,
                view,
                url: url.clone(),
                started: Instant::now(),
            });
            return;
        }
        self.thumbnail_from_tab(id, view, c);
    }

    /// A frame of the video as the tab shows it.
    fn thumbnail_from_tab(&mut self, id: i64, view: ViewId, c: &Candidate) {
        let Some(frame) = self.engine.frame(view) else { return };
        let tex = Vec2::new(frame.texture.width() as f32, frame.texture.height() as f32);
        let dpr = if c.dpr > 0.0 { c.dpr } else { self.scale() };
        let (x, y, w, h) = c.rect;
        let (min, max) = (Vec2::new(x, y) * dpr / tex, Vec2::new(x + w, y + h) * dpr / tex);
        let (min, max) = (min.clamp(Vec2::ZERO, Vec2::ONE), max.clamp(Vec2::ZERO, Vec2::ONE));
        if max.x - min.x < 0.01 || max.y - min.y < 0.01 {
            return;
        }
        let uv = Rect { origin: min, size: max - min };
        self.save_thumbnail_from_texture(id, &frame.texture, uv);
    }

    /// Reads part of a texture back, scales it down and keeps it as the item's thumbnail.
    fn save_thumbnail_from_texture(&mut self, id: i64, texture: &wgpu::Texture, uv: Rect) {
        let Some(image) = self.read_texture(texture, uv, self.config.playlist.thumbnail_max_px) else {
            return;
        };
        self.save_thumbnail(id, image);
    }

    fn save_thumbnail(&mut self, id: i64, image: image::RgbaImage) {
        let dir = self.playlist.thumb_dir.clone();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("can't create the Playlist's thumbnail folder: {e}");
            return;
        }
        let path = dir.join(format!("{id}.png"));
        if let Err(e) = image.save(&path) {
            tracing::warn!("saving a Playlist thumbnail failed: {e}");
            return;
        }
        if let Some(store) = &self.store
            && let Err(e) = store.set_playlist_thumbnail(id, &path.to_string_lossy())
        {
            tracing::warn!("{e}");
        }
        self.playlist_ui.thumbs.forget(id);
        self.playlist.reload(self.store.as_ref(), &self.config);
    }

    /// Copies the part `uv` of `texture` to the CPU, scaled so its long side is `max_px`.
    fn read_texture(&self, texture: &wgpu::Texture, uv: Rect, max_px: u32) -> Option<image::RgbaImage> {
        let (tw, th) = (texture.width(), texture.height());
        let x0 = ((uv.origin.x * tw as f32).floor() as u32).min(tw.saturating_sub(1));
        let y0 = ((uv.origin.y * th as f32).floor() as u32).min(th.saturating_sub(1));
        let w = ((uv.size.x * tw as f32).round() as u32).clamp(1, tw - x0);
        let h = ((uv.size.y * th as f32).round() as u32).clamp(1, th - y0);
        let row = (4 * w).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("playlist thumbnail read back"),
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut source = texture.as_image_copy();
        source.origin = wgpu::Origin3d { x: x0, y: y0, z: 0 };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            source,
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let data = buffer.slice(..).get_mapped_range().ok()?;
        let bgra =
            matches!(texture.format(), wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let mut pixels = Vec::with_capacity((4 * w * h) as usize);
        for y in 0..h as usize {
            let line = &data[y * row as usize..y * row as usize + 4 * w as usize];
            if bgra {
                for px in line.as_chunks::<4>().0 {
                    pixels.extend_from_slice(&[px[2], px[1], px[0], 255]);
                }
            } else {
                pixels.extend_from_slice(line);
            }
        }
        drop(data);
        let image = image::RgbaImage::from_raw(w, h, pixels)?;
        let longest = w.max(h);
        if longest <= max_px || max_px == 0 {
            return Some(image);
        }
        let k = max_px as f32 / longest as f32;
        let (nw, nh) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
        Some(image::imageops::resize(&image, nw, nh, image::imageops::FilterType::Triangle))
    }

    /// A thumbnail fetched from the network arrived (`true`: it was one of ours).
    pub(super) fn playlist_thumbnail_ready(
        &mut self,
        view: ViewId,
        url: &str,
        size: (u32, u32),
        rgba: &[u8],
    ) -> bool {
        let Some(at) = self.playlist.thumb_jobs.iter().position(|j| j.view == view && j.url == url) else {
            return false;
        };
        let job = self.playlist.thumb_jobs.remove(at);
        if let Some(image) = image::RgbaImage::from_raw(size.0, size.1, rgba.to_vec()) {
            let max = self.config.playlist.thumbnail_max_px;
            let image = if image.width().max(image.height()) > max && max > 0 {
                let k = max as f32 / image.width().max(image.height()) as f32;
                image::imageops::resize(
                    &image,
                    ((image.width() as f32 * k).round() as u32).max(1),
                    ((image.height() as f32 * k).round() as u32).max(1),
                    image::imageops::FilterType::Triangle,
                )
            } else {
                image
            };
            self.save_thumbnail(job.item, image);
        }
        true
    }

    /// The picture couldn't be fetched: take a frame of the video instead.
    pub(super) fn playlist_thumbnail_failed(&mut self, view: ViewId, url: &str) -> bool {
        let Some(at) = self.playlist.thumb_jobs.iter().position(|j| j.view == view && j.url == url) else {
            return false;
        };
        let job = self.playlist.thumb_jobs.remove(at);
        self.thumbnail_fallback(job.item, view);
        true
    }

    fn thumbnail_fallback(&mut self, item: i64, view: ViewId) {
        let Some(tab) = find_tab(&self.tabs, &self.parked, view) else { return };
        let Some(media) = &tab.media else { return };
        let sniffed = self.playlist.sniffed.get(&tab.view).map_or(&[][..], Vec::as_slice);
        let Some(c) = playlist::candidates(&tab.url, &tab.label(), media, sniffed, 200)
            .into_iter()
            .find(|c| self.playlist.item(item).is_some_and(|i| playlist::same_video(i, c)))
        else {
            return;
        };
        self.thumbnail_from_tab(item, view, &c);
    }

    /// Called from the engine pump: thumbnails that never arrived fall back to a frame.
    pub(super) fn playlist_housekeeping(&mut self) {
        let wait = Duration::from_secs_f32(self.config.playlist.thumbnail_wait_seconds.max(0.5));
        let expired: Vec<(i64, ViewId)> = self
            .playlist
            .thumb_jobs
            .iter()
            .filter(|j| j.started.elapsed() > wait)
            .map(|j| (j.item, j.view))
            .collect();
        self.playlist.thumb_jobs.retain(|j| j.started.elapsed() <= wait);
        for (item, view) in expired {
            self.thumbnail_fallback(item, view);
        }
    }

    // ---- downloads ----

    /// Downloads an item's file for offline use (a toast says why when it can't).
    pub(super) fn playlist_download(&mut self, id: i64, announce: bool) {
        let Some(item) = self.playlist.item(id).cloned() else { return };
        match playlist::plan_download(&item) {
            DownloadPlan::Done => {
                if announce {
                    self.show_toast(Icon::Download, "Already downloaded");
                }
            }
            DownloadPlan::Unavailable(note) => {
                self.set_item_download(id, dl::STREAMING, "", 0, &note);
                if announce {
                    self.show_toast(Icon::Playlist, playlist::DOWNLOAD_UNAVAILABLE_TOAST);
                }
            }
            DownloadPlan::Start(url) => {
                // Shields decide as they do for the page's own requests.
                let verdict = self.shields.policy().check_request(&ResourceRequest {
                    url: &url,
                    page_url: &item.page_url,
                    kind: ResourceKind::Media,
                });
                if verdict != axion_engine::RequestVerdict::Allow {
                    self.set_item_download(id, dl::FAILED, "", 0, "Shields blocked this download.");
                    if announce {
                        self.show_toast(Icon::Shield, "Shields blocked this download");
                    }
                    return;
                }
                // Any shown page of the normal profile carries it (its network stack).
                let Some(view) = self
                    .playlist
                    .playback
                    .as_ref()
                    .map(|p| p.view)
                    .or_else(|| self.tabs.all().iter().find(|t| !t.private).map(|t| t.view))
                else {
                    return;
                };
                let directory = playlist::download_directory(&self.config, &self.playlist.download_dir);
                if let Err(e) = std::fs::create_dir_all(&directory) {
                    tracing::warn!("creating the Playlist download folder failed: {e}");
                }
                self.playlist.downloads.insert(url.clone(), id);
                self.playlist.progress.insert(id, None);
                self.set_item_download(id, dl::QUEUED, "", 0, "");
                self.engine.start_download(view, &url, directory);
                if announce {
                    self.show_toast(Icon::Download, "Downloading…");
                }
            }
        }
    }

    fn set_item_download(&mut self, id: i64, state: &str, path: &str, size: i64, note: &str) {
        if let Some(store) = &self.store
            && let Err(e) = store.set_playlist_download(id, state, path, size, note)
        {
            tracing::warn!("saving a Playlist download failed: {e}");
        }
        self.playlist.reload(self.store.as_ref(), &self.config);
    }

    /// The engine reports a download; returns whether it is one of the Playlist's (those
    /// don't open by themselves when done).
    pub(super) fn playlist_download_update(&mut self, info: &DownloadInfo) -> bool {
        let id =
            [&info.original_url, &info.url].into_iter().find_map(|u| self.playlist.downloads.get(u).copied());
        let Some(id) = id else { return false };
        let (state, path, size, note) = playlist::download_update(info);
        let progress = (info.total_bytes > 0).then(|| info.received_bytes as f32 / info.total_bytes as f32);
        if info.state.is_finished() {
            self.playlist.downloads.retain(|_, v| *v != id);
            self.playlist.progress.remove(&id);
            self.set_item_download(id, state, &path, size, &note);
        } else {
            self.playlist.progress.insert(id, progress);
            let known = self.playlist_ui.page.row_has_bar(id);
            if let Some(item) = self.playlist.items.iter_mut().find(|i| i.id == id) {
                item.dl_state = dl::DOWNLOADING.to_owned();
                item.dl_size = size;
            }
            if !known {
                self.playlist.dirty = true;
            } else if let Some(item) = self.playlist.item(id).cloned() {
                let detail = item_detail(&item, self.playlist.progress.get(&id).copied());
                self.playlist_ui.page.update_row(
                    &mut self.ui,
                    id,
                    &detail,
                    self.playlist.progress.get(&id).copied(),
                );
            }
        }
        true
    }

    // ---- removing ----

    /// Removes items from the list; their downloaded files go too when
    /// `playlist.delete_files_on_remove` says so (never a file that was the user's own).
    pub(super) fn remove_playlist_items(&mut self, ids: &[i64]) {
        for &id in ids {
            let Some(item) = self.playlist.item(id).cloned() else { continue };
            if self.playlist.playback.as_ref().is_some_and(|p| p.item == id) {
                self.stop_playlist_playback(false);
            }
            if self.config.playlist.delete_files_on_remove {
                self.delete_item_file(&item);
            }
            if !item.thumbnail.is_empty() {
                let _ = std::fs::remove_file(&item.thumbnail);
            }
            self.playlist_ui.thumbs.forget(id);
            if let Some(store) = &self.store
                && let Err(e) = store.remove_playlist_item(id)
            {
                tracing::error!("removing from the Playlist failed: {e}");
            }
            self.playlist.selected.remove(&id);
        }
        self.playlist.reload(self.store.as_ref(), &self.config);
        self.playlist_ui.button = None;
    }

    fn delete_item_file(&self, item: &PlaylistItem) {
        let own = item.media_url.as_deref().is_some_and(|u| u.starts_with("file://"));
        if own || item.dl_path.is_empty() {
            return;
        }
        if let Err(e) = std::fs::remove_file(&item.dl_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %item.dl_path, "deleting a downloaded video failed: {e}");
        }
    }

    /// Deletes only the downloaded file, keeping the item.
    fn remove_item_download(&mut self, id: i64) {
        let Some(item) = self.playlist.item(id).cloned() else { return };
        self.delete_item_file(&item);
        let source = item.media_url.as_deref().map_or(Source::Streaming("stream"), playlist::classify);
        let (state, note) = match source {
            Source::Streaming(why) => (dl::STREAMING, why),
            _ => (dl::NONE, ""),
        };
        self.set_item_download(id, state, "", 0, note);
    }

    // ---- the page ----

    pub(super) fn playlist_open(&self) -> bool {
        self.playlist_ui.page.is_open()
    }

    pub(super) fn open_playlist(&mut self) {
        if self.playlist_open() {
            return;
        }
        if self.is_locked() {
            self.show_unlock_alert();
            return;
        }
        self.close_popups();
        self.playlist_ui.popover.close(&mut self.ui, &self.config);
        if self.overview.is_open() {
            self.close_overview(self.tabs.active_index());
        }
        if self.settings.is_open() {
            self.close_settings();
        }
        self.close_find();
        self.playlist.reload(self.store.as_ref(), &self.config);
        self.playlist.editing = false;
        self.playlist.selected.clear();
        self.playlist_ui.query.clear();
        let window = self.logical_size();
        let rect = self.chrome.playlist_rect(&self.config, window);
        let inset = self.playlist_toggle_inset();
        self.playlist_ui.inset = inset;
        self.playlist_ui.page.open(&mut self.ui, &self.config, rect, inset);
        self.chrome.set_playlist_page(&mut self.ui, &self.config, true);
        self.tabs_changed = true;
        self.order_lock_planes(self.is_locked());
        // The page below rests while the Playlist covers it.
        if let Some(view) = self.active_view() {
            self.engine.set_focus(view, false);
            self.engine.set_hidden(view, true);
            self.playlist_ui.resting = Some(view);
        }
        self.chrome.set_editing(&mut self.ui, false);
        self.playlist.dirty = true;
        self.playlist_ui.button = None;
    }

    pub(super) fn close_playlist(&mut self) {
        if !self.playlist_open() {
            return;
        }
        self.playlist_set_fullscreen(false);
        self.cancel_resolve();
        self.playlist.failure = None;
        // Picture in Picture goes on playing without the page.
        let in_pip = self
            .playlist
            .playback
            .as_ref()
            .is_some_and(|p| self.pip.as_ref().is_some_and(|w| w.is_active() && w.view == Some(p.view)));
        if !in_pip {
            self.stop_playlist_playback(true);
        }
        self.playlist_ui.page.close(&mut self.ui, &self.config);
        self.chrome.set_playlist_page(&mut self.ui, &self.config, false);
        self.tabs_changed = true;
        self.order_lock_planes(self.is_locked());
        if let Some(view) = self.playlist_ui.resting.take()
            && Some(view) == self.active_view()
        {
            self.engine.set_hidden(view, false);
            self.engine.resize(view, self.view_size());
            self.engine.set_focus(view, true);
        }
        self.playlist_ui.button = None;
        self.tabs_changed = true;
        self.update_layout();
    }

    /// Keeps clear of the toolbar's sidebar button while the sidebar is closed.
    fn playlist_toggle_inset(&self) -> f32 {
        if self.chrome.sidebar_open() { 0.0 } else { self.chrome.sidebar_button_room() }
    }

    /// The page follows the window and the sliding sidebar.
    fn place_playlist(&mut self) {
        let rect = self.chrome.playlist_rect(&self.config, self.logical_size());
        let inset = self.playlist_toggle_inset();
        if rect != self.playlist_ui.page.rect() || (inset - self.playlist_ui.inset).abs() > 0.5 {
            self.playlist_ui.inset = inset;
            self.playlist_ui.page.place(&mut self.ui, &self.config, rect, inset);
            self.playlist.dirty = true;
        }
    }

    /// The rows for the current search.
    fn playlist_rows(&mut self) -> Vec<RowModel> {
        let query = self.playlist_ui.query.clone();
        let shown = self.playlist.matching(&query);
        self.playlist_ui.shown = shown.iter().map(|&i| self.playlist.items[i].id).collect();
        let playing = self.playlist_current_item();
        let mut pending = false;
        let rows = shown
            .iter()
            .map(|&i| {
                let item = &self.playlist.items[i];
                let progress = self.playlist.progress.get(&item.id).copied();
                let thumb = self.playlist_ui.thumbs.get(item.id, &item.thumbnail, &mut pending);
                RowModel {
                    id: item.id,
                    title: item.title.clone(),
                    site_url: item.page_url.clone(),
                    detail: item_detail(item, progress),
                    thumb,
                    downloaded: item.dl_state == dl::DONE,
                    watched: item.watched,
                    playing: playing == Some(item.id),
                    selected: self.playlist.selected.contains(&item.id),
                    progress: progress
                        .filter(|_| matches!(item.dl_state.as_str(), dl::DOWNLOADING | dl::QUEUED)),
                }
            })
            .collect();
        // Thumbnails that didn't fit the frame's budget: draw again next frame.
        self.playlist.dirty = pending;
        rows
    }

    /// Everything the page shows, once per frame while it is open.
    pub(super) fn update_playlist_page(&mut self, dt: f32) {
        self.place_playlist();
        self.playlist_ui.thumbs.refill(self.config.playlist.thumbnails_per_frame.max(1));
        let query = self.ui.field_text(self.playlist_ui.page.search_field()).unwrap_or_default().to_owned();
        if query != self.playlist_ui.query {
            self.playlist_ui.query = query;
            self.playlist.dirty = true;
        }
        if self.playlist.dirty {
            let rows = self.playlist_rows();
            let favicons = &self.favicons;
            self.playlist_ui.page.set_rows(&mut self.ui, &rows, self.playlist.editing, favicons);
            let name = self.config.playlist.list_name.clone();
            let summary = playlist::summary(&self.playlist.items);
            self.playlist_ui.page.set_header(
                &mut self.ui,
                &name,
                &summary,
                self.playlist.editing,
                self.playlist.selected.len(),
            );
        }
        self.playlist_playback_tick();
        self.playlist_ui.page.animate(&mut self.ui, &self.config, dt);
    }

    /// The item the player area is about: playing, being looked for, or failed.
    fn playlist_current_item(&self) -> Option<i64> {
        self.playlist
            .playback
            .as_ref()
            .map(|p| p.item)
            .or_else(|| self.playlist.resolving.as_ref().map(|r| r.item))
            .or_else(|| self.playlist.failure.as_ref().map(|f| f.item))
    }

    /// The player half with nothing playing: idle, looking for a video, or a message with
    /// *Open page*.
    fn playlist_player_idle(&mut self) {
        let current = self.playlist_current_item().and_then(|id| self.playlist.item(id)).cloned();
        let site = current.as_ref().map(|i| display_host(&i.page_url)).unwrap_or_default();
        let (note, open_page) = if self.playlist.resolving.is_some() {
            (Some("Finding video…".to_owned()), false)
        } else if let Some(f) = &self.playlist.failure {
            (Some(f.message.clone()), true)
        } else {
            (None, false)
        };
        let model = PlayerModel {
            title: current.as_ref().map_or("", |i| i.title.as_str()),
            site: &site,
            elapsed: current.as_ref().map_or(0.0, |i| i.position),
            duration: current.as_ref().map_or(0.0, |i| i.duration),
            playing: false,
            shuffle: self.playlist.shuffle,
            repeat: self.playlist.repeat,
            rate: self.playlist.rate,
            sleep: self.playlist.sleep != Sleep::Off,
            volume: self.config.playlist.volume,
            muted: self.config.playlist.muted,
            note: note.as_deref(),
            open_page,
            video: None,
        };
        self.playlist_ui.page.set_player(&mut self.ui, &model);
        // Nothing left to show full screen.
        if self.playlist.fullscreen {
            self.playlist_set_fullscreen(false);
        }
    }

    /// The player half of the page, the saved position, the end of a video, the sleep timer.
    fn playlist_playback_tick(&mut self) {
        let now = Instant::now();
        // Sleep timer.
        if let Sleep::At(at) = self.playlist.sleep
            && now >= at
        {
            self.playlist.sleep = Sleep::Off;
            self.playlist_media(MediaAction::Pause);
        }
        self.playlist_resolve_tick();
        let fullscreen = self.playlist.fullscreen;
        let box_size =
            if fullscreen { self.page_rect().size } else { self.playlist_ui.page.video_frame(&self.ui).size };
        let item = self.playlist.playback.as_ref().and_then(|p| self.playlist.item(p.item)).cloned();
        let Some(item) = item else {
            self.playlist_player_idle();
            return;
        };
        let Some(pb) = self.playlist.playback.as_ref() else { return };
        let view = pb.view;
        let player = pb.player().cloned();
        let opened = pb.opened;
        // The view follows the video area (the window in fullscreen).
        if box_size.x > 1.0 && (box_size - self.playlist_ui.view_size).abs().max_element() > 1.0 {
            self.playlist_ui.view_size = box_size;
            self.engine.resize(view, ViewSize { logical: box_size, scale: self.scale() });
        }
        self.engine.request_frame(view);
        let frame = self.engine.frame(view);
        let picture = frame.as_ref().map(|f| {
            let tex = Vec2::new(f.texture.width() as f32, f.texture.height() as f32);
            // The view is the video on black: its picture is the video's part of the frame.
            let (aspect, uv) = match &player {
                Some(p) if p.vw > 0.0 => (p.aspect(), letterbox(tex, p.aspect())),
                _ => (tex.x / tex.y.max(1.0), Rect { origin: Vec2::ZERO, size: Vec2::ONE }),
            };
            VideoPicture {
                view: f.texture.create_view(&Default::default()),
                size: (f.texture.width(), f.texture.height()),
                uv,
                aspect,
            }
        });
        let (elapsed, duration, playing) =
            player.as_ref().map_or((item.position, item.duration, false), |p| {
                (p.time, p.duration.max(item.duration), p.is_playing())
            });
        let note = if picture.is_none() || (player.is_none() && opened.elapsed() < Duration::from_secs(1)) {
            Some("Loading…")
        } else {
            None
        };
        let model = PlayerModel {
            title: &item.title,
            site: &display_host(&item.page_url),
            elapsed,
            duration,
            playing,
            shuffle: self.playlist.shuffle,
            repeat: self.playlist.repeat,
            rate: player.as_ref().map_or(self.playlist.rate, |p| p.rate),
            sleep: self.playlist.sleep != Sleep::Off,
            volume: self.config.playlist.volume,
            muted: self.config.playlist.muted,
            note,
            open_page: false,
            video: if player.is_some() { picture } else { None },
        };
        self.playlist_ui.page.set_player(&mut self.ui, &model);

        let Some(player) = player else { return };
        // The first time the player is ready: take over (local), speed, resume, play.
        self.playlist_start_playback(view, &player);
        self.playlist_follow_playback(&item, &player, frame.as_ref());
    }

    /// What goes with a playing video: its saved position, its length, a thumbnail from its
    /// own picture, and its end.
    fn playlist_follow_playback(
        &mut self,
        item: &PlaylistItem,
        player: &axion_media::Player,
        frame: Option<&axion_engine::Frame>,
    ) {
        let now = Instant::now();
        // The saved position, now and then.
        let interval = Duration::from_secs_f32(self.config.playlist.position_save_seconds.max(0.5));
        let due = self.playlist.playback.as_ref().is_some_and(|p| p.saved_at.elapsed() >= interval);
        if due && player.is_playing() || player.ended {
            self.save_item_progress(item.id, player);
            if let Some(pb) = &mut self.playlist.playback {
                pb.saved_at = now;
            }
        }
        if player.duration > 0.0 && (item.duration - player.duration).abs() > 1.0 {
            if let Some(store) = &self.store {
                let _ = store.set_playlist_duration(item.id, player.duration);
            }
            if let Some(i) = self.playlist.items.iter_mut().find(|i| i.id == item.id) {
                i.duration = player.duration;
            }
            self.playlist.dirty = true;
        }
        // A picture for the row from the video itself (the view shows nothing else).
        let waited = self.playlist.playback.as_ref().is_some_and(|p| !p.thumb_done);
        if waited
            && item.thumbnail.is_empty()
            && player.time >= f64::from(self.config.playlist.thumbnail_frame_after_seconds)
            && player.vw > 0.0
            && let Some(f) = frame
        {
            let tex = Vec2::new(f.texture.width() as f32, f.texture.height() as f32);
            let uv = letterbox(tex, player.aspect());
            self.save_thumbnail_from_texture(item.id, &f.texture, uv);
            self.mark_thumb_done();
        } else if waited && !item.thumbnail.is_empty() {
            self.mark_thumb_done();
        }
        // The end: sleep, repeat, the next.
        let ended = player.ended && self.playlist.playback.as_ref().is_some_and(|p| !p.ended_handled);
        if ended {
            if let Some(pb) = &mut self.playlist.playback {
                pb.ended_handled = true;
            }
            self.playlist_ended(item.id);
        }
    }

    /// While Picture in Picture plays the video without the page: the position keeps being
    /// saved, and the end of the video ends it.
    pub(super) fn playlist_background_tick(&mut self) {
        let Some(pb) = self.playlist.playback.as_ref() else { return };
        let (item, player) = (pb.item, pb.player().cloned());
        let Some(player) = player else { return };
        let interval = Duration::from_secs_f32(self.config.playlist.position_save_seconds.max(0.5));
        if pb.saved_at.elapsed() >= interval && player.is_playing() || player.ended {
            self.save_item_progress(item, &player);
            if let Some(pb) = &mut self.playlist.playback {
                pb.saved_at = Instant::now();
            }
        }
        if player.ended {
            self.stop_playlist_playback(false);
            if let Some(pip) = &mut self.pip {
                pip.hide();
            }
        }
    }

    fn mark_thumb_done(&mut self) {
        if let Some(pb) = &mut self.playlist.playback {
            pb.thumb_done = true;
        }
    }

    /// Saves where `player` is in item `id`.
    fn save_item_progress(&mut self, id: i64, player: &axion_media::Player) {
        let (position, watched) = playlist::progress(player.time, player.duration, &self.config.playlist);
        let watched = watched || self.playlist.item(id).is_some_and(|i| i.watched);
        if let Some(store) = &self.store
            && let Err(e) = store.set_playlist_progress(id, position, watched)
        {
            tracing::warn!("saving the playback position failed: {e}");
        }
        if let Some(i) = self.playlist.items.iter_mut().find(|i| i.id == id) {
            if i.watched != watched {
                self.playlist.dirty = true;
            }
            i.position = position;
            i.watched = watched;
        }
    }

    /// Sends the commands a freshly loaded playback view needs, once.
    fn playlist_start_playback(&mut self, view: ViewId, player: &axion_media::Player) {
        let Some(pb) = &mut self.playlist.playback else { return };
        if pb.started || (player.duration <= 0.0 && player.src.is_empty()) {
            return;
        }
        pb.started = true;
        let (local, autoplay, seek) = (pb.local, pb.autoplay, pb.seek.take());
        let rate = self.playlist.rate;
        let id = player.id.clone();
        let mut commands = Vec::new();
        if local || self.playlist.fullscreen {
            commands.push(MediaAction::Takeover(true));
        }
        // Every Playlist video plays at the shared volume.
        commands.extend(crate::playlist::volume_actions(&self.config.playlist));
        if (rate - 1.0).abs() > 0.01 {
            commands.push(MediaAction::Rate(rate));
        }
        if let Some(seek) = seek.filter(|&s| s > 0.0) {
            commands.push(MediaAction::Seek(seek));
        }
        if autoplay {
            commands.push(MediaAction::Play);
        }
        for action in commands {
            self.media_command(view, &MediaCommand::new(&id, action));
        }
    }

    /// A video of the page's player ended.
    fn playlist_ended(&mut self, id: i64) {
        if self.playlist.sleep == Sleep::EndOfVideo {
            self.playlist.sleep = Sleep::Off;
            return;
        }
        let shown = self.playlist_ui.shown.clone();
        let current = shown.iter().position(|&i| i == id);
        let random = self.playlist.random();
        match playlist::step_index(
            shown.len(),
            current,
            true,
            self.playlist.shuffle,
            self.playlist.repeat,
            random,
        ) {
            Some(next) if Some(next) == current => {
                // The same video again.
                self.playlist_media(MediaAction::Seek(0.0));
                self.playlist_media(MediaAction::Play);
                if let Some(pb) = &mut self.playlist.playback {
                    pb.ended_handled = false;
                }
            }
            Some(next) => {
                let start = shown[next];
                self.playlist_play(start);
            }
            None => {}
        }
    }

    /// A command to the page's player.
    fn playlist_media(&mut self, action: MediaAction) {
        let Some(pb) = &self.playlist.playback else { return };
        let Some(id) = pb.player().map(|p| p.id.clone()) else { return };
        let view = pb.view;
        self.media_command(view, &MediaCommand::new(&id, action));
    }

    /// What plays in the player area for an item.
    fn playlist_target(&self, item: &PlaylistItem) -> Target {
        if let Some(file) = downloaded_file(item) {
            return Target::Local(file);
        }
        if let Some(path) = item.media_url.as_deref().and_then(playlist::path_from_file_url)
            && path.exists()
        {
            return Target::Local(path);
        }
        if let Some(source) = MediaSource::from_json(&item.source) {
            return if source.playable() {
                Target::Source(source)
            } else {
                Target::Unplayable(axion_media::WHY_DASH)
            };
        }
        // Saved before sources were captured: a plain file's address is the source.
        if let Some(url) = item.media_url.as_deref().filter(|u| u.starts_with("http"))
            && let Source::Direct(url) = playlist::classify(url)
        {
            return Target::Source(MediaSource::new(&url, axion_media::StreamKind::File, &item.page_url));
        }
        Target::Find
    }

    /// Plays an item in the page's player: its downloaded file or captured source in a media
    /// view; an item without a source gets it found first. Never the page itself.
    pub(super) fn playlist_play(&mut self, id: i64) {
        let Some(item) = self.playlist.item(id).cloned() else { return };
        self.stop_playlist_playback(true);
        self.cancel_resolve();
        self.playlist.failure = None;
        self.playlist.dirty = true;
        self.pause_resting_page();
        if let Some(at) = self.playlist_ui.shown.iter().position(|&i| i == id) {
            self.playlist_ui.page.reveal(&mut self.ui, at);
        }
        match self.playlist_target(&item) {
            Target::Local(path) => self.playlist_open_view(&item, &playlist::file_url(&path), None, true),
            Target::Source(source) => {
                let fresh = false;
                self.playlist_open_source(&item, source, fresh);
            }
            Target::Unplayable(why) => self.playlist_fail(id, why),
            Target::Find => self.playlist_resolve_start(&item),
        }
    }

    /// The page below the Playlist stops playing (its video would play along with the one
    /// the Playlist starts).
    fn pause_resting_page(&mut self) {
        let Some(view) = self.playlist_ui.resting else { return };
        let Some(player) = self.primary_player(view).filter(|p| p.is_playing()).map(|p| p.id.clone()) else {
            return;
        };
        let pause = MediaCommand::new(&player, MediaAction::Pause);
        self.engine.run_script(view, &pause.script(), false);
    }

    fn playlist_open_source(&mut self, item: &PlaylistItem, source: MediaSource, fresh: bool) {
        let token = self.media_service.register(&source);
        let url = media_view::view_url(&token);
        self.playlist_open_view(item, &url, Some((source, token)), fresh);
    }

    /// Opens the media view at `url`: a saved file, or `axion://media/` with a source.
    fn playlist_open_view(
        &mut self,
        item: &PlaylistItem,
        url: &str,
        source: Option<(MediaSource, String)>,
        fresh: bool,
    ) {
        let size = {
            let s = self.playlist_ui.page.video_frame(&self.ui).size;
            if self.playlist.fullscreen {
                self.page_rect().size
            } else if s.x > 1.0 {
                s
            } else {
                Vec2::new(640.0, 360.0)
            }
        };
        let options = MediaViewOptions {
            headers: source.as_ref().map(|(s, _)| s.headers()).unwrap_or_default(),
            page_url: item.page_url.clone(),
            allow_prefix: if source.is_some() { media_view::view_prefix() } else { url.to_owned() },
        };
        match self.engine.create_view(url, ViewSize { logical: size, scale: self.scale() }, false) {
            Ok(view) => {
                // Before anything is requested: no navigation away, the source's headers.
                self.engine.set_media_view(view, Some(options));
                self.playlist_ui.view_size = size;
                self.engine.set_hidden(view, false);
                let local = source.is_none();
                let token = source.map(|(_, token)| token);
                self.playlist.playback = Some(Playback {
                    item: item.id,
                    view,
                    media: None,
                    seek: Some(playlist::resume_position(item, &self.config.playlist)),
                    autoplay: true,
                    opened: Instant::now(),
                    thumb_done: !item.thumbnail.is_empty(),
                    local,
                    token,
                    fresh,
                    started: false,
                    saved_at: Instant::now(),
                    ended_handled: false,
                });
                self.playlist.dirty = true;
            }
            Err(e) => {
                tracing::error!("opening a video failed: {e:#}");
                self.playlist_fail(item.id, "Couldn’t open this video.");
            }
        }
    }

    /// Ends the page's playback (saving where it was).
    pub(super) fn stop_playlist_playback(&mut self, save: bool) {
        let Some(pb) = self.playlist.playback.take() else { return };
        if save && let Some(player) = pb.player() {
            self.save_item_progress(pb.item, player);
        }
        if let Some(token) = &pb.token {
            self.media_service.forget(token);
        }
        self.engine.close_view(pb.view);
        self.playlist.dirty = true;
    }

    // ---- finding the source of an item saved without one ----

    /// Loads the item's page in a view nobody sees, muted, just long enough for the bridge and
    /// the network to show where the video comes from. The player area says so meanwhile.
    fn playlist_resolve_start(&mut self, item: &PlaylistItem) {
        if !item.page_url.starts_with("http") {
            self.playlist_fail(item.id, "This video has no address to play from.");
            return;
        }
        let size = ViewSize { logical: Vec2::new(1280.0, 720.0), scale: self.scale() };
        match self.engine.create_view(&item.page_url, size, false) {
            Ok(view) => {
                self.engine.set_audio_muted(view, true);
                self.playlist.sniffed.remove(&view);
                self.playlist.resolving = Some(Resolving {
                    item: item.id,
                    view,
                    started: Instant::now(),
                    media: None,
                    unsupported: None,
                    clicked_at: None,
                });
                self.playlist.dirty = true;
            }
            Err(e) => {
                tracing::error!("opening {} to find its video failed: {e:#}", item.page_url);
                self.playlist_fail(item.id, "Couldn’t open this video’s page.");
            }
        }
    }

    pub(super) fn cancel_resolve(&mut self) {
        if let Some(r) = self.playlist.resolving.take() {
            self.engine.close_view(r.view);
            self.playlist.sniffed.remove(&r.view);
            self.playlist.dirty = true;
        }
    }

    /// Checks whether the hidden page has shown its video yet (once per frame).
    fn playlist_resolve_tick(&mut self) {
        let Some(r) = &self.playlist.resolving else { return };
        let (id, view, started) = (r.item, r.view, r.started);
        let Some(item) = self.playlist.item(id).cloned() else {
            self.cancel_resolve();
            return;
        };
        let cfg = self.config.playlist.clone();
        let sniffed = self.playlist.sniffed.get(&view).map_or(&[][..], Vec::as_slice);
        let found = playlist::resolve_capture(r.media.as_ref(), &item.page_url, sniffed);
        let now = Instant::now();
        match found {
            Capture::Found(source) => {
                self.set_item_source(id, &source);
                self.cancel_resolve();
                self.playlist_open_source(&item, source, true);
                return;
            }
            Capture::Unsupported(why) => {
                if let Some(r) = &mut self.playlist.resolving {
                    let since = r.unsupported.filter(|(_, w)| *w == why).map_or(now, |(t, _)| t);
                    r.unsupported = Some((since, why));
                }
            }
            Capture::Nothing => {
                if let Some(r) = &mut self.playlist.resolving {
                    r.unsupported = None;
                }
            }
        }
        let Some(r) = &self.playlist.resolving else { return };
        if let Some((since, why)) = r.unsupported
            && since.elapsed().as_secs_f32() >= cfg.resolve_settle_seconds
        {
            self.playlist_fail(id, why);
            return;
        }
        if started.elapsed().as_secs_f32() >= cfg.resolve_timeout_seconds {
            self.playlist_fail(id, "Couldn’t find a video on this page.");
            return;
        }
        // Site players often load their video only after a click.
        let wait = Duration::from_secs_f32(cfg.resolve_click_seconds.max(0.2));
        let due = r.clicked_at.is_none_or(|t| t.elapsed() >= wait);
        let click = r.media.as_ref().and_then(|m| {
            m.players
                .iter()
                .filter(|p| {
                    p.kind == axion_media::Kind::Video && !p.native && p.area() > 0.0 && !p.is_playing()
                })
                .max_by(|a, b| a.area().total_cmp(&b.area()))
                .map(|p| Vec2::new(p.x + p.w / 2.0, p.y + p.h / 2.0))
        });
        if due && let Some(at) = click {
            if let Some(r) = &mut self.playlist.resolving {
                r.clicked_at = Some(now);
            }
            self.click_page(view, at);
        }
    }

    /// The item can't play in the player area; it says why, with a button to open the page.
    pub(super) fn playlist_fail(&mut self, item: i64, message: &str) {
        self.cancel_resolve();
        self.playlist.failure = Some(Failure { item, message: message.to_owned() });
        self.playlist.dirty = true;
    }

    /// The media view could not play its source.
    pub(super) fn playlist_view_failed(&mut self, view: ViewId, code: &str, detail: &str) {
        let Some(pb) = self.playlist.playback.as_ref().filter(|p| p.view == view) else { return };
        let (id, fresh, local) = (pb.item, pb.fresh, pb.local);
        tracing::warn!(%code, %detail, fresh, "the Playlist's media view failed");
        self.stop_playlist_playback(true);
        let item = self.playlist.item(id).cloned();
        match item {
            // A saved link may have expired (signed addresses): look at the page once more.
            Some(item) if !fresh && !local => {
                self.playlist.failure = None;
                self.playlist_resolve_start(&item);
            }
            _ => self.playlist_fail(id, playlist::failure_text(code)),
        }
    }

    fn set_item_source(&mut self, id: i64, source: &MediaSource) {
        if let Some(store) = &self.store
            && let Err(e) = store.set_playlist_source(id, &source.to_json())
        {
            tracing::warn!("saving a Playlist item's source failed: {e}");
        }
        self.playlist.reload(self.store.as_ref(), &self.config);
    }

    // ---- engine reports ----

    /// A media response passed a view's network stack.
    pub(super) fn playlist_sniffed(&mut self, view: ViewId, url: String, mime: String, referer: String) {
        let limit = self.config.playlist.sniff_limit as usize;
        let list = self.playlist.sniffed.entry(view).or_default();
        playlist::remember_sniffed(list, Sniffed { url, mime, referer }, limit);
    }

    /// A view starts a new page: what its network showed before is no longer about it.
    pub(super) fn playlist_forget_sniffed(&mut self, view: ViewId) {
        if self.playlist.resolving.as_ref().is_some_and(|r| r.view == view) {
            return;
        }
        self.playlist.sniffed.remove(&view);
    }

    // ---- Axion's fullscreen player on the playback ----

    /// Whether `view` is the Playlist's playback view.
    pub(super) fn is_playback_view(&self, view: ViewId) -> bool {
        self.playlist.playback.as_ref().is_some_and(|p| p.view == view)
    }

    /// The media state Axion's players (fullscreen, PiP) use for a view: a tab's, or the
    /// Playlist playback's.
    pub(super) fn media_of(&self, view: ViewId) -> Option<(&PageMedia, f32)> {
        if let Some(pb) = self.playlist.playback.as_ref().filter(|p| p.view == view) {
            let media = pb.media.as_ref()?;
            return Some((media, if media.dpr > 0.0 { media.dpr } else { self.scale() }));
        }
        let media = find_tab(&self.tabs, &self.parked, view)?.media.as_ref()?;
        Some((media, if media.dpr > 0.0 { media.dpr } else { self.scale() }))
    }

    /// Axion's fullscreen player takes over the playback directly (no tab, no page), at the
    /// same position; leaving goes back to the Playlist page.
    pub(super) fn playlist_set_fullscreen(&mut self, on: bool) {
        if on == self.playlist.fullscreen {
            return;
        }
        // The controls change under the pointer: nothing stays held.
        self.release_pointer_holds();
        if on {
            if self.playlist.playback.is_none() || !self.playlist_open() {
                return;
            }
            self.playlist.fullscreen = true;
            self.chrome.set_page_fullscreen(true);
            self.playlist_ui.page.set_hidden(&mut self.ui, true);
            // The player is for the video covering its view.
            self.playlist_media(MediaAction::Takeover(true));
        } else {
            self.playlist.fullscreen = false;
            self.chrome.set_page_fullscreen(false);
            if self.playlist_open() {
                self.playlist_ui.page.set_hidden(&mut self.ui, false);
                self.playlist.dirty = true;
            }
        }
        let window = on || self.chrome.presentation();
        self.window.set_fullscreen(window.then_some(Fullscreen::Borderless(None)));
        self.update_layout();
        self.player.wake();
    }

    /// Picture in Picture for the playback: the video keeps playing in its view.
    fn playlist_picture_in_picture(&mut self) {
        let Some(pb) = &self.playlist.playback else { return };
        let (view, player) = (pb.view, pb.player().cloned());
        if let Some(player) = player {
            self.playlist_set_fullscreen(false);
            self.start_pip(view, player);
        }
    }

    /// Picture in Picture ended: its × stops the video, "back" shows the Playlist again.
    pub(super) fn playlist_pip_ended(&mut self, show: bool) {
        if show {
            self.open_playlist();
        } else if !self.playlist_open() {
            self.stop_playlist_playback(true);
        }
    }

    // ---- the page's buttons ----

    pub(super) fn playlist_action(&mut self, action: PageAction) {
        let ids = self.playlist_ui.shown.clone();
        match action {
            PageAction::Close => self.close_playlist(),
            PageAction::PlayAll => {
                if let Some(&first) = ids.first() {
                    self.playlist_play(first);
                }
            }
            PageAction::Edit => {
                self.playlist.editing = !self.playlist.editing;
                self.playlist.selected.clear();
                self.playlist.dirty = true;
            }
            PageAction::Search => {
                let open = !self.playlist_ui.page.search_open();
                let events = self.playlist_ui.page.set_search(&mut self.ui, open);
                self.handle_ui_events(events);
            }
            PageAction::PictureInPicture => self.playlist_picture_in_picture(),
            PageAction::Fullscreen => self.playlist_set_fullscreen(true),
            PageAction::OpenPage => {
                if let Some(id) = self.playlist.failure.as_ref().map(|f| f.item) {
                    self.playlist_open_in_tab(id, false);
                }
            }
            PageAction::Mute => self.playlist_volume(&[MediaAction::Mute(!self.config.playlist.muted)]),
            PageAction::Shuffle => self.playlist.shuffle = !self.playlist.shuffle,
            PageAction::Repeat => self.playlist.repeat = self.playlist.repeat.next(),
            PageAction::Previous | PageAction::Next => {
                let current =
                    self.playlist.playback.as_ref().and_then(|p| ids.iter().position(|&i| i == p.item));
                let random = self.playlist.random();
                let forward = action == PageAction::Next;
                // Wrapping at the ends is what the buttons do (repeat only matters when a
                // video ends).
                let next = playlist::step_index(
                    ids.len(),
                    current,
                    forward,
                    self.playlist.shuffle,
                    Repeat::All,
                    random,
                );
                if let Some(&id) = next.and_then(|n| ids.get(n)) {
                    self.playlist_play(id);
                }
            }
            PageAction::PlayPause => {
                if self.playlist.playback.is_some() {
                    self.playlist_media(MediaAction::Toggle);
                } else if let Some(&first) = ids.first() {
                    self.playlist_play(first);
                }
            }
            PageAction::Speed => {
                let anchor = self.playlist_ui.page.frame_of(&self.ui, PageAction::Speed);
                let mut speeds: Vec<(usize, f32)> =
                    self.config.media.playback_speeds.iter().copied().enumerate().collect();
                speeds.sort_by(|a, b| b.1.total_cmp(&a.1));
                let rate = self.playlist.rate;
                let items = speeds
                    .into_iter()
                    .map(|(i, s)| {
                        let mut item = MenuItem::new(
                            Icon::Gauge,
                            &format!("{s}×"),
                            MenuAction::Playlist(Command::Speed(i)),
                        );
                        item.checked = (f64::from(s) - rate).abs() < 0.01;
                        item
                    })
                    .collect();
                let window = self.logical_size();
                let at = self.playlist_ui.page.menu_at(&self.ui, PageAction::Speed);
                self.playlist_ui.page.open_menu(&mut self.ui, &self.config, anchor, window, items, at);
            }
            PageAction::Sleep => {
                let anchor = self.playlist_ui.page.frame_of(&self.ui, PageAction::Sleep);
                let sleep = self.playlist.sleep;
                let mut items =
                    vec![MenuItem::new(Icon::Moon, "Off", MenuAction::Playlist(Command::SleepOff))];
                items[0].checked = sleep == Sleep::Off;
                for (i, minutes) in self.config.playlist.sleep_timer_minutes.iter().enumerate() {
                    items.push(MenuItem::new(
                        Icon::History,
                        &playlist::sleep_label(*minutes),
                        MenuAction::Playlist(Command::SleepAfter(i)),
                    ));
                }
                let mut end =
                    MenuItem::new(Icon::Media, "End of video", MenuAction::Playlist(Command::SleepEnd));
                end.checked = sleep == Sleep::EndOfVideo;
                items.push(end);
                let window = self.logical_size();
                let at = self.playlist_ui.page.menu_at(&self.ui, PageAction::Sleep);
                self.playlist_ui.page.open_menu(&mut self.ui, &self.config, anchor, window, items, at);
            }
            PageAction::SelectAll => {
                if self.playlist.selected.len() == ids.len() {
                    self.playlist.selected.clear();
                } else {
                    self.playlist.selected = ids.iter().copied().collect();
                }
                self.playlist.dirty = true;
            }
            PageAction::DeleteSelected => {
                let selected: Vec<i64> =
                    ids.iter().copied().filter(|i| self.playlist.selected.contains(i)).collect();
                self.remove_playlist_items(&selected);
            }
            PageAction::DownloadSelected => {
                let selected: Vec<i64> =
                    ids.iter().copied().filter(|i| self.playlist.selected.contains(i)).collect();
                for id in selected {
                    self.playlist_download(id, false);
                }
                self.playlist.selected.clear();
                self.playlist.dirty = true;
            }
            PageAction::Row(i) => {
                if let Some(&id) = ids.get(i) {
                    if self.playlist.editing {
                        self.playlist_toggle_selected(id);
                    } else {
                        self.playlist_play(id);
                    }
                }
            }
            PageAction::RowSelect(i) => {
                if let Some(&id) = ids.get(i) {
                    self.playlist_toggle_selected(id);
                }
            }
            PageAction::RowUp(i) | PageAction::RowDown(i) => {
                if let (Some(&id), Some(store)) = (ids.get(i), self.store.as_mut()) {
                    let up = matches!(action, PageAction::RowUp(_));
                    if let Err(e) = store.move_playlist_item(id, up) {
                        tracing::warn!("moving a Playlist item failed: {e}");
                    }
                    self.playlist.reload(self.store.as_ref(), &self.config);
                }
            }
            PageAction::RowMenu(i) => {
                if let (Some(&id), Some(anchor)) = (ids.get(i), self.playlist_ui.page.row_frame(&self.ui, i))
                {
                    self.open_row_menu(id, i, anchor);
                }
            }
        }
    }

    fn playlist_toggle_selected(&mut self, id: i64) {
        if !self.playlist.selected.remove(&id) {
            self.playlist.selected.insert(id);
        }
        self.playlist.dirty = true;
    }

    fn open_row_menu(&mut self, id: i64, index: usize, row: Rect) {
        let Some(item) = self.playlist.item(id) else { return };
        let item = item.clone();
        let mut items = vec![MenuItem::new(Icon::Play, "Play", MenuAction::Playlist(Command::Play(id)))];
        let file = downloaded_file(&item);
        match playlist::plan_download(&item) {
            _ if file.is_some() => {
                items.push(MenuItem::new(
                    Icon::Trash,
                    "Remove Download",
                    MenuAction::Playlist(Command::RemoveDownload(id)),
                ));
                items.push(MenuItem::new(
                    Icon::Folder,
                    "Show in Folder",
                    MenuAction::Playlist(Command::Reveal(id)),
                ));
            }
            DownloadPlan::Start(_) => {
                items.push(MenuItem::new(
                    Icon::Download,
                    playlist::DOWNLOAD_ACTION,
                    MenuAction::Playlist(Command::Download(id)),
                ));
            }
            _ => {
                let mut stream = MenuItem::new(
                    Icon::Download,
                    playlist::DOWNLOAD_ACTION,
                    MenuAction::Playlist(Command::Download(id)),
                );
                stream.detail = Some(playlist::DOWNLOAD_UNAVAILABLE_DETAIL.to_owned());
                items.push(stream);
            }
        }
        items.push(
            MenuItem::new(Icon::Globe, "Open Page", MenuAction::Playlist(Command::OpenPage(id))).separated(),
        );
        items.push(MenuItem::new(Icon::Close, "Remove", MenuAction::Playlist(Command::Remove(id))));
        let window = self.logical_size();
        let at = self.playlist_ui.page.row_menu_at(&self.ui, index);
        self.playlist_ui.page.open_menu(&mut self.ui, &self.config, row, window, items, at);
    }

    /// A choice in one of the Playlist's menus.
    pub(super) fn playlist_command(&mut self, command: Command) {
        match command {
            Command::Play(id) => self.playlist_play(id),
            Command::Download(id) => self.playlist_download(id, true),
            Command::RemoveDownload(id) => self.remove_item_download(id),
            Command::OpenPage(id) => self.playlist_open_in_tab(id, false),
            Command::Reveal(id) => {
                if let Some(path) = self.playlist.item(id).and_then(downloaded_file) {
                    downloads::show_in_folder(&path);
                }
            }
            Command::Remove(id) => self.remove_playlist_items(&[id]),
            Command::Speed(i) => {
                if let Some(&speed) = self.config.media.playback_speeds.get(i) {
                    self.playlist.rate = f64::from(speed);
                    self.playlist_media(MediaAction::Rate(f64::from(speed)));
                }
            }
            Command::SleepOff => self.playlist.sleep = Sleep::Off,
            Command::SleepEnd => self.playlist.sleep = Sleep::EndOfVideo,
            Command::SleepAfter(i) => {
                if let Some(&minutes) = self.config.playlist.sleep_timer_minutes.get(i) {
                    self.playlist.sleep =
                        Sleep::At(Instant::now() + Duration::from_secs(u64::from(minutes) * 60));
                }
            }
            Command::Add => self.playlist_add_action(),
            Command::Show => self.open_playlist(),
        }
    }

    /// Opens an item in a real tab: its page (or, with `file`, the downloaded copy), resuming
    /// where it stopped. The only way the Playlist ever shows a video's page.
    fn playlist_open_in_tab(&mut self, id: i64, file: bool) {
        let Some(item) = self.playlist.item(id).cloned() else { return };
        let position = self
            .playlist
            .playback
            .as_ref()
            .filter(|p| p.item == id)
            .and_then(|p| p.player().map(|pl| pl.time))
            .unwrap_or_else(|| playlist::resume_position(&item, &self.config.playlist));
        let url = match downloaded_file(&item).filter(|_| file) {
            Some(path) => playlist::file_url(&path),
            None => item.page_url.clone(),
        };
        self.close_playlist();
        if self.private_mode {
            self.toggle_private_mode();
        }
        if let Err(e) = self.open_tab(&url, true) {
            tracing::error!("opening {url} failed: {e:#}");
            return;
        }
        if let Some(view) = self.active_view() {
            self.playlist.pending.insert(view, Pending { position });
        }
    }

    // ---- tabs showing saved videos ----

    /// A state report from a tab: pending resumes and the saved position of a saved video.
    pub(super) fn playlist_tab_media(&mut self, view: ViewId, media: &PageMedia) {
        let Some(player) = media.primary().cloned() else { return };
        if let Some(pending) = self.playlist.pending.get(&view).copied()
            && (player.duration > 0.0 || !player.src.is_empty())
        {
            self.playlist.pending.remove(&view);
            if pending.position > 0.0 {
                self.media_command(view, &MediaCommand::new(&player.id, MediaAction::Seek(pending.position)));
                self.media_command(view, &MediaCommand::new(&player.id, MediaAction::Play));
            }
        }
        let Some(tab) = find_tab(&self.tabs, &self.parked, view) else { return };
        if tab.private || self.store.is_none() {
            return;
        }
        let Some(item) = self.playlist.item_for_url(&tab.url).cloned() else { return };
        let interval = f64::from(self.config.playlist.position_save_seconds.max(0.5));
        let due = self.playlist.tracked.get(&view).is_none_or(|t| {
            t.item != item.id
                || (player.time - t.last_time).abs() >= interval
                    && t.saved_at.elapsed().as_secs_f64() >= interval
        });
        if due && (player.is_playing() || player.ended) && player.time > 0.0 {
            self.save_item_progress(item.id, &player);
            self.playlist.tracked.insert(
                view,
                playlist::Tracked { item: item.id, saved_at: Instant::now(), last_time: player.time },
            );
        }
    }

    /// A tab closes or leaves its page: the position it reached is kept.
    pub(super) fn playlist_flush_tab(&mut self, view: ViewId) {
        let Some(tracked) = self.playlist.tracked.remove(&view) else { return };
        let player =
            find_tab(&self.tabs, &self.parked, view).and_then(|t| t.media.as_ref()?.primary().cloned());
        if let Some(player) = player {
            self.save_item_progress(tracked.item, &player);
        }
    }

    /// A message from the bridge of a view the Playlist owns (the playback, or the hidden page
    /// being searched); `true` if it was one of those.
    pub(super) fn playlist_playback_media(&mut self, view: ViewId, media: &PageMedia) -> bool {
        if let Some(pb) = self.playlist.playback.as_mut().filter(|p| p.view == view) {
            pb.media = Some(media.clone());
            return true;
        }
        if let Some(r) = self.playlist.resolving.as_mut().filter(|r| r.view == view) {
            r.media = Some(media.clone());
            return true;
        }
        false
    }

    /// Whether the Playlist owns the view (its new tabs and popups are refused).
    pub(super) fn playlist_owns_view(&self, view: ViewId) -> bool {
        self.is_playback_view(view) || self.playlist.resolving.as_ref().is_some_and(|r| r.view == view)
    }

    /// In fullscreen the pointer only wakes the controls; the video has no page to click.
    pub(super) fn playlist_pointer(
        &mut self,
        p: Vec2,
        _button: Option<(winit::event::MouseButton, bool)>,
    ) -> bool {
        if !self.playlist.fullscreen {
            return false;
        }
        // Anything the controls cover (buttons, the sliders) takes the pointer.
        if self.ui.covers(p) {
            return false;
        }
        self.player.wake();
        true
    }

    /// Media keys while the page is open and nothing has the keyboard focus.
    pub(super) fn playlist_media_key(&mut self, action: Action) {
        let skip = f64::from(self.config.media.skip_seconds);
        match action {
            Action::MediaPlayPause => self.playlist_action(PageAction::PlayPause),
            Action::MediaFullscreen => self.playlist_set_fullscreen(!self.playlist.fullscreen),
            Action::MediaSeekBack => self.playlist_media(MediaAction::Skip(-skip)),
            Action::MediaSeekForward => self.playlist_media(MediaAction::Skip(skip)),
            Action::MediaMute => {
                let muted = self.playlist.playback.as_ref().and_then(|p| p.player()).is_some_and(|p| p.muted);
                self.playlist_media(MediaAction::Mute(!muted));
            }
            _ => {}
        }
    }

    /// The page's volume slider moved: the shared volume follows (taking it up from muted
    /// unmutes), on the playing video too.
    pub(super) fn playlist_set_volume(&mut self, value: f32) {
        self.playlist_ui.page.volume_adjusted();
        let muted = self.config.playlist.muted;
        let mut actions = vec![MediaAction::Volume(f64::from(value.clamp(0.0, 1.0)))];
        if muted && value > 0.0 {
            actions.push(MediaAction::Mute(false));
        }
        self.playlist_volume(&actions);
    }

    /// Sets the Playlist's shared volume / mute state, and the playing video to it.
    fn playlist_volume(&mut self, actions: &[MediaAction]) {
        for action in actions {
            if crate::playlist::remember_volume(&mut self.config.playlist, action) {
                self.config_dirty_since.get_or_insert(Instant::now());
            }
            self.playlist_media(*action);
        }
    }

    /// The scrubber moved.
    pub(super) fn playlist_seek(&mut self, value: f32) {
        let duration = self
            .playlist
            .playback
            .as_ref()
            .and_then(|p| p.player().map(|pl| pl.duration))
            .unwrap_or_default();
        if duration > 0.0 {
            self.playlist_ui.page.scrubbed();
            self.playlist_media(MediaAction::Seek(f64::from(value) * duration));
        }
    }

    /// Esc: closes a menu, the search, or the page. Returns whether it did something.
    pub(super) fn playlist_escape(&mut self) -> bool {
        if !self.playlist_open() {
            return false;
        }
        if self.playlist.fullscreen {
            self.playlist_set_fullscreen(false);
        } else if self.playlist_ui.page.menu().is_open() {
            self.playlist_ui.page.menu().close(&mut self.ui, &self.config);
        } else if self.playlist_ui.page.search_open() {
            let events = self.playlist_ui.page.set_search(&mut self.ui, false);
            self.handle_ui_events(events);
        } else if self.playlist.editing {
            self.playlist.editing = false;
            self.playlist.selected.clear();
            self.playlist.dirty = true;
        } else {
            self.close_playlist();
        }
        true
    }
}
