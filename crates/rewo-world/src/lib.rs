//! rewo-world — authoritative world state decoded from the wire.
//!
//! M1 scope: paletted chunk sections, columns keyed by dimension height,
//! block-light/sky-light arrays, an entity table, a `block_state_at` query,
//! and a deterministic world digest (for replay-equivalence in the DoD).
//! Prediction/physics land in M3; this is the read model.

pub mod abilities;
pub mod ambient;
pub mod music;
pub mod anvil;
pub mod edit_box;
pub mod merchant_screen;
pub mod ghost_slots;
pub mod recipe_book_screen;
pub mod recipe_overlay;
pub mod recipe_search;
pub mod stacked_contents;
pub mod attributes;
pub mod biome;
pub mod biome_noise;
pub mod active_text;
pub mod block_entities;
pub mod border;
pub mod cape;
pub mod celestial;
pub mod chat;
pub mod chat_events;
pub mod chat_screen;
pub mod chat_style;
pub mod chat_translate;
pub mod command_suggestions;
pub mod chunk;
pub mod chunk_cache;
pub mod conduit;
pub mod daylight;
pub mod death_screen;
pub mod destruction;
pub mod dimension;
pub mod disconnect_screen;
pub mod entities;
pub mod entity_pick;
pub mod inventory;
pub mod label;
pub mod layout;
pub mod light;
pub mod lightmap;
pub mod menu;
pub mod menu_layout;
pub mod menu_screen;
pub mod nine_slice;
pub mod minecart;
pub mod palette;
pub mod particles;
pub mod pause_screen;
pub mod physics;
pub mod pickup;
pub mod raycast;
pub mod riding;
pub mod rotation;
pub mod screen;
pub mod stats;
pub mod stats_screen;
pub mod string_splitter;
pub mod suggestions;
pub mod server_links_screen;
pub mod wavy_cape;
pub mod weather;

use std::collections::HashMap;
use std::sync::Arc;

use dimension::{CardinalLightType, CardinalLighting, DimensionShape, DimensionTypeDef};

/// The whole client-visible world for one dimension.
///
/// Columns are stored behind `Arc` (the plan's §4 copy-on-write model):
/// readers — mesh workers, collision queries — clone the handle and get a
/// stable immutable view; writers go through `Arc::make_mut`, which only
/// deep-clones a column in the rare case a worker still holds it.
///
/// A `World` is bound to exactly one dimension, the way vanilla's `ClientLevel`
/// is: `shape`, [`World::has_sky_light`] and [`World::cardinal_light`] all come
/// from the same `DimensionType`. A dimension change replaces the whole struct
/// rather than mutating these in place — see `rewo_net::play`.
pub struct World {
    pub shape: DimensionShape,
    columns: HashMap<(i32, i32), Arc<chunk::Column>>,
    pub entities: entities::EntityTable,
    /// Per-biome color context (registry + colormaps + `biomeZoomSeed`), behind
    /// an `Arc` so `snapshot_3x3` clones it for free. `None` for synthetic /
    /// no-biome worlds (the demo) — the mesher then keeps the legacy pre-tinted
    /// path, so those renders stay byte-identical.
    biome: Option<Arc<biome::BiomeContext>>,
    /// `DimensionType.hasSkyLight`. When false the sky channel has no engine at
    /// all — the server sends no sky data and `light::LightEngine` never seeds
    /// or recomputes it, so a Nether column can't report the impossible sky 15
    /// a stale Overworld world produced.
    has_sky_light: bool,
    /// `DimensionType.cardinalLightType`, and the table it resolves to. The
    /// mesher reads these to pick each face's shade code.
    cardinal_light_type: CardinalLightType,
    cardinal_light: CardinalLighting,
    /// The world's block entities, keyed by absolute position (M25). Lives on
    /// the world rather than the column because the render pass wants them by
    /// area; column load / unload keeps it in step.
    pub block_entities: block_entities::BlockEntities,
    /// `ClientLevel.destroyingBlocks` / `destructionProgress` — the crack
    /// overlay somebody else's mining paints (M81).
    pub destruction: destruction::DestructionProgress,
    /// `ItemPickupParticle`s in flight (M81). On the world rather than beside
    /// the entities because the entity they draw has already been removed.
    pub pickups: pickup::Pickups,
}

impl World {
    /// A world with the Overworld lighting contract (sky light on, DEFAULT
    /// cardinal lighting). Every pre-M16 caller — demos, tests, snapshot
    /// renders — keeps its exact behaviour through this constructor.
    pub fn new(shape: DimensionShape) -> Self {
        Self {
            shape,
            columns: HashMap::new(),
            entities: entities::EntityTable::default(),
            biome: None,
            has_sky_light: true,
            cardinal_light_type: CardinalLightType::DEFAULT,
            cardinal_light: CardinalLighting::DEFAULT,
            block_entities: block_entities::BlockEntities::default(),
            destruction: destruction::DestructionProgress::default(),
            pickups: pickup::Pickups::default(),
        }
    }

