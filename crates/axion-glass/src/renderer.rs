use bytemuck::{Pod, Zeroable};

use crate::bounds::{self, IRect, Region};
use crate::params::{GlassParams, ParamsUniform};
use crate::shape::{GlassShape, ShapeGpu};

/// Maximum number of glass shapes drawn in one pass.
pub const MAX_SHAPES: usize = 32;

/// The frosted backdrop is blurred at 1/BLUR_DOWNSAMPLE resolution.
pub(crate) const BLUR_DOWNSAMPLE: u32 = 4;
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

/// Inputs for one glass composite drawn onto the backdrop texture itself.
pub struct GlassInPlace<'a> {
    pub texture: &'a wgpu::Texture,
    pub view: &'a wgpu::TextureView,
    pub shapes: &'a [GlassShape],
    pub params: &'a GlassParams,
}

/// Per-layer GPU buffers and the shapes they were built from.
struct Prepared {
    shapes: Vec<GlassShape>,
    /// Widest smooth-union radius of the layer.
    merge_k: f32,
    params_buffer: wgpu::Buffer,
    shapes_buffer: wgpu::Buffer,
    blur_buffers: [wgpu::Buffer; 2],
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
    scratch: Option<((u32, u32), wgpu::TextureFormat, wgpu::Texture, wgpu::TextureView)>,
    full_screen: bool,
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
            scratch: None,
            full_screen: false,
            warned_overflow: false,
        }
    }

    /// Draws every layer's passes over the whole frame, like the first version did. For
    /// regression tests and as a fallback: the default restricts the work to where shapes are.
    pub fn set_full_screen(&mut self, full_screen: bool) {
        self.full_screen = full_screen;
    }

    /// Records the blur and composite passes into `encoder`, writing the complete result to
    /// `frame.target`.
    ///
    /// Each call gets its own parameter buffers: several glass layers in one frame are recorded
    /// into the same command buffer, and shared buffers would all end up with the last values.
    /// The blur only runs where shapes are; the composite must fill the whole target (use
    /// [`GlassRenderer::render_in_place`] to restrict that too).
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: GlassFrame<'_>,
    ) {
        let size = frame.backdrop_size;
        let prepared = self.prepare(device, frame.shapes, frame.params, size);
        let targets = self.blur_targets(device, size);
        let composite = self.composite_bind_group(&prepared, frame.backdrop, &targets.vertical, device);
        let regions = self.regions(&prepared, frame.params, size);
        self.blur_passes(device, encoder, frame.backdrop, &targets, &prepared, frame.params, size, &regions);
        pass(encoder, "glass composite", frame.target, &self.composite_pipeline, &composite, None, false);
    }

    /// Like [`GlassRenderer::render`], but draws onto `frame.texture` itself: the backdrop and
    /// the target are the same texture, and everything outside the shapes' regions is left
    /// untouched instead of being redrawn. The texture needs `COPY_SRC`, `RENDER_ATTACHMENT`
    /// and `TEXTURE_BINDING` usage and the format given to [`GlassRenderer::new`].
    pub fn render_in_place(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: GlassInPlace<'_>,
    ) {
        let extent = frame.texture.size();
        let size = (extent.width, extent.height);
        let prepared = self.prepare(device, frame.shapes, frame.params, size);
        let regions = self.regions(&prepared, frame.params, size);
        if regions.is_empty() {
            return;
        }
        let targets = self.blur_targets(device, size);
        // All blurs first: they read the backdrop, which the composites change.
        self.blur_passes(device, encoder, frame.view, &targets, &prepared, frame.params, size, &regions);

        // The composite samples the backdrop at refracted positions, so it reads a copy of the
        // regions (grown by the refraction reach) taken before any of them is drawn over.
        let scratch = self.scratch_view(device, size, frame.texture.format());
        let reach = (frame.params.refraction.abs() * (1.0 + frame.params.dispersion.abs())).ceil() as u32 + 3;
        for region in &regions {
            let r = region.composite.grown(reach, size);
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: frame.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: r.x0, y: r.y0, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &scratch.0,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: r.x0, y: r.y0, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d { width: r.width(), height: r.height(), depth_or_array_layers: 1 },
            );
        }
        let composite = self.composite_bind_group(&prepared, &scratch.1, &targets.vertical, device);
        for region in &regions {
            pass(
                encoder,
                "glass composite",
                frame.view,
                &self.composite_pipeline,
                &composite,
                Some(region.composite),
                true,
            );
        }
    }

    /// Uploads the per-layer buffers and orders the shapes (see below).
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        frame_shapes: &[GlassShape],
        params: &GlassParams,
        (width, height): (u32, u32),
    ) -> Prepared {
        use wgpu::util::DeviceExt;

        let resolution = [width as f32, height as f32];
        let shapes = if frame_shapes.len() > MAX_SHAPES {
            if !self.warned_overflow {
                tracing::warn!(
                    count = frame_shapes.len(),
                    max = MAX_SHAPES,
                    "too many glass shapes; extra ones are skipped"
                );
                self.warned_overflow = true;
            }
            &frame_shapes[..MAX_SHAPES]
        } else {
            frame_shapes
        };
        // Shapes with a wider smooth union of their own (bridges) go last and share the widest
        // radius: they flow into everything before them, and the shader needs no per-shape
        // radius (one less load per shape per pixel).
        let wide = |s: &&GlassShape| s.merge.is_some_and(|m| m > params.merge_smoothness);
        let ordered = shapes.iter().filter(|s| !wide(s)).chain(shapes.iter().filter(wide));
        let mut gpu_shapes: Vec<ShapeGpu> = ordered.map(|s| s.to_gpu()).collect();
        let count = gpu_shapes.len() as u32;
        let wide_count = shapes.iter().filter(wide).count() as u32;
        let wide_k = shapes.iter().filter_map(|s| s.merge).fold(params.merge_smoothness, f32::max);
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
        let mut uniform = ParamsUniform::new(
            params,
            resolution,
            count,
            shapes.iter().any(|s| s.tint_opacity.is_some() || s.glow > 0.0 || s.join_only),
        );
        uniform.wide_from = count - wide_count;
        uniform.wide_k = wide_k;
        let params_buffer = buffer("glass params", bytemuck::bytes_of(&uniform), wgpu::BufferUsages::UNIFORM);
        let step = params.blur_radius.max(0.0) / BLUR_TAPS;
        let blur_buffers = [
            BlurUniform { step_uv: [step / resolution[0], 0.0], _pad: [0.0; 2] },
            BlurUniform { step_uv: [0.0, step / resolution[1]], _pad: [0.0; 2] },
        ]
        .map(|u| buffer("glass blur", bytemuck::bytes_of(&u), wgpu::BufferUsages::UNIFORM));
        Prepared {
            shapes: shapes.to_vec(),
            merge_k: wide_k.max(params.merge_smoothness).max(0.0),
            params_buffer,
            shapes_buffer,
            blur_buffers,
        }
    }

    /// The screen regions this layer's passes cover (the whole frame in full-screen mode).
    fn regions(&self, prepared: &Prepared, params: &GlassParams, size: (u32, u32)) -> Vec<Region> {
        if self.full_screen {
            let full = IRect::full(size);
            return vec![Region { composite: full, blur: full }];
        }
        bounds::regions(&prepared.shapes, params, prepared.merge_k, size)
    }

    /// Blurs the backdrop (horizontal, then vertical) inside each region.
    ///
    /// The vertical pass needs the horizontal result `blur_radius` above and below the region,
    /// so the horizontal pass covers the region grown vertically by that much. Horizontally
    /// both passes read the source, so no growth is needed there.
    #[allow(clippy::too_many_arguments)]
    fn blur_passes(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        backdrop: &wgpu::TextureView,
        targets: &BlurTargets,
        prepared: &Prepared,
        params: &GlassParams,
        size: (u32, u32),
        regions: &[Region],
    ) {
        // Fully clear glass never samples the blurred backdrop.
        if params.frost.clamp(0.0, 1.0) <= 0.0 || regions.is_empty() {
            return;
        }
        let blur_h = self.blur_bind_group(device, &prepared.blur_buffers[0], backdrop);
        let blur_v = self.blur_bind_group(device, &prepared.blur_buffers[1], &targets.horizontal);
        let bounds = ((size.0 / BLUR_DOWNSAMPLE).max(1), (size.1 / BLUR_DOWNSAMPLE).max(1));
        let reach = (params.blur_radius.max(0.0) / BLUR_DOWNSAMPLE as f32).ceil() as u32 + 3;
        for region in regions {
            let (v, h) = if self.full_screen {
                (None, None)
            } else {
                let Some(v) = bounds::blur_rect(&region.blur, bounds) else { continue };
                let h = IRect { y0: v.y0.saturating_sub(reach), y1: (v.y1 + reach).min(bounds.1), ..v };
                (Some(v), Some(h))
            };
            pass(encoder, "glass blur h", &targets.horizontal, &self.blur_pipeline, &blur_h, h, h.is_some());
            pass(encoder, "glass blur v", &targets.vertical, &self.blur_pipeline, &blur_v, v, v.is_some());
        }
    }

    fn composite_bind_group(
        &self,
        prepared: &Prepared,
        backdrop: &wgpu::TextureView,
        blurred: &wgpu::TextureView,
        device: &wgpu::Device,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glass composite"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: prepared.params_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: prepared.shapes_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(backdrop) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(blurred) },
            ],
        })
    }

    /// The scratch copy of the backdrop for in-place compositing, recreated on size changes.
    fn scratch_view(
        &mut self,
        device: &wgpu::Device,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        if let Some((s, f, t, v)) = &self.scratch
            && *s == size
            && *f == format
        {
            return (t.clone(), v.clone());
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glass scratch"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        self.scratch = Some((size, format, texture.clone(), view.clone()));
        (texture, view)
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

/// One full-screen-triangle pass, optionally limited to `rect` (a scissor: pixels outside are
/// neither shaded nor, with `load`, changed). Without `load` the target is cleared first.
fn pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    rect: Option<IRect>,
    load: bool,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if load { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(wgpu::Color::BLACK) },
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    if let Some(r) = rect {
        pass.set_scissor_rect(r.x0, r.y0, r.width(), r.height());
    }
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}
