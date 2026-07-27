//! Rain and snow — `WeatherEffectRenderer.render` / `renderInstances`, ported
//! exactly (M33).
//!
//! Ground truth:
//! `net/minecraft/client/renderer/WeatherEffectRenderer.java` and
//! `assets/minecraft/shaders/core/particle.{vsh,fsh}` in the 26.2 decompile.
//! The column *extraction* — which columns get rain, which snow, and their
//! per-column randomness — lives in `rewo_world::weather`; this is the
//! geometry and the pipeline.
//!
//! # What a weather column is
//!
//! One quad per (x, z) block column within the radius, spanning from the
//! terrain height to the top of the band, turned to face the camera. It is not
//! a billboard: the facing comes from a **precomputed 32×32 table** of unit
//! vectors perpendicular to the camera-to-column line, so the quad's width
//! direction is fixed by which cell the column sits in rather than recomputed
//! per frame.
//!
//! # Deviations, stated rather than implied
//!
//! Vanilla renders weather into its own framebuffer (`OutputTarget.
//! WEATHER_TARGET`) and picks between depth-writing and non-depth-writing
//! pipelines based on whether shader transparency is on. Rewo has no such
//! target, so this draws into the main pass with the **no-depth-write**
//! variant — the branch vanilla takes without transparency sorting. Depth is
//! still *tested*, so terrain occludes rain correctly; what is lost is the
//! ordering guarantee between overlapping translucent columns.
//!
//! Vanilla's fog term is omitted, as in every other Rewo pass, because Rewo
//! fogs in its world pass.

use ash::vk;

use crate::end_sky::{upload_buffer, Buf};
use crate::Gpu;

/// One weather column as the renderer needs it.
///
/// The twin of `rewo_world::weather::ColumnInstance`, which is where the
/// *extraction* lives (it needs the world, the biomes and the per-column RNG).
/// `rewo-gpu` depends on no other rewo crate by design, so the app converts
/// between them — the same seam every other cross-crate view type here uses.
/// Vanilla draws the same line: `WeatherRenderState` lives in the renderer
/// package, not in the level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherColumn {
    pub x: i32,
    pub z: i32,
    pub bottom_y: i32,
    pub top_y: i32,
    pub u_offset: f32,
    pub v_offset: f32,
    /// Block level 0..15 and sky level 0..15, already unpacked — the render
    /// side has no reason to know `LightCoordsUtil`'s bit layout.
    pub block_light: u8,
    pub sky_light: u8,
}

/// The 32x32 direction table `WeatherEffectRenderer`'s constructor builds.
///
/// Each entry is the horizontal direction a column's quad faces, perpendicular
/// to the line from the camera — which is what makes every streak turn to face
/// you as you walk past it. Indexed by the column's offset from the camera,
/// biased by 16.
///
/// The centre entry (dx = dz = 0) divides by a zero length and is NaN in
/// vanilla too; only the column at the camera's own feet reads it, and it
/// degenerates to a zero-width quad either way.
pub struct ColumnDirections {
    pub size_x: [f32; 1024],
    pub size_z: [f32; 1024],
}

impl Default for ColumnDirections {
    fn default() -> Self {
        Self::new()
    }
}

impl ColumnDirections {
    pub fn new() -> Self {
        let mut size_x = [0f32; 1024];
        let mut size_z = [0f32; 1024];
        for z in 0..32usize {
            for x in 0..32usize {
                let dx = x as f32 - 16.0;
                let dz = z as f32 - 16.0;
                // `Mth.length(deltaX, deltaZ)` — the plain hypotenuse.
                let distance = (dx * dx + dz * dz).sqrt();
                size_x[z * 32 + x] = -dz / distance;
                size_z[z * 32 + x] = dx / distance;
            }
        }
        Self { size_x, size_z }
    }

    /// The table index for a column, given the camera's floored block position.
    pub fn index(&self, column_x: i32, column_z: i32, cam_x: i32, cam_z: i32) -> usize {
        let ix = column_x - cam_x + 16;
        let iz = column_z - cam_z + 16;
        ((iz * 32 + ix).clamp(0, 1023)) as usize
    }
}

