//! wgpu renderer for the 3D viewport, run as an egui paint callback.

use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::{self, wgpu};
use std::sync::Arc;
use wgpu::util::DeviceExt;

pub const MSAA_SAMPLES: u32 = 4;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
}

impl Vertex {
    pub fn new(pos: glam::DVec3, normal: glam::DVec3, color: [f32; 4]) -> Vertex {
        Vertex { pos: pos.as_vec3().to_array(), normal: normal.as_vec3().to_array(), color }
    }
    pub fn line(pos: glam::DVec3, color: [f32; 4]) -> Vertex {
        Vertex { pos: pos.as_vec3().to_array(), normal: [0.0; 3], color }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Uniforms {
    pub view_proj: [[f32; 4]; 4],
    pub eye: [f32; 4],
    pub light: [f32; 4],
}

/// Geometry that changes only when the model or selection changes.
#[derive(Default)]
pub struct StaticScene {
    pub tris: Vec<Vertex>,
    pub edges: Vec<Vertex>,
}

/// Geometry rebuilt every frame (grid, sketches, previews, highlights).
#[derive(Default)]
pub struct DynamicScene {
    /// Depth-tested lines.
    pub lines: Vec<Vertex>,
    /// Lines drawn on top of everything.
    pub lines_top: Vec<Vertex>,
    /// Translucent fills drawn on top (profiles, planes).
    pub overlay: Vec<Vertex>,
}

const SHADER: &str = r#"
struct U { view_proj: mat4x4<f32>, eye: vec4<f32>, light: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world: vec3<f32>,
};

@vertex
fn vs_main(v: VIn) -> VOut {
    var o: VOut;
    o.clip = u.view_proj * vec4<f32>(v.pos, 1.0);
    o.normal = v.normal;
    o.color = v.color;
    o.world = v.pos;
    return o;
}

@fragment
fn fs_shaded(i: VOut) -> @location(0) vec4<f32> {
    let n = normalize(i.normal);
    let l = normalize(u.light.xyz);
    let view = normalize(u.eye.xyz - i.world);
    let diff = max(dot(n, l), 0.0);
    let h = normalize(l + view);
    let spec = pow(max(dot(n, h), 0.0), 48.0) * 0.25;
    let fill = max(dot(n, -l), 0.0) * 0.12;
    let sky = 0.5 + 0.5 * n.y;
    let ambient = 0.22 + 0.10 * sky;
    let c = i.color.rgb * (ambient + 0.68 * diff + fill) + vec3<f32>(spec);
    return vec4<f32>(c, i.color.a);
}

@fragment
fn fs_flat(i: VOut) -> @location(0) vec4<f32> {
    return i.color;
}
"#;

pub struct GpuResources {
    shaded: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    lines_top: wgpu::RenderPipeline,
    overlay: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    static_gen: u64,
    static_tris: Option<(wgpu::Buffer, u32)>,
    static_edges: Option<(wgpu::Buffer, u32)>,
    dyn_lines: Option<(wgpu::Buffer, u32)>,
    dyn_lines_top: Option<(wgpu::Buffer, u32)>,
    dyn_overlay: Option<(wgpu::Buffer, u32)>,
}

impl GpuResources {
    pub fn install(rs: &egui_wgpu::RenderState) {
        let device = &rs.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewport shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() }],
        });

        let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];
        let make = |label: &str,
                    fs: &str,
                    topology: wgpu::PrimitiveTopology,
                    depth_write: bool,
                    compare: wgpu::CompareFunction,
                    bias: wgpu::DepthBiasState,
                    blend: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: fs,
                    targets: &[Some(wgpu::ColorTargetState {
                        format: rs.target_format,
                        blend: Some(if blend {
                            wgpu::BlendState::ALPHA_BLENDING
                        } else {
                            wgpu::BlendState::REPLACE
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState { topology, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: depth_write,
                    depth_compare: compare,
                    stencil: Default::default(),
                    bias,
                }),
                multisample: wgpu::MultisampleState { count: MSAA_SAMPLES, ..Default::default() },
                multiview: None,
                cache: None,
            })
        };
        use wgpu::{CompareFunction as C, PrimitiveTopology as T};
        let push_back = wgpu::DepthBiasState { constant: 2, slope_scale: 1.5, clamp: 0.0 };
        let none = wgpu::DepthBiasState::default();
        let res = GpuResources {
            shaded: make("shaded", "fs_shaded", T::TriangleList, true, C::LessEqual, push_back, false),
            lines: make("lines", "fs_flat", T::LineList, true, C::LessEqual, none, true),
            lines_top: make("lines top", "fs_flat", T::LineList, false, C::Always, none, true),
            overlay: make("overlay", "fs_flat", T::TriangleList, false, C::LessEqual, none, true),
            uniform_buf,
            bind_group,
            static_gen: u64::MAX,
            static_tris: None,
            static_edges: None,
            dyn_lines: None,
            dyn_lines_top: None,
            dyn_overlay: None,
        };
        rs.renderer.write().callback_resources.insert(res);
    }
}

fn upload(device: &wgpu::Device, data: &[Vertex], label: &str) -> Option<(wgpu::Buffer, u32)> {
    if data.is_empty() {
        return None;
    }
    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(data),
        usage: wgpu::BufferUsages::VERTEX,
    });
    Some((buf, data.len() as u32))
}

pub struct ViewportCallback {
    pub uniforms: Uniforms,
    pub static_gen: u64,
    pub static_scene: Arc<StaticScene>,
    pub dynamic: DynamicScene,
}

impl egui_wgpu::CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _sd: &egui_wgpu::ScreenDescriptor,
        _enc: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(r) = resources.get_mut::<GpuResources>() else {
            return Vec::new();
        };
        queue.write_buffer(&r.uniform_buf, 0, bytemuck::bytes_of(&self.uniforms));
        if r.static_gen != self.static_gen {
            r.static_tris = upload(device, &self.static_scene.tris, "body tris");
            r.static_edges = upload(device, &self.static_scene.edges, "body edges");
            r.static_gen = self.static_gen;
        }
        r.dyn_lines = upload(device, &self.dynamic.lines, "lines");
        r.dyn_lines_top = upload(device, &self.dynamic.lines_top, "lines top");
        r.dyn_overlay = upload(device, &self.dynamic.overlay, "overlay");
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(r) = resources.get::<GpuResources>() else {
            return;
        };
        pass.set_bind_group(0, &r.bind_group, &[]);
        let mut draw = |pipe: &wgpu::RenderPipeline, b: &Option<(wgpu::Buffer, u32)>| {
            if let Some((buf, n)) = b {
                pass.set_pipeline(pipe);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..*n, 0..1);
            }
        };
        draw(&r.shaded, &r.static_tris);
        draw(&r.lines, &r.static_edges);
        draw(&r.lines, &r.dyn_lines);
        draw(&r.overlay, &r.dyn_overlay);
        draw(&r.lines_top, &r.dyn_lines_top);
    }
}

use eframe::egui;
