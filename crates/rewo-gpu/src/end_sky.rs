//! The End skybox — `SkyRenderer.buildEndSky` / `renderEndSky`, ported exactly.
//!
//! Ground truth (26.2 decompile,
//! `net/minecraft/client/renderer/SkyRenderer.java`):
//!
//! ```text
//! private static GpuBuffer buildEndSky() {
//!    for (int i = 0; i < 6; i++) {
//!       Matrix4f pose = new Matrix4f();
//!       switch (i) {                       // 0: identity
//!          case 1: pose.rotationX( PI/2);  break;
//!          case 2: pose.rotationX(-PI/2);  break;
//!          case 3: pose.rotationX( PI);    break;
//!          case 4: pose.rotationZ( PI/2);  break;
//!          case 5: pose.rotationZ(-PI/2);
//!       }
//!       addVertex(pose, -100, -100, -100).setUv( 0,  0).setColor(-14145496);
//!       addVertex(pose, -100, -100,  100).setUv( 0, 16).setColor(-14145496);
//!       addVertex(pose,  100, -100,  100).setUv(16, 16).setColor(-14145496);
//!       addVertex(pose,  100, -100, -100).setUv(16,  0).setColor(-14145496);
//!    }
//! }
//! ```
//!
//! Six copies of the same `y = -100` quad, each rotated onto one cube face, UV
//! `0..16` so `textures/environment/end_sky.png` tiles 16× per face (the
//! sampler must REPEAT), all at the constant colour `-14145496` = `0xFF282828`
//! — a dark grey that is why the End sky reads as near-black star field rather
//! than the raw texture. `renderEndSky` draws them as `QUADS` (36 indices)
//! through `RenderPipelines.END_SKY`: `core/position_tex_color`, no depth
//! interaction, `BlendFunction.TRANSLUCENT`, model-view with the camera
//! translation stripped.
//!
//! **End flashes are explicitly out of scope** (`renderEndFlash`,
//! `SkyRenderState.endFlashIntensity`) — `addSkyPass` draws them after this,
//! and nothing here pretends otherwise.
//!
//! Colour space — **a Rewo attachment-conversion inference, not a decompiled
//! fact.** The decompile gives the constant (`-14145496`) and the pipeline
//! (`core/position_tex_color`, TRANSLUCENT, no depth); it does *not* state the
//! colour space of vanilla's render target, and nothing here should be read as
//! claiming it does.
//!
//! What Rewo knows about its *own* pipeline is exact: the texture is uploaded
//! as `R8G8B8A8_SRGB`, so the sampler returns a linearized texel, and the
//! attachment is `R8G8B8A8_SRGB`, so it re-encodes linear→sRGB on store. For
//! the stored bytes to come out as `texel_srgb * (40/255)` — i.e. for the
//! constant to act as the same proportional darkening in the encoded domain
//! that a gamma-space multiply would — the multiplier applied in linear space
//! must be `srgb_to_linear(40/255)`, which is what [`end_sky_linear_rgba`]
//! computes. (Exact, because both factors sit on the sRGB curve's pure-power
//! segment, where the transfer function is multiplicative.)
//!
//! Whether that reproduces vanilla's screen bytes is therefore an inference
//! about the target, and it is pinned by measurement rather than argument: the
//! `skyshot --check` End cases read the rendered pixels back and compare them
//! against this value computed independently on the CPU. If the inference is
//! ever shown wrong, the fix is here and the oracle moves with it.

use ash::vk;
use glam::Mat4;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::entities::create_texture;
use crate::world::DEPTH_FORMAT;
use crate::Gpu;

/// `buildEndSky`'s per-vertex colour: `-14145496` as an unsigned ARGB word.
pub const END_SKY_COLOR_ARGB: u32 = (-14145496i32) as u32;

/// Half-extent of the cube (`±100.0F` in `buildEndSky`).
pub const END_SKY_EXTENT: f32 = 100.0;

/// UV maximum (`setUv(16, 16)`) — the texture tiles 16× across a face.
pub const END_SKY_UV_MAX: f32 = 16.0;

/// `RenderSystem`'s sequential QUADS index buffer for 6 quads.
pub const END_SKY_INDEX_COUNT: u32 = 36;

