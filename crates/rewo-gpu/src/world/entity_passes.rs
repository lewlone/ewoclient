//! The entity passes: the world entity pass (mobs, players, block entities,
//! nametags, glint, trims), the inventory screen's player-preview pass, and the
//! per-player skin / cape / tab-face texture uploads that feed them.

use ash::vk;

use super::{unorm_of, WorldRenderer};
use crate::entities::{EntityDraw, EntityPass, FontData, MobTextures};
use crate::Gpu;

/// Which per-player atlas pool an eviction came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerTexture {
    Skin,
    Cape,
    Face,
}

/// The owner key the inventory preview's single player uploads under.
const PREVIEW_OWNER: u128 = 0;

impl WorldRenderer {
    fn retire_entities(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.entities.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    /// Attach the entity pass (mob models / capsules + nametags).
    /// Callers that never attach it (demo/view/bench snapshots) pay
    /// nothing. Mobs whose textures are missing fall back to capsules.
    /// Build the entity glint's pipeline and upload its sheet (M45).
    pub fn init_entity_glint(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> Result<(), String> {
        // M50: the glint renders into the UNORM view, so its pipeline must
        // declare that format. No counterpart format means no gamma-space
        // target, and the glint is dropped rather than blended in linear light.
        let Some(format) = unorm_of(self.color_format) else {
            return Ok(());
        };
        match self.entities.as_mut() {
            Some(p) => p.init_glint(gpu, rgba, w, h, format),
            None => Ok(()),
        }
    }

    /// M285 — the charged creeper's energy swirl sheet
    /// (`textures/entity/creeper/creeper_armor.png`). Like the glints it
    /// renders into the UNORM view; no counterpart format draws no swirl.
    pub fn init_entity_swirl(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> Result<(), String> {
        let Some(format) = unorm_of(self.color_format) else {
            return Ok(());
        };
        match self.entities.as_mut() {
            Some(p) => p.init_swirl(gpu, rgba, w, h, format),
            None => Ok(()),
        }
    }

    /// M285 — vertices in the swirl range after the last `set_entities`.
    pub fn swirl_vertex_count(&self) -> u32 {
        self.entities
            .as_ref()
            .map(|p| p.swirl_vertex_count())
            .unwrap_or(0)
    }

    /// Vertices in the worn-armour foil range after the last `set_entities`
    /// (M50). `0` with no entity pass. See
    /// [`crate::entities::EntityPass::armor_glint_vertex_count`].
    pub fn armor_glint_vertex_count(&self) -> u32 {
        self.entities
            .as_ref()
            .map(|p| p.armor_glint_vertex_count())
            .unwrap_or(0)
    }

    /// Upload `misc/enchanted_glint_armor.png` for the worn-armour foil (M50).
    /// Separate from [`Self::init_entity_glint`] because it is a separate
    /// sheet; they share the pipeline underneath.
    pub fn init_entity_armor_glint(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> Result<(), String> {
        // M50: the glint renders into the UNORM view, so its pipeline must
        // declare that format. No counterpart format means no gamma-space
        // target, and the glint is dropped rather than blended in linear light.
        let Some(format) = unorm_of(self.color_format) else {
            return Ok(());
        };
        match self.entities.as_mut() {
            Some(p) => p.init_armor_glint(gpu, rgba, w, h, format),
            None => Ok(()),
        }
    }

    pub fn init_entities(
        &mut self,
        gpu: &mut Gpu,
        font: Option<FontData<'_>>,
        tex: MobTextures<'_>,
    ) -> Result<(), String> {
        self.retire_entities(gpu);
        self.entities = Some(EntityPass::new(gpu, self.color_format, font, tex)?);
        Ok(())
    }

    /// Install the baked held-item models (M22). No-op before
    /// `init_entities`, which is the serverless-still case.
    pub fn set_held_items(&mut self, items: crate::held::HeldItems) {
        if let Some(e) = self.entities.as_mut() {
            e.set_held_items(items);
        }
    }

    /// M260 — append models built after the bake (see
    /// [`crate::entities::EntityPass::extend_held_items`]).
    pub fn extend_held_items(
        &mut self,
        models: Vec<(String, crate::held::HeldItemModel)>,
        textures: Vec<crate::held::HeldTexture>,
    ) {
        if let Some(e) = self.entities.as_mut() {
            e.extend_held_items(models, textures);
        }
    }

    /// Page in the atlas textures the named held items need, before the frame
    /// is built (M22). Uploading needs the device, and only the handful of
    /// items actually on screen should occupy the pool.
    /// The width of a string in font pixels — `Font.width`. Sign text centres
    /// on it, so the caller needs it before building a draw.
    pub fn text_width(&self, text: &str) -> f32 {
        self.entities.as_ref().map_or(0.0, |p| p.text_width(text))
    }

    /// The font's per-glyph advances, for callers that must lay text out
    /// before a draw exists — sign line breaking runs against the board width
    /// in these units (M27).
    ///
    /// Handed out rather than wrapped in a `split` method because the break
    /// rule is `StringSplitter`'s, which is vanilla *data* logic and lives in
    /// `rewo-data` beside the dye table; this crate owns the glyphs, not the
    /// rule.
    pub fn font_advance(&self) -> Option<&[u8; 256]> {
        self.entities.as_ref().map(|p| p.font_advance())
    }

    pub fn prepare_held_items(&mut self, gpu: &mut Gpu, names: &[&str]) -> Result<(), String> {
        match self.entities.as_mut() {
            Some(e) => e.prepare_held_items(gpu, names),
            None => Ok(()),
        }
    }

    /// `init_entities` with resource-pack CEM model overrides (M9).
    pub fn init_entities_with_cem(
        &mut self,
        gpu: &mut Gpu,
        font: Option<FontData<'_>>,
        tex: MobTextures<'_>,
        cem: std::collections::HashMap<crate::mobs::EntityModelKind, crate::mobs::Model>,
    ) -> Result<(), String> {
        self.retire_entities(gpu);
        self.entities = Some(EntityPass::new_with_cem(
            gpu,
            self.color_format,
            font,
            tex,
            cem,
        )?);
        Ok(())
    }

    /// The attached entity pass, if any — `rewo mobshot` uses it to iterate
    /// available mob models and their geometric ground truth.
    pub fn entity_pass(&self) -> Option<&EntityPass> {
        self.entities.as_ref()
    }

    /// [`Self::upload_player_skin`], into the **preview** pass (M36).
    ///
    /// A separate call because the two passes hold separate atlases, so a UV
    /// from one means nothing in the other — uploading once and reusing the
    /// offset would put a slice of some mob's texture on the player.
    pub fn upload_preview_skin(&mut self, gpu: &mut Gpu, rgba: &[u8]) -> Option<[f32; 2]> {
        let pass = self.preview.as_mut()?;
        // The preview draws one player, the local one: a fixed owner, so a
        // re-upload reuses its slot and nothing is ever evicted.
        match pass.upload_skin(gpu, PREVIEW_OWNER, rgba) {
            Ok((uv, _)) => Some(uv),
            Err(e) => {
                log::warn!("preview: skin upload failed: {e}");
                None
            }
        }
    }

    /// Put one permuted armour-trim sprite in the entity atlas's trim pool,
    /// or return where it already is (M48).
    pub fn upload_entity_trim(
        &mut self,
        gpu: &mut Gpu,
        key: &str,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> Option<(u32, u32)> {
        self.entities.as_mut()?.upload_trim(gpu, key, rgba, w, h)
    }

    /// Upload player `owner`'s 64x64 skin into the entity atlas and return the
    /// UV offset for its `EntityDraw::skin_uv`. `None` if the entity pass
    /// isn't initialized, or every skin slot is drawn this frame. A player
    /// whose slot this took is queued for [`Self::take_evicted_textures`].
    pub fn upload_player_skin(
        &mut self,
        gpu: &mut Gpu,
        owner: u128,
        rgba: &[u8],
    ) -> Option<[f32; 2]> {
        let pass = self.entities.as_mut()?;
        match pass.upload_skin(gpu, owner, rgba) {
            Ok((uv, evicted)) => {
                self.evicted_textures
                    .extend(evicted.map(|o| (o, PlayerTexture::Skin)));
                Some(uv)
            }
            Err(e) => {
                log::warn!("entities: skin upload failed: {e}");
                None
            }
        }
    }

    /// Claim a tab-list face slot and upload one player's 16x8 face strip
    /// (M155). `None` when there is no HUD pass, matching the four uploads
    /// around it.
    pub fn upload_tab_face(&mut self, gpu: &mut Gpu, owner: u128, rgba: &[u8]) -> Option<u8> {
        match self.hud.as_mut()?.upload_face(gpu, owner, rgba) {
            Ok((slot, evicted)) => {
                self.evicted_textures
                    .extend(evicted.map(|o| (o, PlayerTexture::Face)));
                Some(slot)
            }
            Err(e) => {
                log::warn!("hud: tab face upload failed: {e}");
                None
            }
        }
    }

    /// Claim a cape slot and upload one player's cape sheet (M60). Returns
    /// the slot's atlas origin in texels — see `EntityPass::upload_cape` for
    /// why this is an origin and not a UV delta.
    pub fn upload_player_cape(
        &mut self,
        gpu: &mut Gpu,
        owner: u128,
        rgba: &[u8],
    ) -> Option<(u32, u32)> {
        let pass = self.entities.as_mut()?;
        match pass.upload_cape(gpu, owner, rgba) {
            Ok((o, evicted)) => {
                self.evicted_textures
                    .extend(evicted.map(|o| (o, PlayerTexture::Cape)));
                Some(o)
            }
            Err(e) => {
                log::warn!("entities: cape upload failed: {e}");
                None
            }
        }
    }

    /// The same, into the **preview** pass (M64).
    ///
    /// A separate call for the reason [`Self::upload_preview_skin`] is one:
    /// the two passes hold separate atlases, so an origin from the world's is
    /// a rectangle of some mob's texture in the preview's. This is stronger
    /// than the skin case, not weaker — an origin is an absolute texel
    /// address rather than a delta, so reusing one would sample a *fixed*
    /// wrong place rather than merely a shifted one.
    pub fn upload_preview_cape(&mut self, gpu: &mut Gpu, rgba: &[u8]) -> Option<(u32, u32)> {
        let pass = self.preview.as_mut()?;
        match pass.upload_cape(gpu, PREVIEW_OWNER, rgba) {
            Ok((o, _)) => Some(o),
            Err(e) => {
                log::warn!("preview: cape upload failed: {e}");
                None
            }
        }
    }

    /// Rebuild this frame's entity geometry (no-op until `init_entities`).
    /// `time` (seconds) drives the ambient mob animations.
    pub fn set_entities(
        &mut self,
        draws: &[EntityDraw<'_>],
        cam_right: [f32; 3],
        cam_up: [f32; 3],
        time: f32,
    ) {
        self.set_entities_and_block_entities(draws, &[], &[], &[], cam_right, cam_up, time);
    }

    /// [`Self::set_entities`] plus the frame's block entities (M25b). They
    /// share the entity pass's buffer, atlas and shader — a chest is textured
    /// geometry at a world position, which is what that pass already draws.
    pub fn set_entities_and_block_entities(
        &mut self,
        draws: &[EntityDraw<'_>],
        block_entities: &[crate::entities::BlockEntityDraw<'_>],
        frame_maps: &[crate::entities::FrameMapDraw<'_>],
        world_text: &[crate::entities::WorldTextDraw<'_>],
        cam_right: [f32; 3],
        cam_up: [f32; 3],
        time: f32,
    ) {
        if let Some(pass) = self.entities.as_mut() {
            pass.set_draws(
                draws,
                block_entities,
                frame_maps,
                world_text,
                cam_right,
                cam_up,
                time,
                self.scene.camera_eye,
            );
        }
    }

    /// M247 — the quads the last frame emitted per block-entity draw (empty
    /// before `init_entities`).
    pub fn block_entity_quads(&self) -> &[u32] {
        self.entities
            .as_ref()
            .map_or(&[][..], |e| e.block_entity_quads())
    }

    /// The map decoration sprites for framed maps (M194), by raw type id —
    /// [`crate::entities::EntityPass::init_map_decorations`]. No-op until
    /// `init_entities`.
    pub fn init_map_decorations(
        &mut self,
        gpu: &mut Gpu,
        sprites: &[Option<&[u8]>],
    ) -> Result<(), String> {
        match self.entities.as_mut() {
            Some(p) => p.init_map_decorations(gpu, sprites),
            None => Ok(()),
        }
    }

    /// Page in this frame's framed maps (M194) —
    /// [`crate::entities::EntityPass::prepare_frame_maps`].
    pub fn prepare_frame_maps(
        &mut self,
        gpu: &mut Gpu,
        maps: &[(i32, u64)],
        rgba_of: &mut dyn FnMut(i32) -> Option<Vec<u8>>,
    ) -> Result<(), String> {
        match self.entities.as_mut() {
            Some(p) => p.prepare_frame_maps(gpu, maps, rgba_of),
            None => Ok(()),
        }
    }

    /// Players whose skin, cape or tab face lost its atlas slot to another
    /// player's upload since the last call (least recently drawn first to go;
    /// a slot drawn in the current frame is never taken). Each one's stored
    /// address now points at someone else's texture: drop it, and upload that
    /// player's texture again when it is next needed.
    pub fn take_evicted_textures(&mut self) -> Vec<(u128, PlayerTexture)> {
        std::mem::take(&mut self.evicted_textures)
    }

    /// Build the preview's entity pass. Idempotent, and cheap to call every
    /// time the screen opens.
    pub fn init_preview(
        &mut self,
        gpu: &mut Gpu,
        font: Option<crate::entities::FontData<'_>>,
        tex: crate::entities::MobTextures<'_>,
    ) -> Result<(), String> {
        if self.preview.is_some() {
            return Ok(());
        }
        self.preview = Some(crate::entities::EntityPass::new(
            gpu,
            self.color_format,
            font,
            tex,
        )?);
        Ok(())
    }

    /// This frame's preview: the model to pose, the matrix that places it, and
    /// the window it is clipped to. `None` closes it.
    pub fn set_preview(&mut self, draw: Option<(&EntityDraw<'_>, [[f32; 4]; 4], vk::Rect2D)>) {
        match (draw, self.preview.as_mut()) {
            (Some((d, vp, rect)), Some(pass)) => {
                // The camera basis only orients billboards — nametags — and the
                // preview draws none, so any orthonormal pair will do.
                pass.set_draws(
                    std::slice::from_ref(d),
                    &[],
                    &[],
                    &[],
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    0.0,
                    [0.0, 0.0, 0.0],
                );
                self.gui.preview_state = Some((vp, rect));
            }
            _ => self.gui.preview_state = None,
        }
    }

    pub fn preview_ready(&self) -> bool {
        self.preview.is_some()
    }

    /// Whether this frame has a preview model to draw (M220, render-check
    /// r86): read back out of the renderer, where the draw reads it.
    pub fn preview_active(&self) -> bool {
        self.gui.preview_state.is_some()
    }
}