/// What `extractRenderState` produces, in render-side terms.
#[derive(Clone, Debug, Default)]
pub struct WeatherRenderState {
    pub intensity: f32,
    pub radius: i32,
    pub rain_columns: Vec<WeatherColumn>,
    pub snow_columns: Vec<WeatherColumn>,
}

impl WeatherRenderState {
    pub fn is_empty(&self) -> bool {
        self.rain_columns.is_empty() && self.snow_columns.is_empty()
    }
}

/// `renderInstances`' `maxAlpha` for rain.
pub const RAIN_MAX_ALPHA: f32 = 1.0;
/// …and for snow, which is drawn fainter.
pub const SNOW_MAX_ALPHA: f32 = 0.8;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WeatherVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub alpha: f32,
    /// Block level in bits 16..19, sky in 20..23 — `lm_light`'s word shape.
    pub light: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Push {
    mvp: [[f32; 4]; 4],
    light: [f32; 4],
    sky_col: [f32; 4],
    ambient: [f32; 4],
}

/// One decoded `rain.png` / `snow.png`.
pub struct WeatherImage<'a> {
    pub rgba: &'a [u8],
    pub w: u32,
    pub h: u32,
}

/// The vertex light word `lm_light` reads: block at bit 16, sky at bit 20 —
/// the same packing the terrain mesher uses.
pub fn light_word(block: u8, sky: u8) -> u32 {
    ((block as u32 & 15) << 16) | ((sky as u32 & 15) << 20)
}

/// `renderInstances` — one column list to four vertices each.
///
/// The alpha ramp is `lerp(min(d²/r², 1), maxAlpha, 0.5) * intensity`, so a
/// column fades from `maxAlpha` at the camera to **half** at the radius — never
/// to nothing, which is why heavy rain still reads as a wall at the horizon.
///
/// **Positions come out in WORLD space, not camera-relative.** Vanilla's are
/// relative because its model-view carries the camera translation; Rewo's
/// `view_proj` already includes it, so emitting the relative form would draw
/// every storm around the world origin. The relative vector is still computed —
/// the alpha ramp and the facing table both need it — and the camera is added
/// back on the way out.
pub fn build_instances(
    out: &mut Vec<WeatherVertex>,
    columns: &[WeatherColumn],
    directions: &ColumnDirections,
    camera: [f64; 3],
    max_alpha: f32,
    radius: i32,
    intensity: f32,
) {
    if columns.is_empty() {
        return;
    }
    let radius_sq = (radius * radius) as f32;
    let cam_x = camera[0].floor() as i32;
    let cam_z = camera[2].floor() as i32;
    for c in columns {
        let rel_x = (c.x as f64 + 0.5 - camera[0]) as f32;
        let rel_z = (c.z as f64 + 0.5 - camera[2]) as f32;
        let dist_sq = rel_x * rel_x + rel_z * rel_z;
        let t = (dist_sq / radius_sq).min(1.0);
        let alpha = (max_alpha + (0.5 - max_alpha) * t) * intensity;
        let i = directions.index(c.x, c.z, cam_x, cam_z);
        let half_x = directions.size_x[i] / 2.0;
        let half_z = directions.size_z[i] / 2.0;
        // World space: the relative vector, with the camera added back.
        let (cx, cz) = (camera[0] as f32, camera[2] as f32);
        let x0 = rel_x - half_x + cx;
        let x1 = rel_x + half_x + cx;
        let y1 = c.top_y as f32;
        let y0 = c.bottom_y as f32;
        let z0 = rel_z - half_z + cz;
        let z1 = rel_z + half_z + cz;
        let u0 = c.u_offset;
        let u1 = c.u_offset + 1.0;
        // The V runs with world height, so the texture scrolls *through* the
        // column rather than stretching to fit it: a tall column shows more
        // repeats, not longer streaks.
        let v0 = c.bottom_y as f32 * 0.25 + c.v_offset;
        let v1 = c.top_y as f32 * 0.25 + c.v_offset;
        let light = light_word(c.block_light, c.sky_light);
        let v = |pos: [f32; 3], uv: [f32; 2]| WeatherVertex {
            pos,
            uv,
            alpha,
            light,
        };
        // Vanilla emits QUADS in the order (top-left, top-right, bottom-right,
        // bottom-left); expanded here into two triangles with the same winding.
        let a = v([x0, y1, z0], [u0, v0]);
        let b = v([x1, y1, z1], [u1, v0]);
        let c2 = v([x1, y0, z1], [u1, v1]);
        let d = v([x0, y0, z0], [u0, v1]);
        out.extend_from_slice(&[a, b, c2, a, c2, d]);
    }
}