    /// A fresh, empty world configured for one dimension-type entry — the
    /// `new ClientLevel(...)` of `handleLogin` / `handleRespawn`.
    pub fn for_dimension(def: &DimensionTypeDef) -> Self {
        Self {
            shape: def.shape,
            columns: HashMap::new(),
            entities: entities::EntityTable::default(),
            biome: None,
            has_sky_light: def.has_sky_light,
            cardinal_light_type: def.cardinal_light_type,
            cardinal_light: def.cardinal_light,
            block_entities: block_entities::BlockEntities::default(),
            destruction: destruction::DestructionProgress::default(),
            pickups: pickup::Pickups::default(),
        }
    }

    /// Re-point an existing world at a dimension type **without** touching its
    /// columns. Only valid when the dimension key did not change (a same-key
    /// respawn, or the login packet configuring the world it was handed);
    /// a real dimension change must build a fresh world, because the old
    /// columns were generated for the old vertical shape.
    pub fn apply_dimension_type(&mut self, def: &DimensionTypeDef) {
        self.shape = def.shape;
        self.has_sky_light = def.has_sky_light;
        self.cardinal_light_type = def.cardinal_light_type;
        self.cardinal_light = def.cardinal_light;
    }

    /// `DimensionType.hasSkyLight`.
    pub fn has_sky_light(&self) -> bool {
        self.has_sky_light
    }

    pub fn set_has_sky_light(&mut self, has_sky_light: bool) {
        self.has_sky_light = has_sky_light;
    }

    pub fn cardinal_light(&self) -> CardinalLighting {
        self.cardinal_light
    }

    pub fn cardinal_light_type(&self) -> CardinalLightType {
        self.cardinal_light_type
    }

    pub fn set_cardinal_light_type(&mut self, t: CardinalLightType) {
        self.cardinal_light_type = t;
        self.cardinal_light = t.get();
    }

    /// Attach (or replace) the biome color context.
    pub fn set_biome_context(&mut self, ctx: Arc<biome::BiomeContext>) {
        self.biome = Some(ctx);
    }

    /// The biome context, if any — the mesher checks this to choose the
    /// dynamic-tint vs legacy-layer path.
    pub fn biome_context(&self) -> Option<&Arc<biome::BiomeContext>> {
        self.biome.as_ref()
    }

    pub fn insert_column(&mut self, cx: i32, cz: i32, column: chunk::Column) {
        // A level-chunk packet is the authoritative block-entity list for its
        // column, so the previous contents go first. Without the clear, a
        // re-sent chunk would leave a broken chest's block entity behind —
        // invisible today, but it would resurrect the moment a renderer exists.
        self.block_entities.remove_column(cx, cz);
        for (pos, be) in &column.block_entities {
            self.block_entities.insert(*pos, be.clone());
        }
        self.columns.insert((cx, cz), Arc::new(column));
    }

    /// Apply a `ClientboundBlockEntityDataPacket` — one block entity at an
    /// absolute position (M25).
    ///
    /// Vanilla's handler looks the block entity up in the world and calls
    /// `onDataPacket`; a position with no block entity is ignored. Rewo has no
    /// per-block-entity behaviour to run, so it stores the payload — but it
    /// keeps the *existence* rule, because inventing an entry for an arbitrary
    /// position would let a stray packet paint a chest into thin air.
    pub fn set_block_entity_data(
        &mut self,
        pos: block_entities::BlockEntityPos,
        type_id: i32,
        data: rewo_proto::nbt::Nbt,
    ) -> bool {
        if self.block_entities.get(pos).is_none() {
            return false;
        }
        self.block_entities
            .insert(pos, block_entities::BlockEntity { type_id, data });
        true
    }

    /// Ensure an all-air, fully-lit column exists (synthetic scenes).
    pub fn ensure_column(&mut self, cx: i32, cz: i32) {
        self.columns
            .entry((cx, cz))
            .or_insert_with(|| Arc::new(chunk::Column::empty_lit(&self.shape, cx, cz)));
    }

    pub fn forget_column(&mut self, cx: i32, cz: i32) {
        self.columns.remove(&(cx, cz));
        self.block_entities.remove_column(cx, cz);
    }

    pub fn loaded_columns(&self) -> usize {
        self.columns.len()
    }

    /// Global block state id at world coords, or 0 (air) if unloaded /
    /// out of vertical range.
    pub fn block_state_at(&self, x: i32, y: i32, z: i32) -> u32 {
        let cx = x >> 4;
        let cz = z >> 4;
        let Some(col) = self.columns.get(&(cx, cz)) else {
            return 0;
        };
        col.block_state_at(&self.shape, x & 15, y, z & 15)
    }

    /// True when the column holding (x,z) is loaded.
    pub fn is_loaded(&self, x: i32, z: i32) -> bool {
        self.columns.contains_key(&(x >> 4, z >> 4))
    }

