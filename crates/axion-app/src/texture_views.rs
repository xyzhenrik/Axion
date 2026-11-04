//! Texture views kept per texture.
//!
//! A frame that is skipped as unchanged (`ui.skip_unchanged_frames`) is told apart by its
//! scene's fingerprint, which compares image views by identity. A view created anew every frame
//! for the same texture makes every frame look changed: a static tab overview was composited
//! and presented at the display's full rate. Views handed out from here stay the same object
//! for as long as their texture does.

use std::collections::HashMap;
use std::hash::Hash;

pub struct ViewCache<K> {
    views: HashMap<K, (wgpu::Texture, wgpu::TextureView)>,
}

impl<K> Default for ViewCache<K> {
    fn default() -> Self {
        Self { views: HashMap::new() }
    }
}

impl<K: Eq + Hash + Copy> ViewCache<K> {
    /// The view of `texture` for `key`: the one handed out before while the texture is the
    /// same, else a new one (which replaces it).
    pub fn view(&mut self, key: K, texture: &wgpu::Texture) -> wgpu::TextureView {
        match self.views.get(&key) {
            Some((kept, view)) if kept == texture => view.clone(),
            _ => {
                let view = texture.create_view(&Default::default());
                self.views.insert(key, (texture.clone(), view.clone()));
                view
            }
        }
    }

    /// Lets go of every view (and the textures they hold).
    pub fn clear(&mut self) {
        self.views.clear();
    }
}