/// The whole frame's weather geometry, rain first then snow — the order
/// vanilla draws them in, and the order the two textures bind in.
pub struct WeatherDraw {
    pub rain: Vec<WeatherVertex>,
    pub snow: Vec<WeatherVertex>,
}

impl WeatherDraw {
    /// Build both lists from an extracted render state.
    pub fn build(
        state: &WeatherRenderState,
        directions: &ColumnDirections,
        camera: [f64; 3],
    ) -> Self {
        let mut rain = Vec::new();
        let mut snow = Vec::new();
        build_instances(
            &mut rain,
            &state.rain_columns,
            directions,
            camera,
            RAIN_MAX_ALPHA,
            state.radius,
            state.intensity,
        );
        build_instances(
            &mut snow,
            &state.snow_columns,
            directions,
            camera,
            SNOW_MAX_ALPHA,
            state.radius,
            state.intensity,
        );
        Self { rain, snow }
    }

    pub fn is_empty(&self) -> bool {
        self.rain.is_empty() && self.snow.is_empty()
    }
}

pub struct WeatherPass {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    /// One set per texture: rain and snow are two draws, not two samplers.
    sets: [vk::DescriptorSet; 2],
    sampler: vk::Sampler,
    images: [(vk::Image, vk::ImageView); 2],
    allocs: Vec<gpu_allocator::vulkan::Allocation>,
    vbuf: Option<Buf>,
    rain_count: u32,
    snow_count: u32,
}