    /// Combined light level 0..15 at world coords (max of block + sky).
    /// Unloaded or above-world positions read as full-bright — but only in a
    /// sky-lit dimension. A no-skylight dimension has no sky channel at all, so
    /// the out-of-bounds read is dark; reporting 15 there is the "impossible
    /// Nether sky" the stale-world path produced at every column edge.
    pub fn brightness_at(&self, x: i32, y: i32, z: i32) -> u8 {
        let Some(col) = self.columns.get(&(x >> 4, z >> 4)) else {
            return self.unloaded_sky();
        };
        if self.has_sky_light {
            col.brightness_at(&self.shape, x & 15, y, z & 15)
        } else {
            // `Column::light_at` deliberately uses the Overworld-compatible
            // full-sky fallback for an absent sparse section. A dimension
            // without a sky light engine must override that fallback even
            // inside a loaded column; only the stored block channel exists.
            col.light_at(&self.shape, x & 15, y, z & 15).0
        }
    }

    /// Separate (block, sky) light — the F3 readout. An unloaded column
    /// reports the same sky fallback as `brightness_at`.
    pub fn light_at(&self, x: i32, y: i32, z: i32) -> (u8, u8) {
        let Some(col) = self.columns.get(&(x >> 4, z >> 4)) else {
            return (0, self.unloaded_sky());
        };
        let (block, sky) = col.light_at(&self.shape, x & 15, y, z & 15);
        (block, if self.has_sky_light { sky } else { 0 })
    }

    /// The sky level an unloaded column reads as: full-bright where a sky
    /// channel exists, dark where it does not.
    fn unloaded_sky(&self) -> u8 {
        if self.has_sky_light {
            15
        } else {
            0
        }
    }

    /// Mutable column access — the `light_update` path re-lights an already
    /// loaded column in place.
    /// `SkullBlockEntity.animation` for every skull in the world (M29).
    ///
    /// Lives here rather than on `BlockEntities` because the driver is a
    /// **block state** property (`SkullBlock.POWERED`), not anything in the
    /// block entity's NBT — a skull animates because the note block under it
    /// is powered, and only the world knows that.
    ///
    /// `powered` is the set of block-state ids that carry `powered=true`,
    /// resolved once from `blocks.json` rather than probed per tick.
    /// `ConduitBlockEntity.clientTick` for every conduit in the world (M30).
    ///
    /// Like the skull tick this lives here because it needs BLOCK STATES: a
    /// conduit's activation is a scan of the water and prismarine around it,
    /// which nothing in its NBT records and no packet reports. `game_time`
    /// selects the re-scan ticks — vanilla runs `updateShape` only on
    /// `gameTime % 40 == 0`, so a conduit snaps on at the next multiple rather
    /// than flickering while a frame is built.
    pub fn tick_conduits(
        &mut self,
        conduit_states: &std::collections::HashSet<u32>,
        water: &[bool],
        frame: &[bool],
        game_time: i64,
    ) {
        if conduit_states.is_empty() {
            return;
        }
        let rescan = game_time % 40 == 0;
        for pos in self.block_entities.positions() {
            if !conduit_states.contains(&self.block_state_at(pos.x, pos.y, pos.z)) {
                continue;
            }
            let shape = rescan.then(|| {
                conduit::scan(
                    (pos.x, pos.y, pos.z),
                    |x, y, z| self.block_state_at(x, y, z),
                    water,
                    frame,
                )
            });
            self.block_entities.tick_conduit(pos, shape);
        }
    }

    /// `Entity.isUnderWater()` for a point, given the per-state water table
    /// (M141h).
    ///
    /// Vanilla is `wasEyeInWater && isInWater()`, and `wasEyeInWater` is
    /// `EntityFluidInteraction`'s `eyesInside`: the eye must be in the
    /// entity's **own block column** and between the fluid's bottom and top,
    /// where the top is `blockY + fluidHeight` and a source block's height is
    /// **8/9**, not 1 (`FlowingFluid.getOwnHeight` is `amount / 9`) — unless
    /// the block above is also the same fluid, in which case it is 1.
    ///
    /// **This is an approximation and here is exactly what it is**: the eye's
    /// block is tested for water and nothing else. So it over-reports by up to
    /// **1/9 of a block at a free surface** — an eye in the top ninth of a
    /// surface water block reads as submerged where vanilla says it is not —
    /// and it does not consult `isInWater()`, which cannot disagree here
    /// because an eye inside water implies water overlapping the box.
    ///
    /// Closing the gap wants the fluid *level* per state, which the bake does
    /// not carry (M30's scan needed only a boolean). The consequence is
    /// bounded to a tenth of a block at the waterline, where the audible
    /// effect is the riding loops swapping fractionally early.
    pub fn is_water_at_point(&self, x: f64, y: f64, z: f64, water: &[bool]) -> bool {
        let (bx, by, bz) = (
            x.floor() as i32,
            y.floor() as i32,
            z.floor() as i32,
        );
        let state = self.block_state_at(bx, by, bz) as usize;
        water.get(state).copied().unwrap_or(false)
    }

