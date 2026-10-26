//! The terrain draw: the opaque indirect draw, the CPU-sorted water pass, the
//! descriptor sets (block texture array + per-frame lightmap UBO) and the world
//! push-constant block.

use ash::vk;

use super::arena::VERTEX_STRIDE;
use super::cull::frustum_planes;
use super::lightmap::{LightmapRing, WorldLightmapExtra, LIGHTMAP_UBO_RING};
use super::textures::TextureArray;
use super::{WorldRenderer, DEPTH_FORMAT};
use crate::Gpu;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct WorldPush {
    view_proj: [[f32; 4]; 4],
    /// xyz camera world pos, w = fog start distance.
    cam_fog: [f32; 4],
    /// xyz fog color (linear = sky horizon), w = fog end distance.
    fog_col: [f32; 4],
    /// The scalar lightmap uniforms, packed to fill the spare push lanes:
    /// x = SkyFactor (time of day), y = BlockFactor, z = BrightnessFactor
    /// (gamma minus the darkness effect), w = DarknessScale. Vanilla scales
    /// only the sky half by the time of day, so a sunrise is one push, not a
    /// remesh.
    light: [f32; 4],
    /// xyz sky-light colour (white by day, blue at night); w = the
    /// night-vision factor (the ambient seed `max(black, nvColor*w)`).
    sky_col: [f32; 4],
}

// 128 bytes: exactly the push-constant size Vulkan guarantees. Anything
// further has to move into a uniform buffer — which is what M16's dimension
// ambient colour does (see `WorldLightmapExtra` / `LIGHTMAP_UBO_RING`).
const _: () = assert!(std::mem::size_of::<WorldPush>() == 128);

fn build_graphics_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
    translucent: bool,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/world.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            if translucent {
                include_bytes!(concat!(env!("OUT_DIR"), "/water.frag.spv")).as_slice()
            } else {
                include_bytes!(concat!(env!("OUT_DIR"), "/world.frag.spv")).as_slice()
            },
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
            .stride(VERTEX_STRIDE as u32)
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
                .format(vk::Format::R32_UINT)
                .offset(20),
            // The vertex color, read as vanilla's `Color` attribute is:
            // UNORM bytes.
            vk::VertexInputAttributeDescription::default()
                .location(3)
                .format(vk::Format::R8G8B8A8_UNORM)
                .offset(24),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(!translucent)
            .depth_compare_op(vk::CompareOp::GREATER); // reversed-Z
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(translucent)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B,
            )];
        let blend =
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
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&ci), None)
            .map_err(|(_, e)| format!("world pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

/// The terrain descriptor set layout, pool and per-frame-in-flight sets, as
/// created by [`create_terrain_descriptors`] (before the lightmap UBO ring they
/// will point at exists).
pub(super) struct TerrainDescriptors {
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    sets: [vk::DescriptorSet; LIGHTMAP_UBO_RING],
}

pub(super) fn create_terrain_descriptors(
    device: &ash::Device,
) -> Result<TerrainDescriptors, String> {
    unsafe {
        // ---- graphics descriptor + pipeline ----
        // Binding 0: the block texture array. Binding 1: the small
        // `LightmapExtra` UBO carrying the dimension ambient colour — the
        // push block is exactly full at 128 bytes, so this is where a new
        // canonical lightmap input has to live.
        let tex_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            // Read by world.vert, which samples the lightmap per vertex.
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
            // Binding 2 (M197): the fluid flow sprites' 32 px array.
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let set_layout = device
            .create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&tex_bindings),
                None,
            )
            .map_err(|e| format!("tex set layout: {e}"))?;
        let ring = LIGHTMAP_UBO_RING as u32;
        let tex_pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(ring * 2),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(ring),
        ];
        let pool = device
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(ring)
                    .pool_sizes(&tex_pool_sizes),
                None,
            )
            .map_err(|e| format!("tex pool: {e}"))?;
        let ring_layouts = [set_layout; LIGHTMAP_UBO_RING];
        let allocated = device
            .allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&ring_layouts),
            )
            .map_err(|e| format!("tex set: {e}"))?;
        let sets: [vk::DescriptorSet; LIGHTMAP_UBO_RING] = std::array::from_fn(|i| allocated[i]);
        Ok(TerrainDescriptors {
            set_layout,
            pool,
            sets,
        })
    }
}

