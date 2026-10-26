//! Chromium's shared GPU frames (dma-bufs), imported into our Vulkan device once per buffer and
//! copied without waiting for the GPU.
//!
//! Chromium draws into a small pool of dma-bufs and hands each frame to `on_accelerated_paint`.
//! Importing a buffer (a Vulkan image bound to its memory) for every frame and waiting on the
//! CPU until the copy was done blocked Chromium's UI thread (our main thread) for 1.5–3 ms per
//! frame; Chromium then held back the page's next frame and pages ran at half their frame rate.
//! So each buffer is imported once and kept (keyed by the dma-buf's identity, which we keep
//! alive with a duplicate of its fd), and a frame is only *submitted* to be copied: the copy and
//! a check that the frame is drawn finish on the GPU while Chromium goes on.

use std::os::fd::{AsRawFd, BorrowedFd, IntoRawFd, OwnedFd};

use ash::vk;
use cef::AcceleratedPaintInfo;
use cef::sys::cef_color_type_t;
use wgpu::hal::api::Vulkan;

/// Imported buffers kept per view. Chromium's pool holds a few buffers per size; more than this
/// means the pool was replaced (resize) and the least recently used ones are dropped.
const MAX_IMPORTS: usize = 6;

/// The wgpu state an imported buffer is left in between copies (GENERAL layout, owned by the
/// foreign queue family).
const EXTERNAL_STATE: wgpu::TextureUses = wgpu::TextureUses::COPY_SRC.union(wgpu::TextureUses::RESOURCE);

#[derive(Debug, thiserror::Error)]
pub(super) enum ImportError {
    #[error("invalid dma-buf: {0}")]
    Invalid(String),
    #[error("the device can't import dma-bufs: {0}")]
    Unsupported(&'static str),
    #[error("Vulkan: {0} failed: {1:?}")]
    Vulkan(&'static str, vk::Result),
}

/// Identifies a shared buffer: the dma-buf (device and inode of its file) and how it is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BufferKey {
    dev: u64,
    ino: u64,
    width: u32,
    height: u32,
    format: u32,
    modifier: u64,
    stride: u32,
    offset: u64,
}

struct Imported {
    key: BufferKey,
    texture: wgpu::Texture,
    /// Keeps the dma-buf open while it is imported, so its inode can't be reused for another one.
    _fd: OwnedFd,
    last_used: u64,
}

#[derive(Default)]
pub(super) struct ImportCache {
    entries: Vec<Imported>,
    clock: u64,
}

impl ImportCache {
    /// The frame's buffer as a texture of our device (imported on first use).
    ///
    /// # Safety
    /// Must be called from `on_accelerated_paint` for `info`, with a device created with
    /// `VULKAN_EXTERNAL_MEMORY_DMA_BUF` (see `required_features`).
    pub unsafe fn texture(
        &mut self,
        device: &wgpu::Device,
        info: &AcceleratedPaintInfo,
    ) -> Result<wgpu::Texture, ImportError> {
        let key = key_of(info)?;
        self.clock += 1;
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
            entry.last_used = self.clock;
            return Ok(entry.texture.clone());
        }
        let fd = borrow_fd(info.planes[0].fd)?
            .try_clone_to_owned()
            .map_err(|e| ImportError::Invalid(format!("duplicating the fd: {e}")))?;
        // SAFETY: forwarded from the caller; `fd` is the live dma-buf of this frame.
        let texture = unsafe { import(device, info, &key, &fd) }?;
        // Buffers of another size belong to a pool Chromium has replaced.
        self.entries.retain(|e| (e.key.width, e.key.height) == (key.width, key.height));
        if self.entries.len() >= MAX_IMPORTS
            && let Some(oldest) =
                self.entries.iter().enumerate().min_by_key(|(_, e)| e.last_used).map(|(i, _)| i)
        {
            self.entries.swap_remove(oldest);
        }
        self.entries.push(Imported { key, texture: texture.clone(), _fd: fd, last_used: self.clock });
        Ok(texture)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// How many shared buffers are imported.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn borrow_fd(fd: i32) -> Result<BorrowedFd<'static>, ImportError> {
    if fd < 0 {
        return Err(ImportError::Invalid("negative fd".into()));
    }
    // SAFETY: CEF keeps the plane fds open for the duration of `on_accelerated_paint`, and the
    // borrow is only used within it (to stat or duplicate the fd).
    Ok(unsafe { BorrowedFd::borrow_raw(fd) })
}

fn key_of(info: &AcceleratedPaintInfo) -> Result<BufferKey, ImportError> {
    let planes = info.planes.get(..info.plane_count.max(0) as usize).unwrap_or_default();
    // Single-plane RGBA/BGRA is what Chromium shares; anything else goes the safe way.
    let [plane] = planes else {
        return Err(ImportError::Invalid(format!("{} planes", planes.len())));
    };
    let fd = borrow_fd(plane.fd)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `fd` is open (see `borrow_fd`) and `stat` is a valid out-pointer.
    if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(ImportError::Invalid(format!("fstat: {}", std::io::Error::last_os_error())));
    }
    // SAFETY: fstat succeeded, so it filled `stat`.
    let stat = unsafe { stat.assume_init() };
    let (width, height) = (info.extra.coded_size.width, info.extra.coded_size.height);
    if width <= 0 || height <= 0 || plane.stride == 0 {
        return Err(ImportError::Invalid(format!("{width}x{height}, stride {}", plane.stride)));
    }
    Ok(BufferKey {
        dev: stat.st_dev,
        ino: stat.st_ino,
        width: width as u32,
        height: height as u32,
        format: *info.format.as_ref() as u32,
        modifier: info.modifier,
        stride: plane.stride,
        offset: plane.offset,
    })
}

