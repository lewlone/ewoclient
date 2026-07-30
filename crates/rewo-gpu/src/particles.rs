//! The particle pass — camera-facing billboards (M37).
//!
//! Ground truth: `net/minecraft/client/particle/SingleQuadParticle.java`
//! (`extractRotatedQuad`) and `assets/minecraft/shaders/core/particle.vsh`.
//! The *simulation* lives in `rewo_world::particles`; this is the geometry and
//! the pipeline, exactly the split `rewo_world::weather` / `rewo_gpu::weather`
//! already use.
//!
//! # What a particle quad is
//!
//! One quad per live particle, centred on its interpolated position, expanded
//! along the camera's right and up axes so it always faces the viewer —
//! vanilla's `FacingCameraMode.LOOKAT_XYZ`, which sets the quad's rotation to
//! the camera's own rotation quaternion. Rewo extracts the right/up basis from
//! the view matrix on the CPU instead of carrying a quaternion, which is the
//! same transform expressed differently: the rows of a rotation matrix *are*
//! the basis vectors the quaternion would rotate to.
//!
//! Vanilla's `roll` (a Z-rotation about the view axis, used by a few types) is
//! not applied — none of the six kinds M37 simulates sets it. Recorded here so
//! the omission is a scope decision rather than an oversight.
//!
//! # Deviations, stated rather than implied
//!
//! * **No depth write.** Particles are translucent and unsorted, so writing
//!   depth would let whichever quad drew first cut holes in the ones behind it.
//!   Depth is still tested, so terrain occludes particles correctly. Vanilla
//!   sorts its translucent particle layers; Rewo does not, which is visible
//!   only where two particles of different colours overlap.
//! * **One texture array, one draw.** Vanilla splits particles across
//!   `SingleQuadParticle.Layer` — six variants over three atlases (particles,
//!   blocks, items), opaque and translucent. Rewo instead puts the block
//!   textures and the particle sprites in a single `sampler2DArray` and selects
//!   per-vertex, so a block-break shard (which samples the *block* texture, as
//!   `getParticleMaterial` requires) and a flame share one pipeline and one
//!   draw. The sprites are point-upscaled to the block `TEX_SIZE` to fit.

use ash::vk;

use crate::buf_ring::BufRing;
use crate::Gpu;

/// One particle as the renderer needs it — the twin of
/// `rewo_world::particles::Particle`, reduced to what a quad needs.
///
/// `rewo-gpu` depends on no other rewo crate by design, so the app converts
/// between them; the same seam `WeatherColumn` uses. Vanilla draws the same
/// line — `QuadParticleRenderState` lives in the renderer package, not in the
/// level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleQuad {
    /// Interpolated world position (the `lerp(partialTick, xo, x)` vanilla
    /// feeds `extractRotatedQuad`).
    pub pos: [f64; 3],
    /// Half-extent in blocks — `getQuadSize(partialTick)`.
    pub size: f32,
    pub color: [f32; 4],
    /// Sub-rectangle within the layer: `[u0, v0, u1, v1]`. A whole sprite is
    /// `[0,0,1,1]`; a terrain shard takes a quarter-window (`TerrainParticle`'s
    /// `uo`/`vo`), which is why this is a rectangle rather than implied.
    pub uv: [f32; 4],
    /// Index into the pass's texture array.
    pub layer: u32,
    pub block_light: u8,
    pub sky_light: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ParticleVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    pub light: u32,
    pub layer: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Push {
    mvp: [[f32; 4]; 4],
    light: [f32; 4],
    sky_col: [f32; 4],
    ambient: [f32; 4],
}

/// The pass's texture array: the block textures followed by the particle
/// sprites, every layer `size` square.
pub struct ParticleAtlas<'a> {
    pub layers: &'a [Vec<u8>],
    pub size: u32,
}

/// The vertex light word `lm_light` reads — block at bit 16, sky at bit 20.
pub fn light_word(block: u8, sky: u8) -> u32 {
    ((block as u32 & 15) << 16) | ((sky as u32 & 15) << 20)
}

