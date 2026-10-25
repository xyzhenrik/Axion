//! Matching Chromium's GPU to ours, so its frames can be imported without a copy across GPUs.

use std::path::PathBuf;

/// The DRM render node (`/dev/dri/renderD*`) of the GPU with these PCI ids, as reported by
/// `wgpu::AdapterInfo::{vendor, device}`.
pub fn render_node_for(vendor: u32, device: u32) -> Option<PathBuf> {
    let read_id = |path: PathBuf| -> Option<u32> {
        let text = std::fs::read_to_string(path).ok()?;
        u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
    };
    let entries = std::fs::read_dir("/sys/class/drm").ok()?;
    entries.flatten().find_map(|entry| {
        let name = entry.file_name().into_string().ok()?;
        if !name.starts_with("renderD") {
            return None;
        }
        let dev = entry.path().join("device");
        (read_id(dev.join("vendor"))? == vendor && read_id(dev.join("device"))? == device)
            .then(|| PathBuf::from("/dev/dri").join(name))
    })
}
