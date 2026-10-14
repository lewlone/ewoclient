//! The frame's draw order: `WorldRenderer::draw` and its world half. The sky
//! and screen-space halves live with their passes (`sky.rs`, `gui_passes.rs`).

use ash::vk;

use super::WorldRenderer;
use crate::Gpu;

impl WorldRenderer {
    /// In-pass draws, in blend-correct order: gradient sky (background) →
    /// opaque terrain (indirect) → opaque entity capsules/models →
    /// translucent water (CPU-sorted back-to-front) → nametag text.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        // Advance the `LightmapExtra` ring once per frame and publish this
        // frame's ambient colour into the slot the terrain/water draws will
        // bind. Done here (not in `set_lightmap_state`) so exactly one slot is
        // consumed per frame regardless of how often the state was set.
        self.lm.advance(&self.scene.lightmap);

        self.draw_sky_pass(gpu, cb, view_proj, extent);
        self.draw_scene_passes(gpu, cb, view_proj, extent);
        self.draw_gui_passes(gpu, cb, extent);
    }

    /// The world half of [`Self::draw`]: opaque terrain, the crack overlay,
    /// leashes and the selection outline, solid entities and their glint, end
    /// portals, clouds, translucent water, particles, weather, the world border
    /// and nametags — in blend-correct order.
    fn draw_scene_passes(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        if self.culler.column_count > 0 {
            self.draw_terrain(gpu, cb, view_proj, extent);
        }
        // The block-break crack (M81), immediately after the terrain whose
        // depth it re-tests against and whose colour it multiplies. Before the
        // entities, so a mob standing in front of a cracked block covers the
        // crack rather than being darkened by it. In gamma space, because
        // `2·src·dst` is not invariant under the sRGB transfer function.
        if let Some(pass) = &self.crumbling {
            if !pass.is_empty() {
                self.in_gamma_space(gpu, cb, extent, || pass.draw(gpu, cb, view_proj, extent));
            }
        }
        // Selection outline over the terrain (depth-tested against it).
        self.draw_leash(gpu, cb, view_proj, extent);
        self.draw_selection(gpu, cb, view_proj, extent);
        if let Some(pass) = &self.entities {
            pass.draw_solid(gpu, cb, view_proj, extent);
            // M52: the vanilla emissive mob layers (mob eyes, the warden's
            // glow) ride straight on top of the solid models — that is where
            // vanilla's `order(1)` layer submits land, under everything below.
            pass.draw_emissive(gpu, cb, view_proj, extent);
            // M50: the worn-armour foil, then the trim. All three depth-test
            // `EQUAL` against the solid geometry just written, so all three
            // must follow it — and the foil precedes the trim because
            // `renderLayers` submits layer, foil, trim in that order and
            // `SubmitNodeStorage` drains its phases in ascending `order`. A
            // trim's opaque texels therefore paint over the foil.
            // Both foils blend in gamma space; the trim between them is an
            // ordinary alpha blend and stays in the sRGB scope.
            self.in_gamma_space(gpu, cb, extent, || {
                pass.draw_armor_glint(gpu, cb, view_proj, extent)
            });
            pass.draw_trim(gpu, cb, view_proj, extent);
            // The glint sits on the solid geometry it just wrote — its depth
            // test is `EQUAL`, so it has to follow, and it precedes the
            // translucent passes because it is additive over opaque pixels.
            self.in_gamma_space(gpu, cb, extent, || {
                pass.draw_glint(gpu, cb, view_proj, extent)
            });
        }
        // End portals (M32). After solid geometry and before the translucent
        // pass: the shader writes opaque pixels and depth, so it belongs with
        // the solids, and drawing it after them lets terrain in front occlude
        // it exactly as vanilla's depth-tested pipeline does.
        if let Some(pass) = &self.end_portal {
            pass.draw(gpu, cb, view_proj, self.scene.end_portal_time, extent);
        }
        // Clouds (M33) sit between the solids and the translucent pass: they
        // are translucent themselves and must blend over terrain, but they are
        // also depth-writing, so water in front of them still sorts correctly.
        if let Some(pass) = &self.clouds {
            pass.draw(gpu, cb, view_proj, extent);
        }
        self.draw_translucent(gpu, cb, view_proj, extent);
        // Particles (M37) sit with the other blended, depth-tested,
        // non-depth-writing passes — after everything solid and translucent so
        // terrain occludes them, before weather so rain layers over smoke.
        if let Some(pass) = &self.particles {
            let (light, sky_col) = self.scene.lightmap.push_words();
            pass.draw(
                gpu,
                cb,
                view_proj,
                (light, sky_col, self.scene.lightmap.extra_words()),
                extent,
            );
        }
        // Rain and snow come after everything solid and translucent, blended
        // and depth-tested but not depth-writing — vanilla's
        // `WEATHER_NO_DEPTH_WRITE` branch.
        if let Some(pass) = &self.weather {
            let (light, sky_col) = self.scene.lightmap.push_words();
            pass.draw(
                gpu,
                cb,
                view_proj,
                (light, sky_col, self.scene.lightmap.extra_words()),
                extent,
            );
        }
        // The world border (M80) draws in vanilla's `weather` frame-graph pass,
        // immediately after the rain and snow — and unlike them it *writes*
        // depth, so it must come after everything it should not be occluded by.
        if let Some(pass) = &self.border {
            pass.draw(gpu, cb, view_proj, extent);
        }
        if let Some(pass) = &self.entities {
            pass.draw_text(gpu, cb, view_proj, extent);
        }
    }
}