/// Camera right/up basis from a view matrix.
///
/// For a rotation matrix the **rows** are the world-space axes the camera's
/// local axes map from, so the first row is "right" and the second is "up".
/// `view` is column-major (`view[col][row]`), so row *r* is
/// `[view[0][r], view[1][r], view[2][r]]`.
///
/// This is `FacingCameraMode.LOOKAT_XYZ` — `target.set(camera.rotation())` —
/// expressed as a basis rather than a quaternion. Deriving it from the matrix
/// the pass already has avoids carrying a second representation of the same
/// rotation that could drift from it.
pub fn camera_basis(view: [[f32; 4]; 4]) -> ([f32; 3], [f32; 3]) {
    let right = [view[0][0], view[1][0], view[2][0]];
    let up = [view[0][1], view[1][1], view[2][1]];
    (right, up)
}

/// Expand a particle list into camera-facing quads, in world space.
///
/// `camera` is subtracted and added back rather than skipped: the size is in
/// blocks and the basis is unitless, so the arithmetic is identical either way
/// — but keeping the relative form visible makes the world-space output an
/// explicit choice (see the module header, and the M33 weather trap).
pub fn build_quads(
    out: &mut Vec<ParticleVertex>,
    particles: &[ParticleQuad],
    view: [[f32; 4]; 4],
) {
    let (right, up) = camera_basis(view);
    for p in particles {
        let s = p.size;
        let (px, py, pz) = (p.pos[0] as f32, p.pos[1] as f32, p.pos[2] as f32);
        // The four corners: centre ± right·s ± up·s.
        let corner = |rs: f32, us: f32| {
            [
                px + right[0] * s * rs + up[0] * s * us,
                py + right[1] * s * rs + up[1] * s * us,
                pz + right[2] * s * rs + up[2] * s * us,
            ]
        };
        let light = light_word(p.block_light, p.sky_light);
        let v = |pos: [f32; 3], uv: [f32; 2]| ParticleVertex {
            pos,
            uv,
            color: p.color,
            light,
            layer: p.layer,
        };
        let (u0, v0, u1, v1) = (p.uv[0], p.uv[1], p.uv[2], p.uv[3]);
        // Vanilla emits QUADS as (-1,-1), (-1,+1), (+1,+1), (+1,-1) with UVs
        // (u1,v1), (u1,v0), (u0,v0), (u0,v1); expanded here into two triangles
        // with the same winding.
        let a = v(corner(-1.0, -1.0), [u1, v1]);
        let b = v(corner(-1.0, 1.0), [u1, v0]);
        let c = v(corner(1.0, 1.0), [u0, v0]);
        let d = v(corner(1.0, -1.0), [u0, v1]);
        out.extend_from_slice(&[a, b, c, a, c, d]);
    }
}

/// A frame's particle geometry.
pub struct ParticleDraw {
    pub verts: Vec<ParticleVertex>,
}

impl ParticleDraw {
    pub fn build(particles: &[ParticleQuad], view: [[f32; 4]; 4]) -> Self {
        let mut verts = Vec::with_capacity(particles.len() * 6);
        build_quads(&mut verts, particles, view);
        Self { verts }
    }

    pub fn is_empty(&self) -> bool {
        self.verts.is_empty()
    }
}

pub struct ParticlePass {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    image: (vk::Image, vk::ImageView),
    alloc: Option<gpu_allocator::vulkan::Allocation>,
    /// This frame's quads. A [`BufRing`] rather than a bare buffer because
    /// `set_draw` runs before the frame is submitted — see `buf_ring`'s module
    /// docs (M86).
    vbuf: BufRing,
    count: u32,
}

