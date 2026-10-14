//! Sky and fog: the gradient sky disc, the celestial and End-sky passes, the
//! per-frame sky/fog tints and the distance-fog band.

use ash::vk;

use super::{WorldRenderer, DEPTH_FORMAT};
use crate::Gpu;

#[repr(C)]
#[derive(Clone, Copy)]
struct SkyPush {
    inv_view_proj: [[f32; 4]; 4],
    horizon: [f32; 4],
    zenith: [f32; 4],
}

/// Sky gradient colors (linear; sRGB pale-blue horizon / deeper zenith).
/// The horizon is also the terrain's fog color for a seamless far edge.
pub const SKY_HORIZON: [f32; 3] = [0.477, 0.638, 0.890];

const SKY_ZENITH: [f32; 3] = [0.147, 0.319, 0.890];

/// The zenith-to-horizon gradient ratio (`SKY_ZENITH / SKY_HORIZON`). When the
/// biome sky color drives the base (M14), the zenith keeps this fixed gradient
/// shape: `zenith = sky_base * ZENITH_RATIO`. `[0.308, 0.5, 1.0]`.
const ZENITH_RATIO: [f32; 3] = [
    SKY_ZENITH[0] / SKY_HORIZON[0],
    SKY_ZENITH[1] / SKY_HORIZON[1],
    SKY_ZENITH[2] / SKY_HORIZON[2],
];

/// Which sky the world pass draws — `DimensionType.Skybox`, as consumed by
/// `LevelRenderer.addSkyPass`.
///
/// Named independently of `rewo_world::dimension::Skybox` so this crate keeps
/// no dependency on rewo-world; the app maps one to the other.
///
/// - [`SkyMode::Overworld`] — gradient sky disc + the M12 celestial pass
///   (`renderSkyDisc` → `renderSunriseAndSunset` → `renderSunMoonAndStars`).
/// - [`SkyMode::None`] — `addSkyPass` adds **no pass at all**
///   (`if (state.skybox != Skybox.NONE)`), so neither gradient nor celestials.
/// - [`SkyMode::End`] — `SkyRenderer.renderEndSky` only: the textured ±100
///   cube. The end *flash* that follows it is out of scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SkyMode {
    /// The pre-M16 behaviour, and the default for every serverless path.
    #[default]
    Overworld,
    None,
    End,
}

/// Fullscreen gradient-sky pipeline: no vertex input, no depth, no blend.
fn build_sky_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/sky.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/sky.frag.spv")),
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
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
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
        // No depth test/write — the sky is pure background.
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(false)
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
        // The pass carries a depth attachment (terrain uses it); the sky
        // pipeline must declare the same format to match, even though its
        // depth test/write are disabled.
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
            .map_err(|(_, e)| format!("sky pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

/// Default fog band, overridable via `REWO_FOG=start,end`. The default
/// end (~180) sits at the flat-world test's loaded radius so the far
/// chunk boundary fully dissolves into the horizon (fog color = sky
/// horizon) instead of ending hard. Ideally derived from render distance;
/// a fixed default + override is the current compromise.
pub(super) fn parse_fog_env() -> [f32; 2] {
    let default = [80.0, 180.0];
    crate::knobs::var("REWO_FOG")
        .ok()
        .and_then(|s| {
            let mut it = s.split(',');
            let a: f32 = it.next()?.trim().parse().ok()?;
            let b: f32 = it.next()?.trim().parse().ok()?;
            Some([a, b.max(a + 1.0)])
        })
        .unwrap_or(default)
}

/// The fullscreen gradient sky: its pipeline layout (a fragment push block, no
/// descriptor sets) and pipeline.
pub(super) struct SkyGradient {
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

impl SkyGradient {
    pub(super) fn create(device: &ash::Device, color_format: vk::Format) -> Result<Self, String> {
        unsafe {
            // ---- sky pipeline (fullscreen, no sets / vertex input) ----
            let sky_pc = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .offset(0)
                .size(std::mem::size_of::<SkyPush>() as u32)];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&sky_pc),
                    None,
                )
                .map_err(|e| format!("sky layout: {e}"))?;
            let pipeline = build_sky_pipeline(device, layout, color_format)?;
            Ok(Self { layout, pipeline })
        }
    }

    pub(super) fn destroy(&self, device: &ash::Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
        }
    }
}