    /// `environmentAttributes().getValue(AMBIENT_SOUNDS, player.position())`
    /// — the record `BiomeAmbientSoundsHandler` reads (M142d).
    ///
    /// `base` is the dimension type's layer, which for the Overworld is
    /// `LEGACY_CAVE_SETTINGS` and for the Nether is nothing.
    ///
    /// **The sample is the RAW QUART**, `QuartPos.fromBlock(Mth.floor(c))`
    /// straight into `getNoiseBiome` — not the fiddled `BiomeManager.getBiome`
    /// that M14's block tint uses, and not any blend. Two independent guards
    /// make it so: the caller passes a null interpolator, and `AMBIENT_SOUNDS`
    /// is `ofNotInterpolated`. Reusing the colour path's resolver here shifts
    /// the switch point by a seed-dependent few blocks.
    ///
    /// **The Y quart is included**, so flying up out of a cave biome changes
    /// the loop.
    ///
    /// With no biome context attached the base is returned unlayered, which is
    /// the honest answer for a synthetic world: the dimension still has its
    /// say, and no biome can override what does not exist.
    pub fn ambient_sounds_at(
        &self,
        pos: [f64; 3],
        base: &crate::ambient::AmbientSounds,
    ) -> crate::ambient::AmbientSounds {
        let Some(ctx) = self.biome.as_ref() else {
            return base.clone();
        };
        let id = self.noise_biome_at_quart(
            crate::ambient::quart_from_block_coord(pos[0]),
            crate::ambient::quart_from_block_coord(pos[1]),
            crate::ambient::quart_from_block_coord(pos[2]),
        );
        crate::ambient::AmbientSounds::resolve(base, ctx.registry.biomes.get(id as usize))
    }

    /// `BACKGROUND_MUSIC` at a position — the dimension's base, replaced by the
    /// biome's if that biome declares one (M146).
    ///
    /// The exact twin of [`Self::ambient_sounds_at`], sampled at the same quart
    /// and through the same registry, because the two attributes travel
    /// together and a second sampling rule is how they would come to disagree.
    pub fn background_music_at(
        &self,
        pos: [f64; 3],
        base: &crate::music::BackgroundMusic,
    ) -> crate::music::BackgroundMusic {
        let Some(ctx) = self.biome.as_ref() else {
            return base.clone();
        };
        let id = self.noise_biome_at_quart(
            crate::ambient::quart_from_block_coord(pos[0]),
            crate::ambient::quart_from_block_coord(pos[1]),
            crate::ambient::quart_from_block_coord(pos[2]),
        );
        crate::music::BackgroundMusic::resolve(
            base,
            ctx.registry
                .biomes
                .get(id as usize)
                .and_then(|b| b.background_music.as_ref()),
        )
    }