impl ParticlePass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        atlas: &ParticleAtlas,
    ) -> Result<Self, String> {
        let (img, alloc, view) = create_layer_array(gpu, atlas)?;
        let device = gpu.device.clone();
        // NEAREST + CLAMP_TO_EDGE: every layer is pixel art, and a terrain
        // shard samples a quarter-window inside its layer — filtering across
        // that window's edge would bleed the rest of the block texture into
        // the shard's border.
        let sampler = unsafe {
            device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                    None,
                )
                .map_err(|e| format!("particle sampler: {e}"))?
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
                .map_err(|e| format!("particle set layout: {e}"))?
        };
        let sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)];
        let pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(|e| format!("particle pool: {e}"))?
        };
        let layouts = [set_layout];
        let set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&layouts),
                )
                .map_err(|e| format!("particle set: {e}"))?[0]
        };
        unsafe {
            let info = [vk::DescriptorImageInfo::default()
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image_view(view)
                .sampler(sampler)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&info)],
                &[],
            );
        }
        let one_layout = [set_layout];
        let push_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(std::mem::size_of::<Push>() as u32)];
        let layout = unsafe {
            device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&one_layout)
                        .push_constant_ranges(&push_range),
                    None,
                )
                .map_err(|e| format!("particle layout: {e}"))?
        };
        let pipeline = build_pipeline(&device, layout, color_format)?;
        Ok(Self {
            pipeline,
            layout,
            set_layout,
            pool,
            set,
            sampler,
            image: (img, view),
            alloc: Some(alloc),
            vbuf: BufRing::new(),
            count: 0,
        })
    }

    pub fn set_draw(&mut self, gpu: &mut Gpu, draw: &ParticleDraw) -> Result<(), String> {
        self.count = draw.verts.len() as u32;
        // `is_empty` is the draw's own emptiness test, not the vertex slice's —
        // keep asking it, and clear the slot when it says yes.
        let verts: &[ParticleVertex] = if draw.is_empty() { &[] } else { &draw.verts };
        self.vbuf.set(
            gpu,
            bytemuck::cast_slice(verts),
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )
    }

    pub fn draw(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        lightmap: ([f32; 4], [f32; 4], [f32; 4]),
        extent: vk::Extent2D,
    ) {
        if self.count == 0 {
            return;
        }
        let Some(vbuf) = self.vbuf.bind() else {
            return;
        };
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            device.cmd_bind_vertex_buffers(cb, 0, &[vbuf], &[0]);
            let push = Push {
                mvp: view_proj,
                light: lightmap.0,
                sky_col: lightmap.1,
                ambient: lightmap.2,
            };
            device.cmd_push_constants(
                cb,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytemuck::bytes_of(&push),
            );
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[self.set],
                &[],
            );
            device.cmd_draw(cb, self.count, 1, 0, 0);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        self.vbuf.destroy(gpu);
        let device = gpu.device.clone();
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.image.1, None);
            device.destroy_image(self.image.0, None);
        }
        if let Some(a) = self.alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

/// A `TYPE_2D_ARRAY` texture holding every layer, single mip.
///
/// Single mip is deliberate: particles are small, camera-facing and sampled
/// NEAREST, so a mip chain would only ever soften them. The block texture
/// array the world pass uses does have mips, because terrain is viewed at
/// grazing angles and at distance; a shard is neither.
fn create_layer_array(
    gpu: &mut Gpu,
    atlas: &ParticleAtlas,
) -> Result<(vk::Image, gpu_allocator::vulkan::Allocation, vk::ImageView), String> {
    use gpu_allocator::vulkan::AllocationCreateDesc;
    use gpu_allocator::MemoryLocation;
    let size = atlas.size;
    let count = atlas.layers.len().max(1) as u32;
    let bytes_per = (size * size * 4) as usize;
    unsafe {
        let image = gpu
            .device
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_SRGB)
                    .extent(vk::Extent3D { width: size, height: size, depth: 1 })
                    .mip_levels(1)
                    .array_layers(count)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
            .map_err(|e| format!("particle array image: {e}"))?;
        let req = gpu.device.get_image_memory_requirements(image);
        let alloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name: "particle-texture-array",
                requirements: req,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: gpu_allocator::vulkan::AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("particle array alloc: {e}"))?;
        gpu.device
            .bind_image_memory(image, alloc.memory(), alloc.offset())
            .map_err(|e| format!("particle array bind: {e}"))?;

        // A layer shorter than a full texture is padded transparent rather
        // than rejected, so one malformed texture cannot take the pass down.
        let padded: Vec<Vec<u8>> = if atlas.layers.is_empty() {
            vec![vec![0u8; bytes_per]]
        } else {
            atlas
                .layers
                .iter()
                .map(|l| {
                    let mut v = vec![0u8; bytes_per];
                    let n = l.len().min(bytes_per);
                    v[..n].copy_from_slice(&l[..n]);
                    v
                })
                .collect()
        };
        crate::world::upload_texture_array(gpu, image, size, 1, &padded)?;

        let view = gpu
            .device
            .create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
                    .format(vk::Format::R8G8B8A8_SRGB)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .base_mip_level(0)
                            .level_count(1)
                            .base_array_layer(0)
                            .layer_count(count),
                    ),
                None,
            )
            .map_err(|e| format!("particle array view: {e}"))?;
        Ok((image, alloc, view))
    }
}

fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/particle.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/particle.frag.spv")),
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
        let binding = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<ParticleVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .binding(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .binding(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(12),
            vk::VertexInputAttributeDescription::default()
                .location(2)
                .binding(0)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(20),
            vk::VertexInputAttributeDescription::default()
                .location(3)
                .binding(0)
                .format(vk::Format::R32_UINT)
                .offset(36),
            vk::VertexInputAttributeDescription::default()
                .location(4)
                .binding(0)
                .format(vk::Format::R32_UINT)
                .offset(40),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&binding)
            .vertex_attribute_descriptions(&attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        // No culling: a billboard's winding flips as the camera crosses its
        // plane, and there is no back face to save.
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // Tested against terrain, not written — see the module header.
        // GREATER because the world pass is reversed-Z.
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::GREATER);
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
            .depth_attachment_format(crate::world::DEPTH_FORMAT);
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
            .create_graphics_pipelines(vk::PipelineCache::null(), &[ci], None)
            .map_err(|(_, e)| format!("particle pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An identity view gives the world axes back: right = +X, up = +Y.
    #[test]
    fn identity_view_yields_world_axes() {
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let (r, u) = camera_basis(id);
        assert_eq!(r, [1.0, 0.0, 0.0]);
        assert_eq!(u, [0.0, 1.0, 0.0]);
    }

    /// A 90°-yaw view must rotate the basis with it — the property that makes
    /// the quad face the camera rather than a fixed direction.
    #[test]
    fn a_yawed_view_rotates_the_basis() {
        // Camera yawed 90° about Y: world +X maps to camera -Z.
        let view = [
            [0.0, 0.0, -1.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let (r, u) = camera_basis(view);
        assert_eq!(r, [0.0, 0.0, 1.0], "right should swing to +Z");
        assert_eq!(u, [0.0, 1.0, 0.0], "up is unchanged by a yaw");
    }

    /// Six vertices per particle, centred on its position, and spanning
    /// exactly ±size along the basis.
    #[test]
    fn a_quad_is_centred_and_sized() {
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let p = ParticleQuad {
            pos: [10.0, 64.0, -5.0],
            size: 0.25,
            color: [1.0, 1.0, 1.0, 1.0],
            uv: [0.0, 0.0, 1.0, 1.0],
            layer: 0,
            block_light: 0,
            sky_light: 15,
        };
        let mut out = Vec::new();
        build_quads(&mut out, &[p], id);
        assert_eq!(out.len(), 6);
        let xs: Vec<f32> = out.iter().map(|v| v.pos[0]).collect();
        let ys: Vec<f32> = out.iter().map(|v| v.pos[1]).collect();
        assert!((xs.iter().cloned().fold(f32::MAX, f32::min) - 9.75).abs() < 1e-6);
        assert!((xs.iter().cloned().fold(f32::MIN, f32::max) - 10.25).abs() < 1e-6);
        assert!((ys.iter().cloned().fold(f32::MAX, f32::min) - 63.75).abs() < 1e-6);
        assert!((ys.iter().cloned().fold(f32::MIN, f32::max) - 64.25).abs() < 1e-6);
        // Z is untouched by an identity basis — the quad lies in the XY plane.
        assert!(out.iter().all(|v| (v.pos[2] + 5.0).abs() < 1e-6));
    }

    /// The light word packs where `lm_light` looks for it.
    #[test]
    fn light_word_packs_block_and_sky() {
        assert_eq!(light_word(0, 0), 0);
        assert_eq!(light_word(15, 0), 15 << 16);
        assert_eq!(light_word(0, 15), 15 << 20);
        // Over-range levels are masked rather than bleeding into a neighbour.
        assert_eq!(light_word(255, 255), (15 << 16) | (15 << 20));
    }
}
