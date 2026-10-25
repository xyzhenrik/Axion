//! Measures colours of a page without stalling the frame: the GPU averages a region into a
//! small grid of pixels (one pixel for the luminance probe, 32×32 for the page palette), which
//! is read back asynchronously a frame or two later.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use bytemuck::{Pod, Zeroable};

use crate::Rect;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Read-back states.
const IDLE: u8 = 0;
const PENDING: u8 = 1;
const READY: u8 = 2;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ProbeUniform {
    region: [f32; 4],
    /// x: cells per side of the output grid.
    cells: [f32; 4],
}

pub(crate) struct LuminanceProbe {
    /// Cells per side of the output grid.
    cells: u32,
    /// Bytes per row of the read-back buffer.
    row_bytes: u32,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    target: wgpu::Texture,
    readback: wgpu::Buffer,
    state: Arc<AtomicU8>,
    latest: Option<[f32; 3]>,
}

impl LuminanceProbe {
    /// A probe that averages a region into `cells` × `cells` colours.
    pub fn new(device: &wgpu::Device, cells: u32) -> Self {
        let cells = cells.max(1);
        let row_bytes = (4 * cells).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("probe.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/probe.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("probe"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("probe"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("probe"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe"),
            size: size_of::<ProbeUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("probe"),
            size: wgpu::Extent3d { width: cells, height: cells, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe readback"),
            size: u64::from(row_bytes * cells),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            cells,
            row_bytes,
            pipeline,
            layout,
            uniform,
            sampler,
            target,
            readback,
            state: Arc::default(),
            latest: None,
        }
    }

    /// Collects a finished measurement, then starts a new one on `texture` if none is in flight.
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::TextureView,
        region: Rect,
    ) {
        if let Some(cells) = self.poll(device) {
            self.latest = cells.first().map(|c| c.map(|c| c as f32 / 255.0));
        }
        self.start(device, queue, texture, region);
    }

    /// The finished measurement (row by row, sRGB), once; `None` while none is ready.
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<Vec<[u8; 3]>> {
        let _ = device.poll(wgpu::PollType::Poll);
        if self.state.load(Ordering::Acquire) != READY {
            return None;
        }
        let cells = self.readback.slice(..).get_mapped_range().ok().map(|data| {
            let mut out = Vec::with_capacity((self.cells * self.cells) as usize);
            for y in 0..self.cells as usize {
                let row = &data[y * self.row_bytes as usize..];
                for x in 0..self.cells as usize {
                    out.push([row[4 * x], row[4 * x + 1], row[4 * x + 2]]);
                }
            }
            out
        });
        self.readback.unmap();
        self.state.store(IDLE, Ordering::Release);
        cells
    }

    /// Starts measuring `region` (0–1) of `texture`, unless a measurement is in flight.
    /// Returns whether it started.
    pub fn start(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::TextureView,
        region: Rect,
    ) -> bool {
        if self.state.load(Ordering::Acquire) != IDLE {
            return false;
        }
        let uniform = ProbeUniform {
            region: [region.origin.x, region.origin.y, region.size.x, region.size.y],
            cells: [self.cells as f32, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("probe"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(texture) },
            ],
        });
        let view = self.target.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("probe"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.row_bytes),
                    rows_per_image: Some(self.cells),
                },
            },
            wgpu::Extent3d { width: self.cells, height: self.cells, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);
        self.state.store(PENDING, Ordering::Release);
        let state = self.state.clone();
        self.readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            state.store(if result.is_ok() { READY } else { IDLE }, Ordering::Release);
        });
        true
    }

    pub fn latest(&self) -> Option<[f32; 3]> {
        self.latest
    }
}
