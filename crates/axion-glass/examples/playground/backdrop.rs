//! Renders the fake web page (or a user image) that sits beneath the glass.

use bytemuck::{Pod, Zeroable};

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PageUniform {
    resolution: [f32; 2],
    scroll: f32,
    time: f32,
    dark: f32,
    use_image: f32,
    image_size: [f32; 2],
}

pub struct PageState {
    pub scroll: f32,
    pub time: f32,
    pub dark: bool,
}

pub struct Backdrop {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    image_size: Option<(u32, u32)>,
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

impl Backdrop {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        image: Option<image::RgbaImage>,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("backdrop.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("backdrop.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("backdrop"),
            layout: None,
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
            label: Some("page"),
            size: size_of::<PageUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let image_size = image.as_ref().map(|i| i.dimensions());
        let (w, h) = image_size.unwrap_or((1, 1));
        let image_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("page image"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let pixels = image.map(|i| i.into_raw()).unwrap_or_else(|| vec![255; 4]);
        queue.write_texture(
            image_texture.as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("page"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &image_texture.create_view(&Default::default()),
                    ),
                },
            ],
        });

        let texture = Self::target(device, size);
        let view = texture.create_view(&Default::default());
        Self { pipeline, bind_group, uniform, image_size, texture, view }
    }

    fn target(device: &wgpu::Device, (width, height): (u32, u32)) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("backdrop"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if (self.texture.width(), self.texture.height()) != size {
            self.texture = Self::target(device, size);
            self.view = self.texture.create_view(&Default::default());
        }
    }

    pub fn render(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, page: &PageState) {
        let (iw, ih) = self.image_size.unwrap_or((1, 1));
        let uniform = PageUniform {
            resolution: [self.texture.width() as f32, self.texture.height() as f32],
            scroll: page.scroll,
            time: page.time,
            dark: if page.dark { 1.0 } else { 0.0 },
            use_image: if self.image_size.is_some() { 1.0 } else { 0.0 },
            image_size: [iw as f32, ih as f32],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("backdrop"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
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
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
