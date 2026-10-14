//! The world-space effect passes layered over the scene: the end portal and
//! gateway, clouds, the world-border wall, rain and snow, particles and the
//! block-break crack overlay. Each is an optional pass attached by an `init_*`
//! call (re-attaching retires the old one) and fed per frame by a `set_*`.

use super::{unorm_of, WorldRenderer};
use crate::Gpu;

impl WorldRenderer {
    fn retire_end_portal(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.end_portal.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    fn retire_clouds(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.clouds.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    fn retire_border(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.border.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    fn retire_weather(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.weather.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    fn retire_particles(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.particles.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    fn retire_crumbling(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.crumbling.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    /// Attach the End skybox pass (M16), from the user's own client jar's
    /// `assets/minecraft/textures/environment/end_sky.png`. Without it,
    /// [`SkyMode::End`] draws nothing — `end_sky_ready` reports which.
    /// Build the end-portal pass from `environment/end_sky.png` and
    /// `entity/end_portal/end_portal.png` (M32). Without both, no portal draws.
    pub fn init_end_portal(
        &mut self,
        gpu: &mut Gpu,
        sky: &crate::end_portal::PortalImage,
        portal: &crate::end_portal::PortalImage,
    ) -> Result<(), String> {
        self.retire_end_portal(gpu);
        self.end_portal = Some(crate::end_portal::EndPortalPass::new(
            gpu,
            self.color_format,
            sky,
            portal,
        )?);
        Ok(())
    }

    pub fn end_portal_ready(&self) -> bool {
        self.end_portal.is_some()
    }

    /// Build the cloud pass (M33). No texture: `clouds.png` is decoded on the
    /// CPU into cell data, and the shader carries its six face colours inline.
    pub fn init_clouds(&mut self, gpu: &mut Gpu) -> Result<(), String> {
        self.retire_clouds(gpu);
        self.clouds = Some(crate::clouds::CloudPass::new(gpu, self.color_format)?);
        Ok(())
    }

    pub fn clouds_ready(&self) -> bool {
        self.clouds.is_some()
    }

    /// This frame's cloud deck. An empty face list draws nothing.
    pub fn set_clouds(
        &mut self,
        gpu: &mut Gpu,
        draw: &crate::clouds::CloudDraw,
    ) -> Result<(), String> {
        match self.clouds.as_mut() {
            Some(pass) => pass.set_draw(gpu, draw),
            None => Ok(()),
        }
    }

    /// Build the world-border pass (M80) over `misc/forcefield.png`.
    pub fn init_border(
        &mut self,
        gpu: &mut Gpu,
        tex: &crate::border::BorderImage,
    ) -> Result<(), String> {
        self.retire_border(gpu);
        self.border = Some(crate::border::BorderPass::new(gpu, self.color_format, tex)?);
        Ok(())
    }

    pub fn border_ready(&self) -> bool {
        self.border.is_some()
    }

    /// This frame's border wall. `None` clears it — a frame whose `extract`
    /// decided the wall is invisible must draw nothing rather than the last
    /// visible geometry.
    pub fn set_border(
        &mut self,
        gpu: &mut Gpu,
        draw: Option<&crate::border::BorderDraw>,
    ) -> Result<(), String> {
        match self.border.as_mut() {
            Some(pass) => pass.set_draw(gpu, draw),
            None => Ok(()),
        }
    }

    /// Build the weather pass (M33) from `rain.png` and `snow.png`.
    pub fn init_weather(
        &mut self,
        gpu: &mut Gpu,
        rain: &crate::weather::WeatherImage,
        snow: &crate::weather::WeatherImage,
    ) -> Result<(), String> {
        self.retire_weather(gpu);
        self.weather = Some(crate::weather::WeatherPass::new(
            gpu,
            self.color_format,
            rain,
            snow,
        )?);
        Ok(())
    }

    pub fn weather_ready(&self) -> bool {
        self.weather.is_some()
    }

    /// This frame's rain and snow geometry.
    pub fn set_weather(
        &mut self,
        gpu: &mut Gpu,
        draw: &crate::weather::WeatherDraw,
    ) -> Result<(), String> {
        match self.weather.as_mut() {
            Some(pass) => pass.set_draw(gpu, draw),
            None => Ok(()),
        }
    }

    /// Build the particle pass (M37) from the particle atlas.
    pub fn init_particles(
        &mut self,
        gpu: &mut Gpu,
        atlas: &crate::particles::ParticleAtlas,
    ) -> Result<(), String> {
        self.retire_particles(gpu);
        self.particles = Some(crate::particles::ParticlePass::new(
            gpu,
            self.color_format,
            atlas,
        )?);
        Ok(())
    }

    pub fn particles_ready(&self) -> bool {
        self.particles.is_some()
    }

    /// This frame's particle geometry.
    pub fn set_particles(
        &mut self,
        gpu: &mut Gpu,
        draw: &crate::particles::ParticleDraw,
    ) -> Result<(), String> {
        match self.particles.as_mut() {
            Some(pass) => pass.set_draw(gpu, draw),
            None => Ok(()),
        }
    }

    /// Build the block-break crumbling pass (M81) from the ten
    /// `destroy_stage_N` textures.
    ///
    /// Built against **`unorm_of(self.color_format)`**, not the format
    /// itself: the pass's `2·src·dst` blend has to run on gamma-encoded
    /// numbers, so it is drawn inside [`Self::with_gamma_space`] and its
    /// pipeline must declare the format that scope reopens with. A pass built
    /// against the sRGB format and drawn there is a validation error, not a
    /// colour shift — the same contract M50's glint records.
    pub fn init_crumbling(
        &mut self,
        gpu: &mut Gpu,
        stages: &[Vec<u8>],
        size: u32,
    ) -> Result<(), String> {
        let Some(format) = unorm_of(self.color_format) else {
            log::warn!(
                "world: no UNORM counterpart for {:?} — no block-break overlay",
                self.color_format
            );
            return Ok(());
        };
        self.retire_crumbling(gpu);
        self.crumbling = Some(crate::crumbling::CrumblingPass::new(
            gpu, format, stages, size,
        )?);
        Ok(())
    }

    pub fn crumbling_ready(&self) -> bool {
        self.crumbling.is_some()
    }

    /// This frame's block-break decal geometry.
    pub fn set_crumbling(
        &mut self,
        gpu: &mut Gpu,
        verts: &[crate::crumbling::CrumblingVertex],
    ) -> Result<(), String> {
        match self.crumbling.as_mut() {
            Some(pass) => pass.set_verts(gpu, verts),
            None => Ok(()),
        }
    }

    /// This frame's portals. `game_time` is the raw world clock; the pass
    /// converts it to the daily fraction the shader expects.
    pub fn set_end_portals(
        &mut self,
        gpu: &mut Gpu,
        draws: &[crate::end_portal::PortalDraw],
        game_time: i64,
    ) -> Result<(), String> {
        self.scene.end_portal_time = crate::end_portal::game_time_fraction(game_time);
        match self.end_portal.as_mut() {
            Some(p) => p.set_draws(gpu, draws),
            None => Ok(()),
        }
    }
}
