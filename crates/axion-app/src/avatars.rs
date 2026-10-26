//! Account profile pictures in the browser: decoded once into textures (kept until the picture
//! changes), and the desktop's file chooser to pick one. The files themselves are stored by
//! `axion-accounts`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use axion_i18n::tr;
use crate::downloads::{Choose, choose_portal, spawn};
use crate::home::Image;

/// Decoded profile pictures as textures, by account id.
pub struct Avatars {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// `None`: the account has no (readable) picture, so the lookup isn't repeated per frame.
    cache: RefCell<HashMap<String, Option<Image>>>,
}

impl Avatars {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self { device, queue, cache: RefCell::default() }
    }

    /// The texture of account `id`'s picture; `file` is looked up (and decoded) only the first
    /// time after [`Self::forget`].
    pub fn get(&self, id: &str, file: impl FnOnce() -> Option<PathBuf>) -> Option<Image> {
        if let Some(cached) = self.cache.borrow().get(id) {
            return cached.clone();
        }
        let image = file().and_then(|path| self.load(&path));
        self.cache.borrow_mut().insert(id.to_owned(), image.clone());
        image
    }

    /// The picture of `id` changed or was removed: decode it again next time.
    pub fn forget(&self, id: &str) {
        self.cache.borrow_mut().remove(id);
    }

    fn load(&self, path: &Path) -> Option<Image> {
        let picture = match image::open(path) {
            Ok(picture) => picture.to_rgba8(),
            Err(e) => {
                tracing::warn!(path = %path.display(), "reading the profile picture failed: {e}");
                return None;
            }
        };
        Some(self.upload(&picture))
    }

    /// A texture of pixels that are in memory already (the picture chosen in the Add Account
    /// sheet, before the account exists).
    pub fn upload(&self, picture: &image::RgbaImage) -> Image {
        let (width, height) = picture.dimensions();
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("profile picture"),
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
            picture.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            size,
        );
        (texture.create_view(&Default::default()), (width, height))
    }
}

/// Where a picture chosen in the file chooser goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhotoFor {
    /// The current account (Account Settings).
    Current,
    /// The account the Add Account sheet is about to create.
    NewAccount,
}

/// The desktop's file chooser for a profile picture; the answer is taken with [`Self::poll`].
#[derive(Default)]
pub struct PhotoChooser {
    chosen: Arc<Mutex<Option<(PhotoFor, PathBuf)>>>,
}

impl PhotoChooser {
    pub fn choose(&self, target: PhotoFor) {
        let chosen = Arc::clone(&self.chosen);
        spawn("choose-photo", move || match choose_portal(tr("Profile Photo"), Choose::Photo) {
            Ok(Some(path)) => *chosen.lock().unwrap_or_else(PoisonError::into_inner) = Some((target, path)),
            Ok(None) => {}
            Err(e) => tracing::warn!("file chooser unavailable: {e}"),
        });
    }

    pub fn poll(&self) -> Option<(PhotoFor, PathBuf)> {
        self.chosen.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}