    /// `level.getBlockStatesIfLoaded(box).filter(is BUBBLE_COLUMN).findFirst()`
    /// — the scan `BubbleColumnAmbientSoundHandler` runs over the player's
    /// torso box (M142c). `Some(drag)` for the first bubble column found.
    ///
    /// `drag` is `BakedAssets::bubble_column_drag`, indexed by state id.
    ///
    /// Three things about this are transcribed rather than chosen:
    ///
    /// 1. **Both bounds are FLOORED**, `min` and `max` alike
    ///    (`LevelReader.java:47-52`). So a box whose max lands exactly on an
    ///    integer includes that block — which is precisely what the handler's
    ///    `deflate(1.0E-6)` exists to prevent, and why that epsilon is
    ///    reproduced rather than rounded away.
    /// 2. **A missing chunk yields an EMPTY scan, not a partial one and not an
    ///    error.** `getBlockStatesIfLoaded` guards the whole box with
    ///    `hasChunksAt` and returns `Stream.empty()` if any covered chunk is
    ///    absent (`:53`). That is a silent no-op which the handler reads as
    ///    "no column", so it clears its edge latch — and therefore *re-arms*,
    ///    firing the moment the chunk arrives. Reading through to the world
    ///    anyway, as an optimisation, loses that re-arm.
    /// 3. **X varies fastest and Z slowest, so the PRIORITY is the reverse.**
    ///    `BlockPos.betweenClosed` (`:410-415`) computes `x = i % width`,
    ///    `y = (i / width) % height`, `z = i / width / height`, which visits
    ///    every block of one Z slice before any of the next. Paired with
    ///    `findFirst()` the winner is therefore the lowest **Z**, ties broken
    ///    by the lowest **Y**, then the lowest **X** — the opposite reading of
    ///    "X is fastest" from the one that comes to mind, and observable
    ///    whenever the player straddles two columns that disagree.
    ///
    /// A degenerate box scans nothing: `betweenClosed` computes
    /// `end = width * height * depth` from `max - min + 1` per axis, so an
    /// inverted range gives a count of **0** and the iteration is simply
    /// empty. That is the case a swimming pose reaches — the handler's two
    /// 0.4 Y insets cross on a 0.6-tall box — and vanilla does **not**
    /// normalise it. (After flooring the count can reach 0 but never goes
    /// negative, because the two insets differ by less than one block.)
    pub fn first_bubble_column_in(&self, aabb: [f64; 6], drag: &[Option<bool>]) -> Option<bool> {
        let (x0, y0, z0) = (
            aabb[0].floor() as i32,
            aabb[1].floor() as i32,
            aabb[2].floor() as i32,
        );
        let (x1, y1, z1) = (
            aabb[3].floor() as i32,
            aabb[4].floor() as i32,
            aabb[5].floor() as i32,
        );
        // `hasChunksAt` — the whole box, before any block is read.
        for cz in (z0 >> 4)..=(z1 >> 4) {
            for cx in (x0 >> 4)..=(x1 >> 4) {
                if !self.columns.contains_key(&(cx, cz)) {
                    return None;
                }
            }
        }
        // Vanilla's index arithmetic, in vanilla's order.
        for z in z0..=z1 {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let state = self.block_state_at(x, y, z) as usize;
                    if let Some(d) = drag.get(state).copied().flatten() {
                        return Some(d);
                    }
                }
            }
        }
        None
    }

    pub fn tick_skull_animations(&mut self, powered: &std::collections::HashSet<u32>) {
        if powered.is_empty() {
            return;
        }
        for pos in self.block_entities.positions() {
            let on = powered.contains(&self.block_state_at(pos.x, pos.y, pos.z));
            self.block_entities.tick_skull(pos, on);
        }
    }

    pub fn column_mut(&mut self, cx: i32, cz: i32) -> Option<&mut chunk::Column> {
        self.columns
            .get_mut(&(cx, cz))
            .map(std::sync::Arc::make_mut)
    }

    pub fn column(&self, cx: i32, cz: i32) -> Option<&chunk::Column> {
        self.columns.get(&(cx, cz)).map(|c| c.as_ref())
    }

    /// Every loaded column's coordinate, as an **owned** vec.
    ///
    /// Owned rather than an iterator on purpose: the caller that needs all of
    /// them at once is the dimension transition in `rewo_net::play`, which
    /// queues them for the renderer to free and then *replaces the whole
    /// `World`* — a borrow of `self.columns` could not outlive that.
    pub fn column_coords(&self) -> Vec<(i32, i32)> {
        self.columns.keys().copied().collect()
    }

    /// A self-contained snapshot of the 3×3 column neighborhood around
    /// (cx, cz) — 9 `Arc` clones, no data copied. Hand this to a mesh worker:
    /// face culling reads ±1 block and AO reads diagonal corners at ±1, so
    /// nothing a column mesh needs lives outside its 3×3. Reads past the
    /// snapshot edge behave exactly like today's unloaded-column edge
    /// (air / full-bright).
    pub fn snapshot_3x3(&self, cx: i32, cz: i32) -> World {
        let mut columns = HashMap::with_capacity(9);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let key = (cx + dx, cz + dz);
                if let Some(col) = self.columns.get(&key) {
                    columns.insert(key, Arc::clone(col));
                }
            }
        }
        World {
            shape: self.shape,
            columns,
            entities: entities::EntityTable::default(),
            biome: self.biome.clone(),
            // A mesh worker must see the *same* lighting contract as the world
            // it was snapshotted from, or a Nether column would mesh with
            // Overworld cardinal shade.
            has_sky_light: self.has_sky_light,
            cardinal_light_type: self.cardinal_light_type,
            cardinal_light: self.cardinal_light,
            // A mesh worker meshes *blocks*; block entities are a render-pass
            // concern and are never read from a snapshot, so carrying them
            // would only cost the clone.
            block_entities: block_entities::BlockEntities::default(),
            // Same reason for both (M81): a crack overlay is a decal drawn
            // over the finished mesh and a pickup is a particle, so neither is
            // reachable from a mesh worker.
            destruction: destruction::DestructionProgress::default(),
            pickups: pickup::Pickups::default(),
        }
    }

    /// Raw quart→biome lookup (`NoiseBiomeSource.getNoiseBiome`): resolve the
    /// column from the quart, then read its section biome container. Unloaded
    /// columns fall back to biome index 0. Used by both the fiddle (block tint)
    /// and the Gaussian (camera sky/fog).
    pub fn noise_biome_at_quart(&self, qx: i32, qy: i32, qz: i32) -> u16 {
        let cx = qx >> 2;
        let cz = qz >> 2;
        match self.columns.get(&(cx, cz)) {
            Some(col) => col.noise_biome_at_quart(&self.shape, qx, qy, qz),
            None => 0,
        }
    }

    /// `ClientLevel.calculateBlockTint` for a resolver at world (x,y,z). `None`
    /// when no biome context is attached (synthetic world). `radius` is the
    /// `biomeBlendRadius` option (default 2).
    pub fn block_tint(
        &self,
        x: i32,
        y: i32,
        z: i32,
        resolver: biome::ColorResolver,
        radius: i32,
    ) -> Option<[u8; 3]> {
        let ctx = self.biome.as_ref()?;
        Some(ctx.block_tint(x, y, z, resolver, radius, &|qx, qy, qz| {
            self.noise_biome_at_quart(qx, qy, qz)
        }))
    }

    /// Camera `visual/sky_color` at the eye (opaque ARGB), or `None` with no
    /// biome context. The caller applies the day/night timeline multiply.
    pub fn camera_sky(&self, eye: [f64; 3]) -> Option<i32> {
        let ctx = self.biome.as_ref()?;
        Some(ctx.camera_sky(eye, &|qx, qy, qz| self.noise_biome_at_quart(qx, qy, qz)))
    }

    /// Camera `visual/fog_color` at the eye (opaque ARGB), or `None`.
    pub fn camera_fog(&self, eye: [f64; 3]) -> Option<i32> {
        let ctx = self.biome.as_ref()?;
        Some(ctx.camera_fog(eye, &|qx, qy, qz| self.noise_biome_at_quart(qx, qy, qz)))
    }

    /// Apply a `chunks_biomes` packet to a loaded column (replace its section
    /// biome palettes). No-op if the column isn't loaded.
    pub fn apply_chunks_biomes(&mut self, cx: i32, cz: i32, biomes: Vec<palette::Container>) {
        if let Some(col) = self.columns.get_mut(&(cx, cz)) {
            Arc::make_mut(col).set_biomes(biomes);
        }
    }

    /// Apply a single block change (Block Update packet).
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let cx = x >> 4;
        let cz = z >> 4;
        if let Some(col) = self.columns.get_mut(&(cx, cz)) {
            Arc::make_mut(col).set_block(&self.shape, x & 15, y, z & 15, state);
        }
    }

    /// Order-independent digest of all loaded block states — two clients
    /// (live vs replay) that saw the same packets must agree. Uses a
    /// commutative fold so column insertion order can't change the result.
    pub fn digest(&self) -> u64 {
        let mut acc: u64 = 0;
        for ((cx, cz), col) in &self.columns {
            let mut h = 1469598103934665603u64; // FNV offset
            fnv(&mut h, *cx as u64);
            fnv(&mut h, *cz as u64);
            h = col.digest(&self.shape, h);
            acc = acc.wrapping_add(h); // commutative across columns
        }
        acc
    }
}

