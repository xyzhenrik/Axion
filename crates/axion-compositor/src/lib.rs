//! Builds each window frame from a [`Scene`]: an ordered list of layers.
//!
//! - [`Layer::Image`]: a texture (a page frame, a thumbnail).
//! - [`Layer::Glass`]: Liquid Glass shapes that refract **everything drawn so far**.
//! - [`Layer::Content`]: rounded rectangles, text and icons on top.
//!
//! Layers render into one of two offscreen targets; a glass layer reads the current target
//! as its backdrop and writes the other, so glass can sit on glass (a card inside the sidebar).
//! The last target is blitted to the window. All coordinates are physical pixels with the
//! origin top-left, and colours are sRGB-encoded (CEF frames are too), blended as on the web.

mod icons;
mod probe;

use axion_glass::{GlassFrame, GlassParams, GlassRenderer, GlassShape};
use bytemuck::{Pod, Zeroable};
use glam::Vec2;
pub use glyphon;
use glyphon::{Cache, ColorMode, CustomGlyph, Resolution, SwashCache, TextAtlas, TextRenderer, Viewport};
pub use icons::{Icon, IconWeight};

const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// An axis-aligned rectangle in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub origin: Vec2,
    pub size: Vec2,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { origin: Vec2::new(x, y), size: Vec2::new(w, h) }
    }

    pub fn center(&self) -> Vec2 {
        self.origin + self.size * 0.5
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.cmpge(self.origin).all() && p.cmplt(self.origin + self.size).all()
    }

    pub fn inset(&self, by: f32) -> Rect {
        Rect { origin: self.origin + by, size: (self.size - 2.0 * by).max(Vec2::ZERO) }
    }
}

/// A filled rounded rectangle, or a soft shadow when `softness > 0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub rect: Rect,
    pub radius: f32,
    /// sRGB, straight alpha.
    pub color: [f32; 4],
    /// 0 for a crisp, anti-aliased edge; otherwise the blur radius of a shadow.
    pub softness: f32,
}

impl Quad {
    pub fn fill(rect: Rect, radius: f32, color: [f32; 4]) -> Self {
        Self { rect, radius, color, softness: 0.0 }
    }

    /// CSS-like `box-shadow: 0 offset_y blur color`.
    pub fn shadow(rect: Rect, radius: f32, offset_y: f32, blur: f32, color: [f32; 4]) -> Self {
        let rect = Rect { origin: rect.origin + Vec2::new(0.0, offset_y), size: rect.size };
        Self { rect, radius, color, softness: blur }
    }
}

/// An icon from the design's set, drawn in `rect` (square) with `color`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconDraw {
    pub icon: Icon,
    pub weight: IconWeight,
    pub rect: Rect,
    pub color: [f32; 4],
}

/// A texture placed in the frame.
pub struct ImageDraw<'a> {
    pub texture: &'a wgpu::TextureView,
    pub rect: Rect,
    /// Part of the texture to show, in 0–1 texture coordinates. A one-texel-thin `uv` stretches
    /// the texture's edge across `rect` (edge extension behind the bar).
    pub uv: Rect,
    pub radius: f32,
    pub opacity: f32,
}

impl<'a> ImageDraw<'a> {
    pub fn new(texture: &'a wgpu::TextureView, rect: Rect) -> Self {
        Self { texture, rect, uv: Rect::new(0.0, 0.0, 1.0, 1.0), radius: 0.0, opacity: 1.0 }
    }
}

/// Solid shapes, then images, then icons and text — all above what was drawn before.
#[derive(Default)]
pub struct Content<'a> {
    pub quads: Vec<Quad>,
    pub images: Vec<ImageDraw<'a>>,
    pub icons: Vec<IconDraw>,
    pub text: Vec<glyphon::TextArea<'a>>,
    /// Draw only inside this rectangle (physical px), e.g. a scrolled list.
    pub clip: Option<Rect>,
}

impl Content<'_> {
    pub fn is_empty(&self) -> bool {
        self.quads.is_empty() && self.images.is_empty() && self.icons.is_empty() && self.text.is_empty()
    }
}