fn formats(format: cef_color_type_t) -> Result<(wgpu::TextureFormat, vk::Format), ImportError> {
    match format {
        cef_color_type_t::CEF_COLOR_TYPE_BGRA_8888 => {
            Ok((wgpu::TextureFormat::Bgra8Unorm, vk::Format::B8G8R8A8_UNORM))
        }
        cef_color_type_t::CEF_COLOR_TYPE_RGBA_8888 => {
            Ok((wgpu::TextureFormat::Rgba8Unorm, vk::Format::R8G8B8A8_UNORM))
        }
        _ => Err(ImportError::Unsupported("colour type")),
    }
}

/// Imports the dma-buf as a Vulkan image owned by wgpu (destroyed, and its memory freed, when
/// the texture is dropped).
///
/// # Safety
/// `fd` must be the frame's dma-buf described by `info` and `key`; see [`ImportCache::texture`].
unsafe fn import(
    device: &wgpu::Device,
    info: &AcceleratedPaintInfo,
    key: &BufferKey,
    fd: &OwnedFd,
) -> Result<wgpu::Texture, ImportError> {
    if !device.features().contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF) {
        return Err(ImportError::Unsupported("VULKAN_EXTERNAL_MEMORY_DMA_BUF is not enabled"));
    }
    let limit = device.limits().max_texture_dimension_2d;
    if key.width > limit || key.height > limit {
        return Err(ImportError::Invalid(format!("{}x{} exceeds the device limit", key.width, key.height)));
    }
    let (wgpu_format, vk_format) = formats(*info.format.as_ref())?;
    let size = wgpu::Extent3d { width: key.width, height: key.height, depth_or_array_layers: 1 };

    // SAFETY: the hal device is only used to create and bind the image below.
    let hal_device = unsafe { device.as_hal::<Vulkan>() }.ok_or(ImportError::Unsupported("not Vulkan"))?;
    let raw = hal_device.raw_device();

    let layouts = [vk::SubresourceLayout {
        offset: key.offset,
        row_pitch: u64::from(key.stride),
        ..Default::default()
    }];
    let mut modifier = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
        .drm_format_modifier(key.modifier)
        .plane_layouts(&layouts);
    let mut external = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk_format)
        .extent(vk::Extent3D { width: key.width, height: key.height, depth: 1 })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .push_next(&mut modifier)
        .push_next(&mut external);
    // SAFETY: valid create info; the image is destroyed below on failure or owned by wgpu.
    let image = unsafe { raw.create_image(&image_info, None) }
        .map_err(|e| ImportError::Vulkan("vkCreateImage", e))?;
    let destroy_image = |raw: &ash::Device| {
        // SAFETY: the image was created above, has no memory bound by wgpu and isn't in use.
        unsafe { raw.destroy_image(image, None) }
    };

    // SAFETY: `image` is a valid image of this device.
    let requirements = unsafe { raw.get_image_memory_requirements(image) };
    let import_fd = match fd.try_clone() {
        Ok(fd) => fd,
        Err(e) => {
            destroy_image(raw);
            return Err(ImportError::Invalid(format!("duplicating the fd: {e}")));
        }
    };
    let external_fd =
        ash::khr::external_memory_fd::Device::new(hal_device.shared_instance().raw_instance(), raw);
    let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
    // SAFETY: `import_fd` is an open dma-buf fd.
    if let Err(e) = unsafe {
        external_fd.get_memory_fd_properties(
            vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
            import_fd.as_raw_fd(),
            &mut fd_properties,
        )
    } {
        destroy_image(raw);
        return Err(ImportError::Vulkan("vkGetMemoryFdPropertiesKHR", e));
    }
    // SAFETY: plain query of the physical device.
    let memory_properties = unsafe {
        hal_device
            .shared_instance()
            .raw_instance()
            .get_physical_device_memory_properties(hal_device.raw_physical_device())
    };
    let bits = requirements.memory_type_bits & fd_properties.memory_type_bits;
    let Some(memory_type) = (0..memory_properties.memory_type_count).find(|i| bits & (1 << i) != 0) else {
        destroy_image(raw);
        return Err(ImportError::Unsupported("no memory type for the dma-buf"));
    };
    let mut import_info = vk::ImportMemoryFdInfoKHR::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
        .fd(import_fd.as_raw_fd());
    let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type)
        .push_next(&mut import_info)
        .push_next(&mut dedicated);
    // SAFETY: valid allocate info importing `import_fd`; on success Vulkan owns that fd.
    let memory = match unsafe { raw.allocate_memory(&allocate_info, None) } {
        Ok(memory) => {
            let _ = import_fd.into_raw_fd();
            memory
        }
        Err(e) => {
            destroy_image(raw);
            return Err(ImportError::Vulkan("importing the dma-buf memory", e));
        }
    };
    // SAFETY: `memory` was allocated for `image` (dedicated) above.
    if let Err(e) = unsafe { raw.bind_image_memory(image, memory, 0) } {
        destroy_image(raw);
        // SAFETY: `memory` is unused after the failed bind.
        unsafe { raw.free_memory(memory, None) };
        return Err(ImportError::Vulkan("vkBindImageMemory", e));
    }
    let descriptor = |label| wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu_format,
        usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    };
    // SAFETY: `image` is bound to `memory`; wgpu-hal takes ownership of both (Dedicated) and
    // frees them when the texture is dropped.
    let hal_texture = unsafe {
        hal_device.texture_from_raw(
            image,
            &wgpu::hal::TextureDescriptor {
                label: Some("chromium shared frame"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu_format,
                usage: wgpu::TextureUses::COPY_SRC | wgpu::TextureUses::RESOURCE,
                memory_flags: wgpu::hal::MemoryFlags::empty(),
                view_formats: vec![],
            },
            None,
            wgpu::hal::vulkan::TextureMemory::Dedicated(memory),
        )
    };
    drop(hal_device);
    // SAFETY: the hal texture was created from this device; it starts in GENERAL layout.
    Ok(unsafe {
        device.create_texture_from_hal::<Vulkan>(
            hal_texture,
            &descriptor("chromium shared frame"),
            EXTERNAL_STATE,
        )
    })
}

