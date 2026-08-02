//! rewo-world — authoritative world state decoded from the wire.
//!
//! M1 scope: paletted chunk sections, columns keyed by dimension height,
//! block-light/sky-light arrays, an entity table, a `block_state_at` query,
//! and a deterministic world digest (for replay-equivalence in the DoD).
//! Prediction/physics land in M3; this is the read model.

pub mod abilities;
pub mod attributes;
pub mod biome;
pub mod biome_noise;
pub mod block_entities;
pub mod border;
pub mod cape;
pub mod celestial;
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