pub enum Layer<'a> {
    Image(ImageDraw<'a>),
    Glass { shapes: Vec<GlassShape>, params: GlassParams },
    Content(Content<'a>),
}

pub struct Scene<'a> {
    /// sRGB background behind the first layer.
    pub clear: [f32; 4],
    pub layers: Vec<Layer<'a>>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ViewportUniform {
    size: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct QuadInstance {
    rect: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ImageUniform {
    rect: [f32; 4],
    uv: [f32; 4],
    viewport: [f32; 2],
    radius: f32,
    opacity: f32,
}

struct Targets {
    size: (u32, u32),
    textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
}

/// Per-content-layer text state (glyphon prepares one renderer per draw batch).
struct TextBatch {
    renderer: TextRenderer,
}

pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    glass: GlassRenderer,
    quad_pipeline: wgpu::RenderPipeline,
    quad_bind_group: wgpu::BindGroup,
    viewport_buffer: wgpu::Buffer,
    image_layout: wgpu::BindGroupLayout,
    image_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    text_atlas: TextAtlas,
    text_viewport: Viewport,
    text_batches: Vec<TextBatch>,
    swash: SwashCache,
    empty_text: glyphon::Buffer,
    probe: probe::LuminanceProbe,
    targets: Option<Targets>,
    /// Which target holds the finished frame after [`Compositor::render`].
    last: usize,
}

impl Compositor {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        output_format: wgpu::TextureFormat,
        fonts: &mut glyphon::FontSystem,
    ) -> Self {
        let viewport_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compositor viewport"),
            size: size_of::<ViewportUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let quad_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/quad.wgsl").into()),
        });
        let quad_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX)],
        });
        let quad_pipeline = pipeline(
            device,
            "quad",
            &quad_layout,
            &quad_shader,
            &[wgpu::VertexBufferLayout {
                array_stride: size_of::<QuadInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
            }],
            TARGET_FORMAT,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let quad_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quad"),
            layout: &quad_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: viewport_buffer.as_entire_binding() }],
        });

        let image_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/image.wgsl").into()),
        });
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
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
        let image_pipeline = pipeline(
            device,
            "image",
            &image_layout,
            &image_shader,
            &[],
            TARGET_FORMAT,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let blit_pipeline = pipeline(device, "blit", &image_layout, &image_shader, &[], output_format, None);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("compositor"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let cache = Cache::new(device);
        let mut text_atlas = TextAtlas::with_color_mode(device, queue, &cache, TARGET_FORMAT, ColorMode::Web);
        let text_viewport = Viewport::new(device, &cache);
        let first = TextBatch {
            renderer: TextRenderer::new(&mut text_atlas, device, wgpu::MultisampleState::default(), None),
        };
        let empty_text = glyphon::Buffer::new(fonts, glyphon::Metrics::new(1.0, 1.0));

        Self {
            device: device.clone(),
            queue: queue.clone(),
            glass: GlassRenderer::new(device, TARGET_FORMAT),
            quad_pipeline,
            quad_bind_group,
            viewport_buffer,
            image_layout,
            image_pipeline,
            blit_pipeline,
            sampler,
            text_atlas,
            text_viewport,
            text_batches: vec![first],
            swash: SwashCache::new(),
            empty_text,
            probe: probe::LuminanceProbe::new(device),
            targets: None,
            last: 0,
        }
    }

    fn ensure_targets(&mut self, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let make = |label| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let textures = [make("compositor target a"), make("compositor target b")];
        let views =
            [textures[0].create_view(&Default::default()), textures[1].create_view(&Default::default())];
        self.targets = Some(Targets { size, textures, views });
    }

    /// Composites `scene` and writes it to `output` (the window's surface view), which must be
    /// `size` and of the format given to [`Compositor::new`].
    pub fn render(
        &mut self,
        scene: &Scene<'_>,
        fonts: &mut glyphon::FontSystem,
        output: &wgpu::TextureView,
        size: (u32, u32),
    ) {
        let size = (size.0.max(1), size.1.max(1));
        self.ensure_targets(size);
        let Some(targets) = self.targets.take() else { return };
        let viewport = Vec2::new(size.0 as f32, size.1 as f32);
        self.queue.write_buffer(
            &self.viewport_buffer,
            0,
            bytemuck::bytes_of(&ViewportUniform { size: viewport.to_array(), _pad: [0.0; 2] }),
        );
        self.text_viewport.update(&self.queue, Resolution { width: size.0, height: size.1 });

        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("compositor") });
        let mut current = 0;
        clear(&mut encoder, &targets.views[current], scene.clear);

        let mut text_batch = 0;
        for layer in &scene.layers {
            match layer {
                Layer::Image(image) => {
                    let bind_group = self.image_bind_group(image, viewport);
                    let mut pass = begin_pass(&mut encoder, "image", &targets.views[current]);
                    pass.set_pipeline(&self.image_pipeline);
                    pass.set_bind_group(0, &bind_group, &[]);
                    pass.draw(0..4, 0..1);
                }
                Layer::Glass { shapes, params } => {
                    let next = 1 - current;
                    self.glass.render(
                        &self.device,
                        &mut encoder,
                        GlassFrame {
                            backdrop: &targets.views[current],
                            backdrop_size: size,
                            target: &targets.views[next],
                            shapes,
                            params,
                        },
                    );
                    current = next;
                }
                Layer::Content(content) => {
                    self.draw_content(
                        &mut encoder,
                        &targets.views[current],
                        content,
                        fonts,
                        text_batch,
                        viewport,
                    );
                    text_batch += 1;
                }
            }
        }

        let full = ImageDraw::new(&targets.views[current], Rect::new(0.0, 0.0, viewport.x, viewport.y));
        let bind_group = self.image_bind_group(&full, viewport);
        {
            let mut pass = begin_pass(&mut encoder, "present blit", output);
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.text_atlas.trim();
        self.last = current;
        self.targets = Some(targets);
    }

    fn draw_content(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        content: &Content<'_>,
        fonts: &mut glyphon::FontSystem,
        batch: usize,
        viewport: Vec2,
    ) {
        let image_groups: Vec<wgpu::BindGroup> =
            content.images.iter().map(|image| self.image_bind_group(image, viewport)).collect();
        let instances: Vec<QuadInstance> = content
            .quads
            .iter()
            .map(|q| QuadInstance {
                rect: [q.rect.origin.x, q.rect.origin.y, q.rect.size.x, q.rect.size.y],
                color: q.color,
                params: [q.radius, q.softness, 0.0, 0.0],
            })
            .collect();
        let quad_buffer = (!instances.is_empty()).then(|| {
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("quads"),
                size: (instances.len() * size_of::<QuadInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&instances));
            buffer
        });

        // Icons ride along as glyphon custom glyphs in one extra text area.
        let glyphs: Vec<CustomGlyph> = content
            .icons
            .iter()
            .map(|i| CustomGlyph {
                id: icons::glyph_id(i.icon, i.weight),
                left: i.rect.origin.x,
                top: i.rect.origin.y,
                width: i.rect.size.x,
                height: i.rect.size.y,
                color: Some(to_glyphon(i.color)),
                snap_to_physical_pixel: true,
                metadata: 0,
            })
            .collect();
        let mut areas = content.text.clone();
        if !glyphs.is_empty() {
            areas.push(glyphon::TextArea {
                buffer: &self.empty_text,
                left: 0.0,
                top: 0.0,
                scale: 1.0,
                bounds: glyphon::TextBounds::default(),
                default_color: glyphon::Color::rgb(255, 255, 255),
                custom_glyphs: &glyphs,
            });
        }
        while self.text_batches.len() <= batch {
            let renderer = TextRenderer::new(
                &mut self.text_atlas,
                &self.device,
                wgpu::MultisampleState::default(),
                None,
            );
            self.text_batches.push(TextBatch { renderer });
        }
        let renderer = &mut self.text_batches[batch].renderer;
        if let Err(error) = renderer.prepare_with_custom(
            &self.device,
            &self.queue,
            fonts,
            &mut self.text_atlas,
            &self.text_viewport,
            areas,
            &mut self.swash,
            icons::rasterize,
        ) {
            tracing::warn!(%error, "preparing text failed");
        }

        let mut pass = begin_pass(encoder, "content", target);
        if let Some(clip) = content.clip {
            let x = clip.origin.x.max(0.0).floor();
            let y = clip.origin.y.max(0.0).floor();
            let right = (clip.origin.x + clip.size.x).min(viewport.x).ceil();
            let bottom = (clip.origin.y + clip.size.y).min(viewport.y).ceil();
            if right <= x || bottom <= y {
                return;
            }
            pass.set_scissor_rect(x as u32, y as u32, (right - x) as u32, (bottom - y) as u32);
        }
        if let Some(buffer) = &quad_buffer {
            pass.set_pipeline(&self.quad_pipeline);
            pass.set_bind_group(0, &self.quad_bind_group, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..4, 0..instances.len() as u32);
        }
        if !image_groups.is_empty() {
            pass.set_pipeline(&self.image_pipeline);
            for group in &image_groups {
                pass.set_bind_group(0, group, &[]);
                pass.draw(0..4, 0..1);
            }
        }
        if let Err(error) =
            self.text_batches[batch].renderer.render(&self.text_atlas, &self.text_viewport, &mut pass)
        {
            tracing::warn!(%error, "drawing text failed");
        }
    }

    fn image_bind_group(&self, image: &ImageDraw<'_>, viewport: Vec2) -> wgpu::BindGroup {
        let (rect, uv) = (image.rect, image.uv);
        let uniform = ImageUniform {
            rect: [rect.origin.x, rect.origin.y, rect.size.x, rect.size.y],
            uv: [uv.origin.x, uv.origin.y, uv.size.x, uv.size.y],
            viewport: viewport.to_array(),
            radius: image.radius,
            opacity: image.opacity,
        };
        let texture = image.texture;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("image draw"),
            size: size_of::<ImageUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&uniform));
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("image draw"),
            layout: &self.image_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(texture) },
            ],
        })
    }

    /// Measures the average colour (sRGB, 0–1) of `region` (0–1 texture coordinates) of
    /// `texture`. Non-blocking: returns the latest finished measurement (a frame or two old).
    pub fn probe_color(&mut self, texture: &wgpu::TextureView, region: Rect) -> Option<[f32; 3]> {
        self.probe.update(&self.device, &self.queue, texture, region);
        self.probe.latest()
    }

    /// Reads back the last rendered frame as tightly packed RGBA8 (for screenshots and tests).
    pub fn read_back(&self) -> Option<(u32, u32, Vec<u8>)> {
        let targets = self.targets.as_ref()?;
        let (width, height) = targets.size;
        let row = (4 * width).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("read back"),
            size: (row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            targets.textures[self.last].as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let data = buffer.slice(..).get_mapped_range().ok()?;
        let mut pixels = Vec::with_capacity((4 * width * height) as usize);
        for y in 0..height as usize {
            let start = y * row as usize;
            pixels.extend_from_slice(&data[start..start + 4 * width as usize]);
        }
        Some((width, height, pixels))
    }
}

fn to_glyphon(c: [f32; 4]) -> glyphon::Color {
    let [r, g, b, a] = c.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
    glyphon::Color::rgba(r, g, b, a)
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    buffers: &[wgpu::VertexBufferLayout<'_>],
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
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
            buffers: &buffers.iter().cloned().map(Some).collect::<Vec<_>>(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn clear(encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, color: [f32; 4]) {
    let [r, g, b, a] = color.map(f64::from);
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
}

fn begin_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

#[cfg(test)]
mod tests {
    use wgpu::naga;

    #[test]
    fn shaders_are_valid() {
        for (name, source) in [
            ("quad.wgsl", include_str!("../shaders/quad.wgsl")),
            ("image.wgsl", include_str!("../shaders/image.wgsl")),
            ("probe.wgsl", include_str!("../shaders/probe.wgsl")),
        ] {
            let module = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }
}