/// The opaque and water terrain pipelines with their shared layout and the
/// descriptor sets they bind: one set per frame in flight, identical texture
/// binding, but each pointing at its own `LightmapExtra` uniform slot (see
/// [`LIGHTMAP_UBO_RING`]).
pub(super) struct TerrainPass {
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    sets: [vk::DescriptorSet; LIGHTMAP_UBO_RING],
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    /// Blended, depth-write-off variant for the water pass.
    water_pipeline: vk::Pipeline,
}

impl TerrainPass {
    /// Point every set at the texture array and its own lightmap UBO slot, then
    /// build the pipeline layout and the two pipelines.
    pub(super) fn create(
        device: &ash::Device,
        color_format: vk::Format,
        descriptors: TerrainDescriptors,
        textures: &TextureArray,
        flow: &TextureArray,
        lm: &LightmapRing,
    ) -> Result<Self, String> {
        unsafe {
            let TerrainDescriptors {
                set_layout,
                pool,
                sets,
            } = descriptors;
            let tex_layouts = [set_layout];
            let image_info = [vk::DescriptorImageInfo::default()
                .sampler(textures.sampler)
                .image_view(textures.view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let flow_info = [vk::DescriptorImageInfo::default()
                .sampler(flow.sampler)
                .image_view(flow.view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            // `buffer_info` slices must outlive the update call, so build them
            // all up front rather than inside the loop.
            let buf_infos: [[vk::DescriptorBufferInfo; 1]; LIGHTMAP_UBO_RING] =
                std::array::from_fn(|i| {
                    [vk::DescriptorBufferInfo::default()
                        .buffer(lm.bufs[i])
                        .offset(0)
                        .range(std::mem::size_of::<WorldLightmapExtra>() as u64)]
                });
            let mut writes: Vec<vk::WriteDescriptorSet> = Vec::new();
            for i in 0..LIGHTMAP_UBO_RING {
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(sets[i])
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&image_info),
                );
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(sets[i])
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                        .buffer_info(&buf_infos[i]),
                );
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(sets[i])
                        .dst_binding(2)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&flow_info),
                );
            }
            device.update_descriptor_sets(&writes, &[]);

            // view_proj is read in the vertex stage, the fog fields in the
            // fragment stage — one range spanning both.
            let pc_ranges = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .offset(0)
                .size(std::mem::size_of::<WorldPush>() as u32)];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&tex_layouts)
                        .push_constant_ranges(&pc_ranges),
                    None,
                )
                .map_err(|e| format!("graphics layout: {e}"))?;
            let pipeline = build_graphics_pipeline(device, layout, color_format, false)?;
            let water_pipeline = build_graphics_pipeline(device, layout, color_format, true)?;
            Ok(Self {
                set_layout,
                pool,
                sets,
                layout,
                pipeline,
                water_pipeline,
            })
        }
    }

    pub(super) fn destroy_pipelines(&self, device: &ash::Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline(self.water_pipeline, None);
        }
    }

    /// Point every set's binding 2 at a new flow array (M197). The caller has
    /// waited for the device: no frame may still use the old one.
    pub(super) fn rebind_flow(&self, device: &ash::Device, flow: &TextureArray) {
        let info = [vk::DescriptorImageInfo::default()
            .sampler(flow.sampler)
            .image_view(flow.view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let writes: Vec<vk::WriteDescriptorSet> = self
            .sets
            .iter()
            .map(|&set| {
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&info)
            })
            .collect();
        unsafe { device.update_descriptor_sets(&writes, &[]) };
    }

    pub(super) fn destroy_layouts(&self, device: &ash::Device) {
        unsafe {
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}

impl WorldRenderer {
    /// M412 — the terrain's fog as the entity pass's fragment words:
    /// `[camera, render start]`, `[colour, render end]`, `[env start, env
    /// end, _, _]` (the same numbers `push_world` and the lightmap UBO give
    /// `world.frag`).
    pub(crate) fn entity_fog(&self) -> [[f32; 4]; 3] {
        let e = self.scene.camera_eye;
        let env = self.scene.lightmap.env_fog_words();
        [
            [e[0], e[1], e[2], self.scene.fog[0]],
            {
                let [r, g, b] = self.fog_color();
                [r, g, b, self.scene.fog[1]]
            },
            [env[0], env[1], 0.0, 0.0],
        ]
    }

    /// Push the world/fog constants (view_proj + camera + fog band).
    fn push_world(&self, device: &ash::Device, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4]) {
        let (light, sky_col) = self.scene.lightmap.push_words();
        let push = WorldPush {
            view_proj,
            cam_fog: [
                self.scene.camera_eye[0],
                self.scene.camera_eye[1],
                self.scene.camera_eye[2],
                self.scene.fog[0],
            ],
            fog_col: {
                let [r, g, b] = self.fog_color();
                [r, g, b, self.scene.fog[1]]
            },
            light,
            sky_col,
        };
        unsafe {
            device.cmd_push_constants(
                cb,
                self.terrain.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                std::slice::from_raw_parts(
                    (&push as *const WorldPush) as *const u8,
                    std::mem::size_of::<WorldPush>(),
                ),
            );
        }
    }

    pub(super) fn draw_terrain(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.terrain.pipeline);
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
                self.terrain.layout,
                0,
                &[self.terrain.sets[self.lm.cursor]],
                &[],
            );
            self.push_world(device, cb, view_proj);
            device.cmd_bind_vertex_buffers(cb, 0, &[self.arena.vbuf], &[0]);
            device.cmd_bind_index_buffer(cb, self.arena.ibuf, 0, vk::IndexType::UINT32);
            device.cmd_draw_indexed_indirect_count(
                cb,
                self.culler.indirect_bufs[self.culler.cursor],
                0,
                self.culler.count_bufs[self.culler.cursor],
                0,
                self.culler.column_count,
                std::mem::size_of::<vk::DrawIndexedIndirectCommand>() as u32,
            );
        }
    }

    /// Water: per-column direct draws, CPU frustum-culled (mirroring
    /// cull.comp's positive-vertex test) and sorted far→near — the plan's
    /// "per-section back-to-front CPU sort; intra-section artifacts
    /// accepted v1". Column counts with water are small; the indirect path
    /// stays opaque-only.
    pub(super) fn draw_translucent(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let planes = frustum_planes(&view_proj);
        let visible = |mn: &[f32; 4], mx: &[f32; 4]| -> bool {
            planes.iter().all(|p| {
                let pv = [
                    if p[0] >= 0.0 { mx[0] } else { mn[0] },
                    if p[1] >= 0.0 { mx[1] } else { mn[1] },
                    if p[2] >= 0.0 { mx[2] } else { mn[2] },
                ];
                p[0] * pv[0] + p[1] * pv[1] + p[2] * pv[2] + p[3] >= 0.0
            })
        };
        let eye = self.scene.camera_eye;
        let mut cols: Vec<(f32, u32, u32, i32)> = self
            .arena
            .columns
            .values()
            .filter(|s| s.tidx_len > 0 && visible(&s.meta.aabb_min, &s.meta.aabb_max))
            .map(|s| {
                let cx = (s.meta.aabb_min[0] + s.meta.aabb_max[0]) * 0.5 - eye[0];
                let cy = (s.meta.aabb_min[1] + s.meta.aabb_max[1]) * 0.5 - eye[1];
                let cz = (s.meta.aabb_min[2] + s.meta.aabb_max[2]) * 0.5 - eye[2];
                (
                    cx * cx + cy * cy + cz * cz,
                    s.tidx_len as u32,
                    s.tidx_off as u32,
                    s.tvtx_off as i32,
                )
            })
            .collect();
        if cols.is_empty() {
            return;
        }
        cols.sort_by(|a, b| b.0.total_cmp(&a.0));

        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.terrain.water_pipeline,
            );
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
                self.terrain.layout,
                0,
                &[self.terrain.sets[self.lm.cursor]],
                &[],
            );
            self.push_world(device, cb, view_proj);
            device.cmd_bind_vertex_buffers(cb, 0, &[self.arena.vbuf], &[0]);
            device.cmd_bind_index_buffer(cb, self.arena.ibuf, 0, vk::IndexType::UINT32);
            for (_, count, first, voff) in cols {
                device.cmd_draw_indexed(cb, count, 1, first, voff, 0);
            }
        }
    }
}