/// Records copying `region` of the shared `source` to `dest` (at its origin) into one command
/// buffer list, with the queue-family ownership transfers Vulkan needs for foreign images.
pub(super) fn copy_commands(
    device: &wgpu::Device,
    source: &wgpu::Texture,
    region: (wgpu::Origin3d, wgpu::Extent3d),
    dest: &wgpu::Texture,
    extra: impl FnOnce(&mut wgpu::CommandEncoder),
) -> Option<[wgpu::CommandBuffer; 3]> {
    // SAFETY: only reads the queue family setup and the raw image handle.
    let (raw_device, local_family, foreign_family, image) = unsafe {
        let hal_device = device.as_hal::<Vulkan>()?;
        let hal_texture = source.as_hal::<Vulkan>()?;
        let foreign =
            if hal_device.enabled_device_extensions().contains(&ash::ext::queue_family_foreign::NAME) {
                vk::QUEUE_FAMILY_FOREIGN_EXT
            } else {
                vk::QUEUE_FAMILY_EXTERNAL
            };
        (hal_device.raw_device().clone(), hal_device.queue_family_index(), foreign, hal_texture.raw_handle())
    };
    let acquire = ownership_transfer(device, &raw_device, image, foreign_family, local_family, true)?;
    let mut copy =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("chromium frame copy") });
    copy.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: source,
            mip_level: 0,
            origin: region.0,
            aspect: wgpu::TextureAspect::All,
        },
        dest.as_image_copy(),
        region.1,
    );
    copy.transition_resources(
        std::iter::empty(),
        std::iter::once(wgpu::TextureTransition { texture: source, selector: None, state: EXTERNAL_STATE }),
    );
    extra(&mut copy);
    let release = ownership_transfer(device, &raw_device, image, local_family, foreign_family, false)?;
    Some([acquire, copy.finish(), release])
}

fn ownership_transfer(
    device: &wgpu::Device,
    raw_device: &ash::Device,
    image: vk::Image,
    from: u32,
    to: u32,
    acquire: bool,
) -> Option<wgpu::CommandBuffer> {
    let mut encoder = device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("chromium frame ownership") });
    let barrier = vk::ImageMemoryBarrier::default()
        .image(image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })
        .src_queue_family_index(from)
        .dst_queue_family_index(to)
        // GENERAL stays GENERAL, matching wgpu's tracking of EXTERNAL_STATE.
        .old_layout(vk::ImageLayout::GENERAL)
        .new_layout(vk::ImageLayout::GENERAL)
        .src_access_mask(if acquire { vk::AccessFlags::empty() } else { vk::AccessFlags::MEMORY_READ })
        .dst_access_mask(if acquire { vk::AccessFlags::MEMORY_READ } else { vk::AccessFlags::empty() });
    let (src_stage, dst_stage) = if acquire {
        (vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::ALL_COMMANDS)
    } else {
        (vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::BOTTOM_OF_PIPE)
    };
    // SAFETY: records one barrier on the encoder's open Vulkan command buffer; `image` is alive
    // (the caller holds its texture).
    let recorded = unsafe {
        encoder.as_hal_mut::<Vulkan, _, _>(|hal| {
            let hal = hal?;
            raw_device.cmd_pipeline_barrier(
                hal.raw_handle(),
                src_stage,
                dst_stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
            Some(())
        })
    };
    recorded.map(|()| encoder.finish())
}
