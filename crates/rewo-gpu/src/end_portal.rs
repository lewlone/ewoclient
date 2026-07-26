//! The end portal / gateway pass — `rendertype_end_portal` (M32).
//!
//! M28f shipped the geometry and approximated the shader with one static layer
//! of `end_portal.png`, saying so. This is the shader.
//!
//! ```glsl
//! vec3 color = textureProj(Sampler0, texProj0).rgb * COLORS[0];
//! for (int i = 0; i < PORTAL_LAYERS; i++)
//!     color += textureProj(Sampler1, texProj0 * end_portal_layer(float(i + 1))).rgb * COLORS[i];
//! fragColor = vec4(color, 1.0);
//! ```
//!
//! **It samples in SCREEN space, not model space.** The vertex format is
//! position-only and `texProj0` is `projection_from_position(gl_Position)`, so
//! the starfield slides as the camera moves rather than being painted onto the
//! quad. That is why the portal's mesh UVs were never used, and why this needs
//! a pipeline of its own rather than a texture swap on an existing one.
//!
//! **`PORTAL_LAYERS` is 15 for a portal and 16 for a gateway** — a shader
//! define in vanilla, which is why they are two pipelines there. Here it is a
//! push constant, so one pipeline serves both and the difference is data.
//!
//! Sampler0 is `environment/end_sky.png`, Sampler1 is
//! `entity/end_portal/end_portal.png`; both REPEAT, because the layer matrices
//! scale the coordinate by up to nine and every sample past the first would
//! otherwise clamp to an edge texel.
//!
//! Vanilla's fog term is **omitted**: Rewo applies fog in its own world pass,
//! and a second application here would double it. Stated rather than implied.

use ash::vk;
use gpu_allocator::vulkan::Allocation;

use crate::end_sky::upload_buffer;
use crate::entities::create_texture;
use crate::end_sky::Buf;
use crate::Gpu;
use crate::world::DEPTH_FORMAT;

/// `PORTAL_LAYERS` for `RenderPipelines.END_PORTAL`.
pub const PORTAL_LAYERS: i32 = 15;
/// `PORTAL_LAYERS` for `RenderPipelines.END_GATEWAY` — one more, and the only
/// difference between the two pipelines.
pub const GATEWAY_LAYERS: i32 = 16;

/// One of the two textures this pass samples.
pub struct PortalImage<'a> {
    pub rgba: &'a [u8],
    pub w: u32,
    pub h: u32,
}

/// Position-only, exactly as `DefaultVertexFormat.POSITION`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PortalVertex {
    pub pos: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Push {
    mvp: [[f32; 4]; 4],
    game_time: f32,
    layers: i32,
    _pad: [f32; 2],
}

/// One portal or gateway to draw: its faces, in world space.
pub struct PortalDraw {
    pub verts: Vec<PortalVertex>,
    pub layers: i32,
}

pub struct EndPortalPass {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    images: [(vk::Image, vk::ImageView); 2],
    allocs: Vec<Allocation>,
    vbuf: Option<Buf>,
    vert_count: u32,
    /// Where each draw's vertices start, with its layer count.
    runs: Vec<(u32, u32, i32)>,
}

