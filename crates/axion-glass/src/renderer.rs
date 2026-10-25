use bytemuck::{Pod, Zeroable};

use crate::params::{GlassParams, ParamsUniform};
use crate::shape::{GlassShape, ShapeGpu};

/// Maximum number of glass shapes drawn in one pass.
pub const MAX_SHAPES: usize = 32;

/// The frosted backdrop is blurred at 1/BLUR_DOWNSAMPLE resolution.
const BLUR_DOWNSAMPLE: u32 = 4;
const BLUR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Number of taps on each side in `blur.wgsl`; the step is chosen so they span the radius.
const BLUR_TAPS: f32 = 6.0;

/// Inputs for one glass composite.
pub struct GlassFrame<'a> {
    /// Everything that lies beneath the glass, at full resolution.
    pub backdrop: &'a wgpu::TextureView,
    pub backdrop_size: (u32, u32),
    /// Where the composited result goes (usually the swapchain image). Same size as the backdrop.
    pub target: &'a wgpu::TextureView,
    pub shapes: &'a [GlassShape],
    pub params: &'a GlassParams,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlurUniform {
    step_uv: [f32; 2],
    _pad: [f32; 2],
}

#[derive(Clone)]
struct BlurTargets {
    size: (u32, u32),
    horizontal: wgpu::TextureView,
    vertical: wgpu::TextureView,
}

pub struct GlassRenderer {
    composite_pipeline: wgpu::RenderPipeline,
    composite_layout: wgpu::BindGroupLayout,
    blur_pipeline: wgpu::RenderPipeline,
    blur_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    blur_targets: Option<BlurTargets>,
    warned_overflow: bool,
}

impl GlassRenderer {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let fragment_texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let fragment_sampler = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };

        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glass composite"),
            entries: &[
                uniform(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                fragment_sampler(2),
                fragment_texture(3),
                fragment_texture(4),
            ],
        });
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glass blur"),
            entries: &[uniform(0), fragment_sampler(1), fragment_texture(2)],
        });

        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glass.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/glass.wgsl").into()),
        });
        let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blur.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/blur.wgsl").into()),
        });

        let composite_pipeline = fullscreen_pipeline(
            device,
            "glass composite",
            &composite_layout,
            &composite_shader,
            target_format,
        );
        let blur_pipeline =
            fullscreen_pipeline(device, "glass blur", &blur_layout, &blur_shader, BLUR_FORMAT);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glass"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            composite_pipeline,
            composite_layout,
            blur_pipeline,
            blur_layout,
            sampler,
            blur_targets: None,
            warned_overflow: false,
        }
    }

    /// Records the blur and composite passes into `encoder`.
    ///
    /// Each call gets its own parameter buffers: several glass layers in one frame are recorded
    /// into the same command buffer, and shared buffers would all end up with the last values.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: GlassFrame<'_>,
    ) {
        use wgpu::util::DeviceExt;

        let (width, height) = frame.backdrop_size;
        let resolution = [width as f32, height as f32];

        let shapes = if frame.shapes.len() > MAX_SHAPES {
            if !self.warned_overflow {
                tracing::warn!(
                    count = frame.shapes.len(),
                    max = MAX_SHAPES,
                    "too many glass shapes; extra ones are skipped"
                );
                self.warned_overflow = true;
            }
            &frame.shapes[..MAX_SHAPES]
        } else {
            frame.shapes
        };
        let mut gpu_shapes: Vec<ShapeGpu> = shapes.iter().map(|s| s.to_gpu()).collect();
        let count = gpu_shapes.len() as u32;
        if gpu_shapes.is_empty() {
            // Storage bindings can't be empty; the shader ignores it (count is 0).
            gpu_shapes.push(GlassShape::circle(glam::Vec2::ZERO, 0.0).to_gpu());
        }
        let buffer = |label: &str, contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        let shapes_buffer =
            buffer("glass shapes", bytemuck::cast_slice(&gpu_shapes), wgpu::BufferUsages::STORAGE);
        let uniform = ParamsUniform::new(frame.params, resolution, count);
        let params_buffer = buffer("glass params", bytemuck::bytes_of(&uniform), wgpu::BufferUsages::UNIFORM);
        let step = frame.params.blur_radius.max(0.0) / BLUR_TAPS;
        let blur_buffers = [
            BlurUniform { step_uv: [step / resolution[0], 0.0], _pad: [0.0; 2] },
            BlurUniform { step_uv: [0.0, step / resolution[1]], _pad: [0.0; 2] },
        ]
        .map(|u| buffer("glass blur", bytemuck::bytes_of(&u), wgpu::BufferUsages::UNIFORM));

        let targets = self.blur_targets(device, (width, height));
        let blur_h = self.blur_bind_group(device, &blur_buffers[0], frame.backdrop);
        let blur_v = self.blur_bind_group(device, &blur_buffers[1], &targets.horizontal);
        let composite = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glass composite"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: shapes_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(frame.backdrop),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&targets.vertical),
                },
            ],
        });

        fullscreen_pass(encoder, "glass blur h", &targets.horizontal, &self.blur_pipeline, &blur_h);
        fullscreen_pass(encoder, "glass blur v", &targets.vertical, &self.blur_pipeline, &blur_v);
        fullscreen_pass(encoder, "glass composite", frame.target, &self.composite_pipeline, &composite);
    }

    /// Blur targets sized for `size`, recreated when the backdrop size changes.
    /// Returned by value: texture views are reference counted, so this is cheap.
    fn blur_targets(&mut self, device: &wgpu::Device, size: (u32, u32)) -> BlurTargets {
        if let Some(t) = &self.blur_targets
            && t.size == size
        {
            return t.clone();
        }
        let make = |label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: (size.0 / BLUR_DOWNSAMPLE).max(1),
                        height: (size.1 / BLUR_DOWNSAMPLE).max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: BLUR_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let targets = BlurTargets { size, horizontal: make("glass blur h"), vertical: make("glass blur v") };
        self.blur_targets = Some(targets.clone());
        targets
    }

    fn blur_bind_group(
        &self,
        device: &wgpu::Device,
        uniform: &wgpu::Buffer,
        src: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glass blur"),
            layout: &self.blur_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(src) },
            ],
        })
    }
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
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
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}