pub(crate) fn fnv(h: &mut u64, v: u64) {
    for i in 0..8 {
        *h ^= (v >> (i * 8)) & 0xff;
        *h = h.wrapping_mul(1099511628211);
    }
}

#[cfg(test)]
mod bubble_column_scan_tests {
    use crate::dimension::DimensionShape;

    /// Two bubble-column states, mirroring `blocks.json`: 15294 `drag=true`
    /// (the block's DEFAULT) and 15295 `drag=false`.
    const DRAG_TRUE: u32 = 1;
    const DRAG_FALSE: u32 = 2;

    fn drag_table() -> Vec<Option<bool>> {
        let mut t = vec![None; 8];
        t[DRAG_TRUE as usize] = Some(true);
        t[DRAG_FALSE as usize] = Some(false);
        t
    }

    fn world_with(cols: &[(i32, i32)]) -> crate::World {
        let shape = DimensionShape::OVERWORLD;
        let mut w = crate::World::new(shape);
        for &(cx, cz) in cols {
            w.insert_column(cx, cz, crate::chunk::Column::empty_lit(&shape, cx, cz));
        }
        w
    }

    /// A column in the box is found, and its `drag` comes back — not the
    /// block's default.
    #[test]
    fn a_column_in_the_box_is_found_with_its_own_drag() {
        let t = drag_table();
        for (state, want) in [(DRAG_TRUE, true), (DRAG_FALSE, false)] {
            let mut w = world_with(&[(0, 0)]);
            w.set_block(2, 64, 3, state);
            let box_ = [1.5, 63.5, 2.5, 3.5, 65.5, 4.5];
            assert_eq!(w.first_bubble_column_in(box_, &t), Some(want));
        }
        // …and an empty box of air finds nothing.
        let w = world_with(&[(0, 0)]);
        assert_eq!(
            w.first_bubble_column_in([1.5, 63.5, 2.5, 3.5, 65.5, 4.5], &t),
            None
        );
    }

