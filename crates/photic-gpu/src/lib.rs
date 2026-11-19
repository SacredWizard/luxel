//! # photic-gpu
//!
//! wgpu renderer for `photic-core` primitives. Works with any wgpu host:
//! egui (`egui-wgpu` paint callback), iced, bevy, winit, or headless.
//!
//! * Data lives in persistent instance buffers ([`Layer`]); pan/zoom only
//!   rewrites a 32-byte uniform, so a static dataset costs zero uploads.
//! * One draw call per primitive kind per layer, regardless of item count.
//! * Only vertex/instance buffers and a uniform are used, so it runs on
//!   WebGPU **and** the WebGL2 fallback.

use bytemuck::{Pod, Zeroable};
use photic_core::{GpuTransform, Point, Rect, Segment};
use wgpu::util::DeviceExt;

pub use wgpu;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct ViewUniform {
    scale: [f32; 2],
    offset: [f32; 2],
    viewport_px: [f32; 2],
    _pad: [f32; 2],
}

/// Shared pipelines. Create once per device + target format.
pub struct Renderer {
    layout: wgpu::BindGroupLayout,
    rect: wgpu::RenderPipeline,
    segment: wgpu::RenderPipeline,
    point: wgpu::RenderPipeline,
}

const RECT_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Uint32];
const SEG_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Uint32, 3 => Float32];
const POINT_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Uint32, 2 => Float32];

impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, sample_count: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("photic shaders"),
            source: wgpu::ShaderSource::Wgsl(
                format!("const SRGB_TARGET: bool = {};\n{}", format.is_srgb(), include_str!("shaders.wgsl")).into(),
            ),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("photic view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("photic"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let make = |vs: &str, fs: &str, stride: usize, attrs: &[wgpu::VertexAttribute]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(vs),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: stride as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: attrs,
                    }],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState { count: sample_count, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            rect: make("vs_rect", "fs_solid", size_of::<Rect>(), &RECT_ATTRS),
            segment: make("vs_segment", "fs_solid", size_of::<Segment>(), &SEG_ATTRS),
            point: make("vs_point", "fs_point", size_of::<Point>(), &POINT_ATTRS),
            layout,
        }
    }

    /// Record draw calls for a layer (3 draw calls max).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, layer: &Layer) {
        pass.set_bind_group(0, &layer.bind_group, &[]);
        for (pipe, buf) in [
            (&self.rect, &layer.rects),
            (&self.segment, &layer.segments),
            (&self.point, &layer.points),
        ] {
            if buf.len > 0 {
                pass.set_pipeline(pipe);
                pass.set_vertex_buffer(0, buf.buffer.slice(..));
                pass.draw(0..6, 0..buf.len);
            }
        }
    }
}

/// A growable GPU instance buffer.
struct InstanceBuf {
    buffer: wgpu::Buffer,
    capacity: u64,
    len: u32,
}

impl InstanceBuf {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("photic instances"),
                size: 256,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity: 256,
            len: 0,
        }
    }

    fn upload<T: Pod>(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[T]) {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        let need = bytes.len() as u64;
        if need > self.capacity {
            self.capacity = need.next_power_of_two();
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("photic instances"),
                size: self.capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytes);
        }
        self.len = data.len() as u32;
    }
}

/// One coordinate space (e.g. the candle pane, the volume pane, a scatter
/// plot) with its own transform and instance buffers.
pub struct Layer {
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    rects: InstanceBuf,
    segments: InstanceBuf,
    points: InstanceBuf,
}

impl Layer {
    pub fn new(device: &wgpu::Device, renderer: &Renderer) -> Self {
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("photic view"),
            contents: bytemuck::bytes_of(&ViewUniform::default()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("photic view"),
            layout: &renderer.layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        Self {
            uniform,
            bind_group,
            rects: InstanceBuf::new(device),
            segments: InstanceBuf::new(device),
            points: InstanceBuf::new(device),
        }
    }

    /// Update the view (cheap: 32 bytes). `viewport_px` is the size of the
    /// render-pass viewport in physical pixels.
    pub fn set_view(&self, queue: &wgpu::Queue, t: GpuTransform, viewport_px: [f32; 2]) {
        let u = ViewUniform { scale: t.scale, offset: t.offset, viewport_px, _pad: [0.0; 2] };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));
    }

    pub fn set_rects(&mut self, d: &wgpu::Device, q: &wgpu::Queue, data: &[Rect]) {
        self.rects.upload(d, q, data);
    }
    pub fn set_segments(&mut self, d: &wgpu::Device, q: &wgpu::Queue, data: &[Segment]) {
        self.segments.upload(d, q, data);
    }
    pub fn set_points(&mut self, d: &wgpu::Device, q: &wgpu::Queue, data: &[Point]) {
        self.points.upload(d, q, data);
    }

    /// Instance counts (rects, segments, points).
    pub fn counts(&self) -> (u32, u32, u32) {
        (self.rects.len, self.segments.len, self.points.len)
    }
}