impl WeatherPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        rain: &WeatherImage,
        snow: &WeatherImage,
    ) -> Result<Self, String> {
        let (rain_img, rain_alloc, rain_view) =
            crate::entities::create_texture(gpu, rain.rgba, rain.w, rain.h)?;
        let (snow_img, snow_alloc, snow_view) =
            crate::entities::create_texture(gpu, snow.rgba, snow.w, snow.h)?;
        let device = gpu.device.clone();
        // `rain.png` is a vertical strip meant to scroll: REPEAT on V is what
        // makes the streaks continuous, and the column's V spans many repeats.
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
                .map_err(|e| format!("weather sampler: {e}"))?
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
                .map_err(|e| format!("weather set layout: {e}"))?
        };
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(2)];
        let pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(2)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .map_err(|e| format!("weather pool: {e}"))?
        };
        let set_layouts = [set_layout, set_layout];
        let allocated = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("weather sets: {e}"))?
        };
        let sets = [allocated[0], allocated[1]];
        for (set, view) in [(sets[0], rain_view), (sets[1], snow_view)] {
            let info = [vk::DescriptorImageInfo::default()
                .sampler(sampler)
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            unsafe {
                device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&info)],
                    &[],
                );
            }
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
                .map_err(|e| format!("weather layout: {e}"))?
        };
        let pipeline = build_pipeline(&device, layout, color_format)?;
        Ok(Self {
            pipeline,
            layout,
            set_layout,
            pool,
            sets,
            sampler,
            images: [(rain_img, rain_view), (snow_img, snow_view)],
            allocs: vec![rain_alloc, snow_alloc],
            vbuf: None,
            rain_count: 0,
            snow_count: 0,
        })
    }

    pub fn set_draw(&mut self, gpu: &mut Gpu, draw: &WeatherDraw) -> Result<(), String> {
        free_buf(gpu, self.vbuf.take());
        self.rain_count = draw.rain.len() as u32;
        self.snow_count = draw.snow.len() as u32;
        if draw.is_empty() {
            return Ok(());
        }
        let mut verts = draw.rain.clone();
        verts.extend_from_slice(&draw.snow);
        self.vbuf = Some(upload_buffer(
            gpu,
            bytemuck::cast_slice(&verts),
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?);
        Ok(())
    }

    pub fn draw(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        lightmap: ([f32; 4], [f32; 4], [f32; 4]),
        extent: vk::Extent2D,
    ) {
        let Some(vbuf) = self.vbuf.as_ref() else {
            return;
        };
        if self.rain_count + self.snow_count == 0 {
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
            device.cmd_bind_vertex_buffers(cb, 0, &[vbuf.buffer], &[0]);
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
            // Two draws, two textures — vanilla's `renderWeather` calls, with
            // rain's vertices first in the buffer and snow's after.
            for (set, start, count) in [
                (self.sets[0], 0, self.rain_count),
                (self.sets[1], self.rain_count, self.snow_count),
            ] {
                if count == 0 {
                    continue;
                }
                device.cmd_bind_descriptor_sets(
                    cb,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.layout,
                    0,
                    &[set],
                    &[],
                );
                device.cmd_draw(cb, count, 1, start, 0);
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

fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/weather.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/weather.frag.spv")),
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
            .stride(std::mem::size_of::<WeatherVertex>() as u32)
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
                .format(vk::Format::R32_SFLOAT)
                .offset(20),
            vk::VertexInputAttributeDescription::default()
                .location(3)
                .format(vk::Format::R32_UINT)
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
        // A weather quad is seen from both sides as the camera turns past it.
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // `WEATHER_NO_DEPTH_WRITE` — tested so terrain occludes rain, but not
        // written, so overlapping columns do not cut each other out. See the
        // module docs: vanilla picks between this and a depth-writing variant
        // by whether shader transparency is on, and Rewo has only this one.
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
            .map_err(|(_, e)| format!("weather pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(x: i32, z: i32) -> WeatherColumn {
        WeatherColumn {
            x,
            z,
            bottom_y: 64,
            top_y: 96,
            u_offset: 0.0,
            v_offset: 0.0,
            block_light: 0,
            sky_light: 15,
        }
    }

    /// The direction table is perpendicular to the camera-to-column line, and
    /// unit length — that is what turns each streak to face the viewer.
    #[test]
    fn the_direction_table_is_a_unit_perpendicular() {
        let t = ColumnDirections::new();
        for (x, z) in [(20usize, 16usize), (16, 24), (8, 8), (31, 0)] {
            let i = z * 32 + x;
            let (sx, sz) = (t.size_x[i], t.size_z[i]);
            let len = (sx * sx + sz * sz).sqrt();
            assert!((len - 1.0).abs() < 1e-5, "not unit at ({x},{z}): {len}");
            // Perpendicular to the offset from the centre.
            let (dx, dz) = (x as f32 - 16.0, z as f32 - 16.0);
            assert!(
                (sx * dx + sz * dz).abs() < 1e-4,
                "not perpendicular at ({x},{z})"
            );
        }
    }

    /// A column fades from `maxAlpha` at the camera to **half** at the radius,
    /// never to zero — `lerp(t, maxAlpha, 0.5)`, not `lerp(t, maxAlpha, 0)`.
    #[test]
    fn alpha_falls_to_half_at_the_radius_not_to_nothing() {
        let dirs = ColumnDirections::new();
        let mut out = Vec::new();
        // Dead centre: t = 0 (the +0.5 block centre against a .5 camera).
        build_instances(
            &mut out,
            &[column(0, 0)],
            &dirs,
            [0.5, 0.0, 0.5],
            RAIN_MAX_ALPHA,
            10,
            1.0,
        );
        assert!((out[0].alpha - 1.0).abs() < 1e-5, "{}", out[0].alpha);

        // At and beyond the radius the ramp is clamped at exactly half.
        let mut far = Vec::new();
        build_instances(
            &mut far,
            &[column(40, 0)],
            &dirs,
            [0.5, 0.0, 0.5],
            RAIN_MAX_ALPHA,
            10,
            1.0,
        );
        assert!((far[0].alpha - 0.5).abs() < 1e-5, "{}", far[0].alpha);
    }

    /// Intensity scales the whole ramp, so a light drizzle is faint everywhere
    /// rather than merely smaller.
    #[test]
    fn intensity_scales_every_column() {
        let dirs = ColumnDirections::new();
        let mut full = Vec::new();
        let mut half = Vec::new();
        for (out, intensity) in [(&mut full, 1.0f32), (&mut half, 0.5)] {
            build_instances(
                out,
                &[column(3, 0)],
                &dirs,
                [0.5, 0.0, 0.5],
                RAIN_MAX_ALPHA,
                10,
                intensity,
            );
        }
        assert!((full[0].alpha * 0.5 - half[0].alpha).abs() < 1e-6);
    }

    /// Snow is drawn fainter than rain at the same distance.
    #[test]
    fn snow_is_fainter_than_rain() {
        let dirs = ColumnDirections::new();
        let mut rain = Vec::new();
        let mut snow = Vec::new();
        build_instances(
            &mut rain,
            &[column(2, 0)],
            &dirs,
            [0.5, 0.0, 0.5],
            RAIN_MAX_ALPHA,
            10,
            1.0,
        );
        build_instances(
            &mut snow,
            &[column(2, 0)],
            &dirs,
            [0.5, 0.0, 0.5],
            SNOW_MAX_ALPHA,
            10,
            1.0,
        );
        assert!(snow[0].alpha < rain[0].alpha);
    }

    /// The V coordinate tracks world height, so a taller column shows more
    /// texture repeats rather than stretching one.
    #[test]
    fn the_texture_scrolls_through_the_column_rather_than_stretching() {
        let dirs = ColumnDirections::new();
        let mut short = Vec::new();
        let mut tall = Vec::new();
        let mut c = column(1, 0);
        c.top_y = 68;
        build_instances(&mut short, &[c], &dirs, [0.5, 0.0, 0.5], 1.0, 10, 1.0);
        let mut c2 = column(1, 0);
        c2.top_y = 128;
        build_instances(&mut tall, &[c2], &dirs, [0.5, 0.0, 0.5], 1.0, 10, 1.0);
        let span = |v: &[WeatherVertex]| {
            let vs: Vec<f32> = v.iter().map(|x| x.uv[1]).collect();
            vs.iter().cloned().fold(f32::MIN, f32::max)
                - vs.iter().cloned().fold(f32::MAX, f32::min)
        };
        // 4 blocks -> 1.0 of V; 64 blocks -> 16.0. A stretched quad would give
        // the same span for both.
        assert!((span(&short) - 1.0).abs() < 1e-4, "{}", span(&short));
        assert!((span(&tall) - 16.0).abs() < 1e-4, "{}", span(&tall));
    }

    /// Six vertices per column, two triangles sharing the quad's diagonal.
    #[test]
    fn each_column_is_two_triangles() {
        let dirs = ColumnDirections::new();
        let mut out = Vec::new();
        build_instances(
            &mut out,
            &[column(1, 0), column(2, 0)],
            &dirs,
            [0.5, 0.0, 0.5],
            1.0,
            10,
            1.0,
        );
        assert_eq!(out.len(), 12);
        assert_eq!(out[0], out[3], "the diagonal's first shared corner");
        assert_eq!(out[2], out[4], "and its second");
    }

    /// The light word must land where `lm_light` reads it: block at bit 16,
    /// sky at bit 20. A word built for `LightCoordsUtil`'s own `block << 4`
    /// layout would render every streak black.
    #[test]
    fn the_light_word_is_packed_for_the_shader() {
        let word = light_word(7, 12);
        assert_eq!((word >> 16) & 15, 7);
        assert_eq!((word >> 20) & 15, 12);
        assert_eq!(word & 0xFFFF, 0, "nothing below bit 16");
    }
}