impl WorldRenderer {
    /// Multiply the sky gradient and the distance-fog colour by the time of
    /// day (vanilla's `SKY_COLOR` / `FOG_COLOR` modifier tracks, both → near
    /// black at night). Without this the ground would darken at dusk while the
    /// sky stayed noon-blue.
    pub fn set_sky_tint(&mut self, sky: [f32; 3], fog: [f32; 3]) {
        self.scene.sky_tint = sky;
        self.scene.fog_tint = fog;
    }

    /// Set the camera biome sky/fog base color (LINEAR), M14. The horizon and
    /// (ratio-derived) zenith are then multiplied by the day/night `sky_tint`,
    /// and the fog by `fog_tint` — so this composes with the timeline and never
    /// requires a remesh (it is a per-frame uniform). Passing `SKY_HORIZON`
    /// restores the pre-M14 fixed sky.
    pub fn set_sky_fog_base(&mut self, sky_linear: [f32; 3], fog_linear: [f32; 3]) {
        self.scene.sky_base = sky_linear;
        self.scene.fog_base = fog_linear;
    }

    /// Select the dimension's skybox (`DimensionType.Skybox` →
    /// `LevelRenderer.addSkyPass`). The live paths set this every frame from
    /// `PlaySession::active_dimension_type`, so a dimension change or respawn
    /// takes effect on the next frame with no other bookkeeping.
    pub fn set_sky_mode(&mut self, mode: SkyMode) {
        self.scene.sky_mode = mode;
    }

    pub fn sky_mode(&self) -> SkyMode {
        self.scene.sky_mode
    }

    /// Override the distance-fog band (defaults from `REWO_FOG`).
    pub fn set_fog(&mut self, start: f32, end: f32) {
        self.scene.fog = [start, end.max(start + 1.0)];
    }