    /// **A missing chunk empties the whole scan** — not a partial read.
    /// `getBlockStatesIfLoaded` guards the entire box with `hasChunksAt`, so a
    /// column that IS loaded and DOES hold a bubble column is still not seen
    /// when a neighbouring chunk the box also covers is absent.
    #[test]
    fn a_missing_chunk_empties_the_whole_scan() {
        let t = drag_table();
        let mut w = world_with(&[(0, 0)]);
        w.set_block(15, 64, 8, DRAG_TRUE);
        // Wholly inside the loaded column: found.
        assert_eq!(
            w.first_bubble_column_in([15.2, 63.9, 8.2, 15.8, 64.9, 8.8], &t),
            Some(true)
        );
        // Straddling into chunk (1, 0), which is not loaded: nothing at all,
        // even though the column that holds the block is loaded.
        assert_eq!(
            w.first_bubble_column_in([15.2, 63.9, 8.2, 16.8, 64.9, 8.8], &t),
            None,
            "hasChunksAt guards the WHOLE box"
        );
        // Loading the neighbour makes it visible again — the re-arm the
        // handler depends on.
        let mut w2 = w;
        let shape = DimensionShape::OVERWORLD;
        w2.insert_column(1, 0, crate::chunk::Column::empty_lit(&shape, 1, 0));
        assert_eq!(
            w2.first_bubble_column_in([15.2, 63.9, 8.2, 16.8, 64.9, 8.8], &t),
            Some(true)
        );
    }

    /// **X varies fastest and Z slowest — so the PRIORITY is the reverse.**
    ///
    /// `findFirst()` over `BlockPos.betweenClosed`'s index arithmetic
    /// (`x = i % width`, `y = (i / width) % height`, `z = i / width / height`)
    /// visits every block of one Z slice before any block of the next, and
    /// every block of one Y row before the next row. So the winner is the
    /// lowest **Z**, ties broken by the lowest **Y**, then the lowest **X** —
    /// the opposite reading of "X is fastest" from the one that comes to mind.
    ///
    /// The first draft of this test asserted the other way round and failed on
    /// the Z case; its Y case *passed for the wrong reason*, because the block
    /// it expected to win on X happened also to be the lower one in Y.
    #[test]
    fn the_scan_order_is_z_major_then_y_then_x() {
        let t = drag_table();
        let box_ = [0.5, 64.5, 0.5, 2.5, 66.5, 2.5];

        // Same Z, same Y, differing X: the lower X wins.
        let mut w = world_with(&[(0, 0)]);
        w.set_block(2, 65, 1, DRAG_TRUE);
        w.set_block(1, 65, 1, DRAG_FALSE);
        assert_eq!(w.first_bubble_column_in(box_, &t), Some(false), "lower X");

        // Same Z, differing Y: the lower Y wins EVEN AT A HIGHER X, so Y
        // dominates X.
        let mut w = world_with(&[(0, 0)]);
        w.set_block(1, 66, 1, DRAG_TRUE); // lower X, higher Y
        w.set_block(2, 65, 1, DRAG_FALSE); // higher X, lower Y
        assert_eq!(
            w.first_bubble_column_in(box_, &t),
            Some(false),
            "Y outranks X — a whole Y row is visited before the next"
        );

        // Differing Z: the lower Z wins even at a higher Y and X, so Z
        // dominates both.
        let mut w = world_with(&[(0, 0)]);
        w.set_block(1, 65, 2, DRAG_TRUE); // lowest X and Y, higher Z
        w.set_block(2, 66, 1, DRAG_FALSE); // highest X and Y, lower Z
        assert_eq!(
            w.first_bubble_column_in(box_, &t),
            Some(false),
            "Z outranks everything — a whole Z slice is visited before the next"
        );
    }

    /// **Both bounds are floored**, so a box whose max lands exactly on an
    /// integer includes that block. This is what the handler's 1e-6 deflate
    /// exists to prevent, and pinning it is what makes that epsilon meaningful
    /// rather than decorative.
    #[test]
    fn both_bounds_floor_so_an_exact_max_is_included() {
        let t = drag_table();
        let mut w = world_with(&[(0, 0)]);
        w.set_block(3, 64, 0, DRAG_TRUE);
        // max X exactly 3.0 — floor(3.0) == 3, so block 3 IS scanned.
        assert_eq!(
            w.first_bubble_column_in([2.0, 64.0, 0.0, 3.0, 64.0, 0.0], &t),
            Some(true)
        );
        // A hair under, and it is not.
        assert_eq!(
            w.first_bubble_column_in([2.0, 64.0, 0.0, 3.0 - 1e-6, 64.0, 0.0], &t),
            None,
            "the deflate is what keeps the neighbouring block out"
        );
    }

    /// A degenerate box scans nothing rather than normalising. That is the
    /// case vanilla's two 0.4 insets reach on a 0.6-tall pose, and the count
    /// arithmetic (`max - min + 1` per axis) makes it a clean zero.
    #[test]
    fn an_inverted_range_scans_nothing() {
        let t = drag_table();
        let mut w = world_with(&[(0, 0)]);
        w.set_block(1, 64, 1, DRAG_TRUE);
        assert_eq!(
            w.first_bubble_column_in([1.0, 65.0, 1.0, 1.0, 64.0, 1.0], &t),
            None,
            "maxY below minY is empty, not normalised"
        );
        // The same box the right way up does find it.
        assert_eq!(
            w.first_bubble_column_in([1.0, 64.0, 1.0, 1.0, 65.0, 1.0], &t),
            Some(true)
        );
    }
}