/// One decoded `end_sky.png` (borrowed RGBA8 + dims).
pub struct EndSkyImage<'a> {
    pub rgba: &'a [u8],
    pub w: u32,
    pub h: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct EndSkyVertex {
    pos: [f32; 3],
    uv: [f32; 2],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Push {
    mvp: [[f32; 4]; 4],
}

/// sRGB → linear decode (IEC 61966-2-1), the same piecewise curve the rest of
/// rewo uses when a vanilla gamma-space constant has to reach an sRGB
/// attachment.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_449_936 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The vanilla constant colour as the linear RGBA the shader multiplies in.
/// Alpha stays straight (`0xFF` → `1.0`); only the colour channels are
/// linearized.
pub fn end_sky_linear_rgba() -> [f32; 4] {
    let argb = END_SKY_COLOR_ARGB;
    let ch = |shift: u32| ((argb >> shift) & 0xFF) as f32 / 255.0;
    [
        srgb_to_linear(ch(16)),
        srgb_to_linear(ch(8)),
        srgb_to_linear(ch(0)),
        ch(24),
    ]
}

/// `buildEndSky`'s six per-face poses, in the decompile's `switch` order.
fn face_pose(i: usize) -> Mat4 {
    use std::f32::consts::{FRAC_PI_2, PI};
    match i {
        1 => Mat4::from_rotation_x(FRAC_PI_2),
        2 => Mat4::from_rotation_x(-FRAC_PI_2),
        3 => Mat4::from_rotation_x(PI),
        4 => Mat4::from_rotation_z(FRAC_PI_2),
        5 => Mat4::from_rotation_z(-FRAC_PI_2),
        _ => Mat4::IDENTITY,
    }
}

/// The 24 logical QUADS vertices of `buildEndSky`, in source order.
fn end_sky_quads() -> Vec<EndSkyVertex> {
    let color = end_sky_linear_rgba();
    let e = END_SKY_EXTENT;
    let u = END_SKY_UV_MAX;
    // The four `addVertex(pose, x, -100, z).setUv(..)` calls, verbatim.
    let corners: [([f32; 3], [f32; 2]); 4] = [
        ([-e, -e, -e], [0.0, 0.0]),
        ([-e, -e, e], [0.0, u]),
        ([e, -e, e], [u, u]),
        ([e, -e, -e], [u, 0.0]),
    ];
    let mut out = Vec::with_capacity(24);
    for i in 0..6 {
        let pose = face_pose(i);
        for (pos, uv) in corners {
            let p = pose.transform_point3(glam::Vec3::from_array(pos));
            out.push(EndSkyVertex {
                pos: p.to_array(),
                uv,
                color,
            });
        }
    }
    out
}

/// The QUADS → triangle-list expansion `RenderSystem.getSequentialBuffer`
/// performs: `0,1,2, 0,2,3` per quad, 36 indices for 6 quads.
fn quad_indices(quads: usize) -> Vec<u32> {
    let mut idx = Vec::with_capacity(quads * 6);
    for q in 0..quads as u32 {
        let b = q * 4;
        idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    idx
}

struct Buf {
    buffer: vk::Buffer,
    alloc: Option<Allocation>,
}

pub struct EndSkyPass {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    image: vk::Image,
    image_alloc: Option<Allocation>,
    view: vk::ImageView,
    vbuf: Buf,
    ibuf: Buf,
    index_count: u32,
}

impl EndSkyPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        tex: &EndSkyImage,
    ) -> Result<Self, String> {
        let (image, image_alloc, view) = create_texture(gpu, tex.rgba, tex.w, tex.h)?;

        let verts = end_sky_quads();
        let indices = quad_indices(verts.len() / 4);
        let index_count = indices.len() as u32;
        let vbuf = upload_buffer(
            gpu,
            bytemuck::cast_slice(&verts),
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?;
        let ibuf = upload_buffer(
            gpu,
            bytemuck::cast_slice(&indices),
            vk::BufferUsageFlags::INDEX_BUFFER,
        )?;

        let device = gpu.device.clone();
        // REPEAT is load-bearing: the UVs run 0..16, so anything else would
        // clamp the whole face to the texture's last texel column.
        let sampler = unsafe {
            device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .address_mode_u(vk::SamplerAddressMode::REPEAT)
                        .address_mode_v(vk::SamplerAddressMode::REPEAT)
                        .address_mode_w(vk::SamplerAddressMode::REPEAT),
                    None,
                )
                .map_err(|e| format!("end-sky sampler: {e}"))?
        };
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let set_layout = unsafe {
            device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| format!("end-sky set layout: {e}"))?
        };
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)];
        let pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .map_err(|e| format!("end-sky pool: {e}"))?
        };
        let set_layouts = [set_layout];
        let set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("end-sky set: {e}"))?[0]
        };
        let image_info = [vk::DescriptorImageInfo::default()
            .sampler(sampler)
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        unsafe {
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&image_info)],
                &[],
            );
        }
        let push_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(std::mem::size_of::<Push>() as u32)];
        let layout = unsafe {
            device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&push_range),
                    None,
                )
                .map_err(|e| format!("end-sky layout: {e}"))?
        };
        let pipeline = build_pipeline(&device, layout, color_format)?;

        Ok(Self {
            pipeline,
            layout,
            set_layout,
            pool,
            set,
            sampler,
            image,
            image_alloc: Some(image_alloc),
            view,
            vbuf,
            ibuf,
            index_count,
        })
    }

    /// Draw the cube in rotation-only sky space. `sky_vp` is the world pass's
    /// `view_proj · T(eye)` — the same construction the celestial pass uses,
    /// reproducing vanilla's translation-stripped model-view.
    pub fn draw(&self, gpu: &Gpu, cb: vk::CommandBuffer, sky_vp: [[f32; 4]; 4], extent: vk::Extent2D) {
        let device = &gpu.device;
        let push = Push { mvp: sky_vp };
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[self.set],
                &[],
            );
            device.cmd_push_constants(
                cb,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytemuck::bytes_of(&push),
            );
            device.cmd_bind_vertex_buffers(cb, 0, &[self.vbuf.buffer], &[0]);
            device.cmd_bind_index_buffer(cb, self.ibuf.buffer, 0, vk::IndexType::UINT32);
            device.cmd_draw_indexed(cb, self.index_count, 1, 0, 0, 0);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        let device = &gpu.device;
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            device.destroy_buffer(self.vbuf.buffer, None);
            device.destroy_buffer(self.ibuf.buffer, None);
        }
        for a in [
            self.image_alloc.take(),
            self.vbuf.alloc.take(),
            self.ibuf.alloc.take(),
        ]
        .into_iter()
        .flatten()
        {
            let _ = gpu.allocator.free(a);
        }
    }
}