    /// Fullscreen gradient sky — drawn first, no depth test/write, so the
    /// terrain (reversed-Z GREATER, cleared 0.0) draws over it and the fog
    /// fade meets a matching horizon color.
    fn draw_sky(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let inv = glam::Mat4::from_cols_array_2d(&view_proj)
            .inverse()
            .to_cols_array_2d();
        let push = SkyPush {
            inv_view_proj: inv,
            horizon: [
                self.scene.sky_base[0] * self.scene.sky_tint[0],
                self.scene.sky_base[1] * self.scene.sky_tint[1],
                self.scene.sky_base[2] * self.scene.sky_tint[2],
                1.0,
            ],
            // Vanilla's `SKY_COLOR` (`MULTIPLY_RGB`) tints the whole sky disc.
            // Tinting only the horizon left a blue zenith at midnight. The zenith
            // keeps the fixed gradient ratio off the (biome) base color.
            zenith: [
                self.scene.sky_base[0] * ZENITH_RATIO[0] * self.scene.sky_tint[0],
                self.scene.sky_base[1] * ZENITH_RATIO[1] * self.scene.sky_tint[1],
                self.scene.sky_base[2] * ZENITH_RATIO[2] * self.scene.sky_tint[2],
                1.0,
            ],
        };
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.sky_gradient.pipeline,
            );
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            device.cmd_push_constants(
                cb,
                self.sky_gradient.layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                std::slice::from_raw_parts(
                    (&push as *const SkyPush) as *const u8,
                    std::mem::size_of::<SkyPush>(),
                ),
            );
            device.cmd_draw(cb, 3, 1, 0, 0);
        }
    }

    // Re-initialising a pass (rare: asset reload, reconnect) destroys the old
    // one instead of leaking its handles. Init-time only, so the idle inside
    // each `destroy` is acceptable here.
    fn retire_celestial(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.celestial.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    fn retire_end_sky(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.end_sky.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    /// Attach the celestial pass (sun/moon/stars/sunrise, M12). Callers that
    /// never attach it render the bare gradient sky. Textures come from the
    /// user's own client jar (`assets::CelestialTextures`).
    pub fn init_celestial(
        &mut self,
        gpu: &mut Gpu,
        tex: &crate::celestial::CelestialTextures,
    ) -> Result<(), String> {
        self.retire_celestial(gpu);
        self.celestial = Some(crate::celestial::CelestialPass::new(
            gpu,
            self.color_format,
            tex,
        )?);
        Ok(())
    }

    /// Update the clear-weather celestial state for this frame (angles in
    /// radians, sunrise colour linear rgb + straight alpha).
    pub fn set_celestial(&mut self, state: crate::celestial::CelestialState) {
        self.scene.celestial_state = state;
    }

    /// `SkyRenderState.endFlash{Intensity,XAngle,YAngle}` — `None` when the
    /// level has no flash state, which is every non-END skybox.
    pub fn set_end_flash(&mut self, flash: Option<(f32, f32, f32)>) {
        self.scene.end_flash = flash;
    }

    /// The attached celestial pass, if any (`rewo skyshot` reports its
    /// accepted-star count through this).
    pub fn celestial_pass(&self) -> Option<&crate::celestial::CelestialPass> {
        self.celestial.as_ref()
    }

    pub fn init_end_sky(
        &mut self,
        gpu: &mut Gpu,
        tex: &crate::end_sky::EndSkyImage,
    ) -> Result<(), String> {
        self.retire_end_sky(gpu);
        self.end_sky = Some(crate::end_sky::EndSkyPass::new(
            gpu,
            self.color_format,
            tex,
        )?);
        Ok(())
    }

    /// Whether [`SkyMode::End`] can actually draw its cube. A caller that wants
    /// the End sky and gets `false` is missing the jar asset — the renderer
    /// never substitutes an approximation.
    pub fn end_sky_ready(&self) -> bool {
        self.end_sky.is_some()
    }

    /// The sky pass of [`Self::draw`] (`LevelRenderer.addSkyPass`), drawn first
    /// with no depth write so the terrain overwrites it.
    pub(super) fn draw_sky_pass(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        // `LevelRenderer.addSkyPass`: NONE adds no pass at all, END draws only
        // the textured cube, otherwise the gradient disc + celestials.
        // Rotation-only sky space: `view_proj · T(eye)` cancels the camera
        // translation so the sky sits at infinity. Terrain then overwrites it
        // via reversed-Z where it exists.
        let sky_vp = (glam::Mat4::from_cols_array_2d(&view_proj)
            * glam::Mat4::from_translation(glam::Vec3::from_array(self.scene.camera_eye)))
        .to_cols_array_2d();
        match self.scene.sky_mode {
            SkyMode::None => {}
            SkyMode::End => {
                if let Some(end) = &self.end_sky {
                    end.draw(gpu, cb, sky_vp, extent);
                }
                // `addSkyPass`'s END arm: the sky cube, then the flash on top
                // of it, gated on `endFlashIntensity > 1.0E-5F`
                // (`LevelRenderer.java:333-338`) — a threshold rather than
                // `> 0`, which is what keeps the flash's ~1e-16 tail from
                // costing a draw. The sun, moon and stars are NOT in this arm:
                // the End has none.
                if let (Some(celestial), Some(flash)) = (&self.celestial, self.scene.end_flash) {
                    if flash.0 > 1.0e-5 {
                        celestial
                            .draw_end_flash(gpu, cb, sky_vp, extent, flash.0, flash.1, flash.2);
                    }
                }
            }
            SkyMode::Overworld => {
                self.draw_sky(gpu, cb, view_proj, extent);
                // Sun/moon/stars/sunrise (M12), between the gradient sky and
                // terrain.
                if let Some(celestial) = &self.celestial {
                    celestial.draw(gpu, cb, sky_vp, extent, &self.scene.celestial_state);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `SkyMode` must default to the pre-M16 Overworld sky, so every serverless
    /// path (demo/view/bench/meshshot) keeps drawing exactly what it did.
    #[test]
    fn sky_mode_defaults_to_overworld() {
        assert_eq!(SkyMode::default(), SkyMode::Overworld);
        assert_ne!(SkyMode::None, SkyMode::Overworld);
        assert_ne!(SkyMode::End, SkyMode::Overworld);
    }
}