#[cfg(test)]
mod ambient_sounds_at_tests {
    use crate::ambient::{AmbientMood, AmbientSounds};
    use crate::dimension::DimensionShape;
    use std::sync::Arc;

    fn biome(name: &str, ambient: Option<AmbientSounds>) -> crate::biome::BiomeDef {
        crate::biome::BiomeDef {
            name: name.into(),
            temperature: 0.8,
            downfall: 0.4,
            water_color: 0,
            grass_override: None,
            foliage_override: None,
            dry_foliage_override: None,
            grass_modifier: crate::biome::GrassModifier::None,
            sky_color: None,
            fog_color: None,
            has_precipitation: true,
            temperature_modifier: Default::default(),
            ambient_sounds: ambient,
            background_music: None,
        }
    }

    /// Biome 0 declares nothing (so it inherits), biome 1 declares a loop.
    fn world_with_two_biomes() -> crate::World {
        let shape = DimensionShape::OVERWORLD;
        let mut w = crate::World::new(shape);
        for cx in -1..=1 {
            for cz in -1..=1 {
                w.insert_column(cx, cz, crate::chunk::Column::empty_lit(&shape, cx, cz));
            }
        }
        let defs = vec![
            biome("test:plains", None),
            biome(
                "test:nether",
                Some(AmbientSounds {
                    loop_sound: Some("test:loop".into()),
                    mood: None,
                    additions: Vec::new(),
                }),
            ),
        ];
        w.set_biome_context(Arc::new(crate::biome::BiomeContext::new(
            Arc::new(crate::biome::BiomeRegistry::new(defs)),
            crate::biome::Colormaps::neutral(),
            0,
        )));
        w
    }

    /// The dimension's base is what an unopinionated biome hears — which is
    /// the whole reason an Overworld cave has cave sounds, since no vanilla
    /// Overworld biome sets the attribute at all.
    #[test]
    fn an_unopinionated_biome_inherits_the_dimension_base() {
        let w = world_with_two_biomes();
        let base = AmbientSounds::legacy_cave();
        let got = w.ambient_sounds_at([0.5, 64.5, 0.5], &base);
        assert_eq!(got, base, "biome 0 declares nothing, so the base stands");
        assert_eq!(
            got.mood.map(|m| m.sound),
            Some("minecraft:ambient.cave".to_string())
        );
    }

    /// **The sample follows the position.** Biome containers are per section,
    /// so this varies one by chunk and one by height — enough to show that
    /// both the horizontal quart and the **Y** quart reach the answer, which
    /// is why flying up out of a cave biome changes the loop.
    #[test]
    fn the_sample_follows_the_position() {
        let mut w = world_with_two_biomes();
        let base = AmbientSounds::legacy_cave();
        let sections = w.shape.section_count();

        // Chunk (0,0): biome 1 everywhere. Chunk (-1,0): left as biome 0.
        w.apply_chunks_biomes(
            0,
            0,
            (0..sections)
                .map(|_| crate::palette::Container::single(1))
                .collect(),
        );
        assert_eq!(
            w.ambient_sounds_at([4.5, 64.5, 4.5], &base)
                .loop_sound
                .as_deref(),
            Some("test:loop"),
            "the biome under the player answers"
        );
        // …and it REPLACED the base rather than merging with it.
        assert!(
            w.ambient_sounds_at([4.5, 64.5, 4.5], &base).mood.is_none(),
            "the biome's record replaces the base's"
        );
        // A chunk over is the other biome, so the answer moves with the player.
        assert_eq!(
            w.ambient_sounds_at([-4.5, 64.5, 4.5], &base),
            base,
            "a different column is a different answer"
        );

        // Now vary by HEIGHT: the lowest section becomes biome 0 again.
        let mut per_section: Vec<_> = (0..sections)
            .map(|_| crate::palette::Container::single(1))
            .collect();
        per_section[0] = crate::palette::Container::single(0);
        w.apply_chunks_biomes(0, 0, per_section);
        let low_y = w.shape.min_y as f64 + 0.5;
        assert_eq!(
            w.ambient_sounds_at([4.5, low_y, 4.5], &base),
            base,
            "the Y quart reaches the answer too"
        );
        assert_eq!(
            w.ambient_sounds_at([4.5, 64.5, 4.5], &base)
                .loop_sound
                .as_deref(),
            Some("test:loop"),
            "…while the section above still hears its own biome"
        );
    }

    /// With no biome context the base stands unlayered — the honest answer for
    /// a synthetic world, where the dimension still has its say and no biome
    /// can override what does not exist.
    #[test]
    fn no_biome_context_leaves_the_base_alone() {
        let w = crate::World::new(DimensionShape::OVERWORLD);
        let base = AmbientSounds {
            loop_sound: Some("test:base".into()),
            mood: Some(AmbientMood::legacy_cave()),
            additions: Vec::new(),
        };
        assert_eq!(w.ambient_sounds_at([1.0, 2.0, 3.0], &base), base);
    }
}