fn upload_buffer(
    gpu: &mut Gpu,
    bytes: &[u8],
    usage: vk::BufferUsageFlags,
) -> Result<Buf, String> {
    let device = &gpu.device;
    let buffer = unsafe {
        device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(bytes.len().max(4) as u64)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("end-sky buffer: {e}"))?
    };
    let req = unsafe { device.get_buffer_memory_requirements(buffer) };
    let mut alloc = gpu
        .allocator
        .allocate(&AllocationCreateDesc {
            name: "end-sky",
            requirements: req,
            location: MemoryLocation::CpuToGpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|e| format!("end-sky alloc: {e}"))?;
    unsafe {
        gpu.device
            .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
            .map_err(|e| format!("end-sky bind: {e}"))?;
    }
    if !bytes.is_empty() {
        alloc.mapped_slice_mut().ok_or("end-sky buffer not mapped")?[..bytes.len()]
            .copy_from_slice(bytes);
    }
    Ok(Buf {
        buffer,
        alloc: Some(alloc),
    })
}

fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/end_sky.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/end_sky.frag.spv")),
        )?;
        let entry = c"main";
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert)
                .name(entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag)
                .name(entry),
        ];
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<EndSkyVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(12),
            vk::VertexInputAttributeDescription::default()
                .location(2)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(20),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        // The cube is viewed from the inside on three faces and from "behind"
        // on the others; vanilla's END_SKY pipeline specifies no cull, so
        // neither does this.
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // Drawn before terrain, which overwrites it via reversed-Z.
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);
        // BlendFunction.TRANSLUCENT, matching END_SKY's ColorTargetState.
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B
                    | vk::ColorComponentFlags::A,
            )];
        let blend_state =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [color_format];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&color_formats)
            .depth_attachment_format(DEPTH_FORMAT);
        let ci = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend_state)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&ci), None)
            .map_err(|(_, e)| format!("end-sky pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_constant_is_the_decompiled_word() {
        // -14145496 == 0xFF282828: opaque, rgb (40, 40, 40).
        assert_eq!(END_SKY_COLOR_ARGB, 0xFF28_2828);
        assert_eq!((END_SKY_COLOR_ARGB >> 24) & 0xFF, 0xFF);
        assert_eq!(END_SKY_COLOR_ARGB & 0xFF, 40);
        let rgba = end_sky_linear_rgba();
        assert_eq!(rgba[3], 1.0, "alpha stays straight");
        // 40/255 = 0.156863 sRGB → ~0.02122 linear. The raw gamma value would
        // be ~7.4× brighter; this pins that the linearization actually happened.
        assert!(
            (rgba[0] - 0.021_219_0).abs() < 1e-5,
            "linear grey {} != srgb_to_linear(40/255)",
            rgba[0]
        );
        assert_eq!(rgba[0], rgba[1]);
        assert_eq!(rgba[1], rgba[2]);
    }

    /// The six poses must produce a closed ±100 cube: every vertex has exactly
    /// one axis at ±100 magnitude on all three components, and each of the six
    /// axis-aligned faces is covered exactly once. A dropped or duplicated
    /// `switch` case shows up here as a missing/doubled face.
    #[test]
    fn six_poses_build_a_closed_cube() {
        let v = end_sky_quads();
        assert_eq!(v.len(), 24, "6 quads × 4 vertices");
        let mut faces: Vec<(usize, i32)> = Vec::new();
        for quad in v.chunks(4) {
            // Every vertex of a quad shares one constant ±100 coordinate.
            let mut found = None;
            for axis in 0..3 {
                let a = quad[0].pos[axis];
                if quad.iter().all(|q| (q.pos[axis] - a).abs() < 1e-3) && a.abs() > 99.0 {
                    found = Some((axis, a.signum() as i32));
                }
            }
            let f = found.expect("quad is not axis-aligned at ±100");
            assert!(!faces.contains(&f), "face {f:?} built twice");
            faces.push(f);
            // Its other two coordinates must span the full ±100 square.
            for axis in 0..3 {
                if axis == f.0 {
                    continue;
                }
                let lo = quad.iter().fold(f32::MAX, |m, q| m.min(q.pos[axis]));
                let hi = quad.iter().fold(f32::MIN, |m, q| m.max(q.pos[axis]));
                assert!((lo + 100.0).abs() < 1e-3 && (hi - 100.0).abs() < 1e-3);
            }
        }
        assert_eq!(faces.len(), 6, "all six faces present");
    }

    #[test]
    fn uvs_and_color_are_uniform_and_tile_sixteen() {
        let v = end_sky_quads();
        let color = end_sky_linear_rgba();
        for vert in &v {
            assert_eq!(vert.color, color, "every vertex carries -14145496");
            assert!(vert.uv[0] == 0.0 || vert.uv[0] == 16.0);
            assert!(vert.uv[1] == 0.0 || vert.uv[1] == 16.0);
        }
        // The source order is (0,0) (0,16) (16,16) (16,0) per quad.
        for quad in v.chunks(4) {
            assert_eq!(quad[0].uv, [0.0, 0.0]);
            assert_eq!(quad[1].uv, [0.0, 16.0]);
            assert_eq!(quad[2].uv, [16.0, 16.0]);
            assert_eq!(quad[3].uv, [16.0, 0.0]);
        }
    }

    #[test]
    fn quads_expand_to_the_vanilla_index_count() {
        let idx = quad_indices(6);
        assert_eq!(idx.len() as u32, END_SKY_INDEX_COUNT);
        assert_eq!(&idx[..6], &[0, 1, 2, 0, 2, 3]);
        assert_eq!(&idx[30..], &[20, 21, 22, 20, 22, 23]);
    }
}