impl EndPortalPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        sky: &PortalImage,
        portal: &PortalImage,
    ) -> Result<Self, String> {
        let (sky_img, sky_alloc, sky_view) =
            create_texture(gpu, sky.rgba, sky.w, sky.h)?;
        let (por_img, por_alloc, por_view) =
            create_texture(gpu, portal.rgba, portal.w, portal.h)?;

        let device = gpu.device.clone();
        // REPEAT is load-bearing here in a way it is not for a normal texture:
        // `end_portal_layer` scales the coordinate by `(4.5 - layer/4) * 2`,
        // up to 9x, so every layer past the first samples far outside 0..1.
        let sampler = unsafe {
            device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::LINEAR)
                        .min_filter(vk::Filter::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::REPEAT)
                        .address_mode_v(vk::SamplerAddressMode::REPEAT)
                        .address_mode_w(vk::SamplerAddressMode::REPEAT),
                    None,
                )
                .map_err(|e| format!("end-portal sampler: {e}"))?
        };
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let set_layout = unsafe {
            device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| format!("end-portal set layout: {e}"))?
        };
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(2)];
        let pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .map_err(|e| format!("end-portal pool: {e}"))?
        };
        let set_layouts = [set_layout];
        let set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("end-portal set: {e}"))?[0]
        };
        let sky_info = [vk::DescriptorImageInfo::default()
            .sampler(sampler)
            .image_view(sky_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let por_info = [vk::DescriptorImageInfo::default()
            .sampler(sampler)
            .image_view(por_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        unsafe {
            device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&sky_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&por_info),
                ],
                &[],
            );
        }
        // The push block is read by BOTH stages: the vertex shader needs the
        // mvp, the fragment shader the time and the layer count.
        let push_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
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
                .map_err(|e| format!("end-portal layout: {e}"))?
        };
        let pipeline = build_pipeline(&device, layout, color_format)?;

        Ok(Self {
            pipeline,
            layout,
            set_layout,
            pool,
            set,
            sampler,
            images: [(sky_img, sky_view), (por_img, por_view)],
            allocs: vec![sky_alloc, por_alloc],
            vbuf: None,
            vert_count: 0,
            runs: Vec::new(),
        })
    }

    /// Replace this frame's portal geometry.
    pub fn set_draws(&mut self, gpu: &mut Gpu, draws: &[PortalDraw]) -> Result<(), String> {
        self.runs.clear();
        let mut verts: Vec<PortalVertex> = Vec::new();
        for d in draws {
            let start = verts.len() as u32;
            verts.extend_from_slice(&d.verts);
            self.runs.push((start, d.verts.len() as u32, d.layers));
        }
        self.vert_count = verts.len() as u32;
        free_buf(gpu, self.vbuf.take());
        if verts.is_empty() {
            return Ok(());
        }
        self.vbuf = Some(upload_buffer(
            gpu,
            bytemuck::cast_slice(&verts),
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?);
        Ok(())
    }

    /// Draw every portal. One `cmd_draw` per run, because the layer count is a
    /// push constant and a portal and a gateway in the same frame differ in it.
    pub fn draw(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        game_time: f32,
        extent: vk::Extent2D,
    ) {
        let Some(vbuf) = self.vbuf.as_ref() else {
            return;
        };
        if self.vert_count == 0 {
            return;
        }
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
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[self.set],
                &[],
            );
            device.cmd_bind_vertex_buffers(cb, 0, &[vbuf.buffer], &[0]);
            for (start, count, layers) in &self.runs {
                let push = Push {
                    mvp: view_proj,
                    game_time,
                    layers: *layers,
                    _pad: [0.0; 2],
                };
                device.cmd_push_constants(
                    cb,
                    self.layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    bytemuck::bytes_of(&push),
                );
                device.cmd_draw(cb, *count, 1, *start, 0);
            }
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        free_buf(gpu, self.vbuf.take());
        let device = gpu.device.clone();
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
            for (img, view) in self.images {
                device.destroy_image_view(view, None);
                device.destroy_image(img, None);
            }
        }
        for a in self.allocs.drain(..) {
            let _ = gpu.allocator.free(a);
        }
    }
}

fn free_buf(gpu: &mut Gpu, buf: Option<Buf>) {
    if let Some(mut b) = buf {
        unsafe { gpu.device.destroy_buffer(b.buffer, None) };
        if let Some(a) = b.alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

/// `GameTime` as the shader wants it.
///
/// Vanilla's `globals.glsl` `GameTime` is the world clock expressed as a
/// fraction of a day — `(gameTime % 24000) / 24000`, a small number that wraps
/// daily. The layer translate multiplies it by `(2 + layer/1.5) * 1.5`, so a
/// raw tick count would scroll the starfield hundreds of times faster than
/// vanilla's and read as static noise.
pub fn game_time_fraction(game_time: i64) -> f32 {
    (game_time.rem_euclid(24_000)) as f32 / 24_000.0
}

fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/end_portal.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/end_portal.frag.spv")),
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
            .stride(std::mem::size_of::<PortalVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [vk::VertexInputAttributeDescription::default()
            .location(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(0)];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        // A portal is a slab seen from above and below and a gateway a cube
        // seen from outside; vanilla's pipeline specifies no cull, so neither
        // does this.
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // `DepthStencilState.DEFAULT` — tested and written, like terrain. The
        // world pass uses REVERSED-Z (GREATER, clear 0), so the comparison
        // matches it rather than vanilla's LESS.
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::GREATER);
        // The shader writes alpha 1 and vanilla's pipeline sets no blend
        // function, so this is an opaque overwrite. The glow comes from the
        // shader's own accumulation across layers, not from additive blending.
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(false)
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
            .map_err(|(_, e)| format!("end-portal pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gateway_gets_one_more_layer_than_a_portal() {
        // `withShaderDefine("PORTAL_LAYERS", 15)` vs 16 — the ONLY difference
        // between vanilla's two pipelines.
        assert_eq!(PORTAL_LAYERS, 15);
        assert_eq!(GATEWAY_LAYERS, 16);
        assert_eq!(GATEWAY_LAYERS, PORTAL_LAYERS + 1);
    }

    #[test]
    fn game_time_is_a_daily_fraction_not_a_tick_count() {
        assert_eq!(game_time_fraction(0), 0.0);
        assert!((game_time_fraction(12_000) - 0.5).abs() < 1e-6);
        // It wraps, and a negative clock wraps forward rather than going
        // negative — the layer translate would otherwise scroll backwards.
        assert!((game_time_fraction(24_000) - 0.0).abs() < 1e-6);
        assert!(game_time_fraction(-1) > 0.999);
    }
}
