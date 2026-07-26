//! Block entities — the half of the world Rewo had never decoded (M25).
//!
//! A block entity is two things at once: an ordinary block model, and a
//! `BlockEntityRenderer`. Where the model has no `elements`, **the renderer is
//! the block**, and a client without one draws nothing at all. Measured against
//! the real 26.2 jar, 96 blocks bake to no geometry, and once air, water, lava,
//! light, barriers, `structure_void`, `bubble_column` and `moving_piston` are
//! set aside — those are *correctly* invisible — the remainder is ~78 blocks
//! that Rewo has been rendering as empty space since M2:
//!
//! ```text
//! chests (incl. copper + waxed)  10    banners (standing + wall)  32
//! shulker boxes                  17    heads / skulls             16
//! copper golem statues            9    decorated pot               1
//! conduit                         1    end portal / gateway        2
//! ```
//!
//! Beds and signs are deliberately absent from that list: their block models
//! *do* carry geometry, so a bed is a bed and a sign is a plank — only the
//! sign's text and the bed's extras live in the renderer.
//!
//! **What this module is, and is not.** It decodes and stores block entities;
//! it does not render them. The scoped registry below is the seam a renderer
//! plugs into, and it is **fail-closed**: every one of the 49 registered types
//! is either named as supported or explicitly listed as unsupported with a
//! reason, and a type in neither list is an error rather than a silent skip.
//! That is the same discipline the mob redo used, and for the same reason — a
//! partial set that fails *open* renders a subset while claiming coverage.
//!
//! Wire form. The level-chunk payload carries a packed list (26.2
//! `ClientboundLevelChunkPacketData`):
//!
//! ```text
//! VarInt count
//!   u8    packedXZ    // (x & 15) << 4 | (z & 15), relative to the chunk
//!   i16   y           // absolute
//!   VarInt type       // minecraft:block_entity_type protocol id
//!   NBT   data        // the network-trimmed tag; may be TAG_End
//! ```
//!
//! and `ClientboundBlockEntityDataPacket` carries one of the same, keyed by an
//! absolute `BlockPos`.

use std::collections::HashMap;

use rewo_proto::nbt::Nbt;

/// One block entity's decoded identity and payload.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockEntity {
    /// `minecraft:block_entity_type` protocol id, exactly as sent. Resolved to
    /// a [`BlockEntityKind`] through the registry rather than assumed, because
    /// these ids renumber between versions.
    pub type_id: i32,
    /// The network tag. Vanilla trims this to what the renderer needs
    /// (`getUpdateTag`), so it is *not* the full save form — a chest sends no
    /// inventory, for instance.
    pub data: Nbt,
}

/// Absolute block position, as the key a block entity is stored under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockEntityPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockEntityPos {
    /// Unpack the chunk payload's `(packedXZ, y)` pair against a chunk origin.
    ///
    /// `packedXZ` is `(x & 15) << 4 | (z & 15)` — **x in the high nibble**,
    /// which is the opposite of the `(y << 4 | z) << 4 | x` ordering the
    /// section-biome index uses, and worth not transposing by muscle memory.
    pub fn from_packed(cx: i32, cz: i32, packed_xz: u8, y: i16) -> Self {
        BlockEntityPos {
            x: cx * 16 + ((packed_xz >> 4) & 15) as i32,
            y: y as i32,
            z: cz * 16 + (packed_xz & 15) as i32,
        }
    }
}

/// Whether Rewo's renderer knows what to do with a block-entity type.
///
/// The point of the `Unsupported` arm carrying a reason is that it is written
/// down once, next to the supported ones, instead of being the absence of a
/// match arm somewhere else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockEntityKind {
    /// The block's own model carries its geometry; the renderer adds
    /// something Rewo does not model (sign text, a bell's swing, a spawner's
    /// spinning mob). **The block is not invisible** — skipping the renderer
    /// loses detail, not the block.
    ModelIsEnough,
    /// The block model is empty, so this type renders as nothing until a
    /// renderer exists. These are the real gap.
    Invisible,
    /// Renderer implemented.
    Rendered,
}

/// Every registered `minecraft:block_entity_type`, classified.
///
/// This table is the fail-closed boundary: [`BlockEntityRegistry::resolve`]
/// rejects a registry containing a name absent from it, so a version that adds
/// a block-entity type stops the client instead of silently dropping it.
///
/// The classification is *measured*, not guessed — see the module docs for how
/// the `Invisible` set was derived from the real jar's models.
pub const TYPE_TABLE: &[(&str, BlockEntityKind)] = &[
    // -- the invisible ones: no `elements` in any of their blockstate models --
    ("minecraft:chest", BlockEntityKind::Invisible),
    ("minecraft:trapped_chest", BlockEntityKind::Invisible),
    ("minecraft:ender_chest", BlockEntityKind::Invisible),
    ("minecraft:shulker_box", BlockEntityKind::Invisible),
    ("minecraft:banner", BlockEntityKind::Invisible),
    ("minecraft:skull", BlockEntityKind::Invisible),
    ("minecraft:decorated_pot", BlockEntityKind::Invisible),
    ("minecraft:conduit", BlockEntityKind::Invisible),
    ("minecraft:end_portal", BlockEntityKind::Invisible),
    ("minecraft:end_gateway", BlockEntityKind::Invisible),
    ("minecraft:copper_golem_statue", BlockEntityKind::Invisible),
    // -- the block model already draws these; only extra detail is missing --
    ("minecraft:sign", BlockEntityKind::ModelIsEnough),
    ("minecraft:hanging_sign", BlockEntityKind::ModelIsEnough),
    ("minecraft:bell", BlockEntityKind::ModelIsEnough),
    ("minecraft:lectern", BlockEntityKind::ModelIsEnough),
    ("minecraft:enchanting_table", BlockEntityKind::ModelIsEnough),
    ("minecraft:mob_spawner", BlockEntityKind::ModelIsEnough),
    ("minecraft:trial_spawner", BlockEntityKind::ModelIsEnough),
    ("minecraft:vault", BlockEntityKind::ModelIsEnough),
    ("minecraft:beacon", BlockEntityKind::ModelIsEnough),
    ("minecraft:campfire", BlockEntityKind::ModelIsEnough),
    ("minecraft:brushable_block", BlockEntityKind::ModelIsEnough),
    ("minecraft:piston", BlockEntityKind::ModelIsEnough),
    ("minecraft:structure_block", BlockEntityKind::ModelIsEnough),
    ("minecraft:jigsaw", BlockEntityKind::ModelIsEnough),
    ("minecraft:shelf", BlockEntityKind::ModelIsEnough),
    ("minecraft:chiseled_bookshelf", BlockEntityKind::ModelIsEnough),
    ("minecraft:creaking_heart", BlockEntityKind::ModelIsEnough),
    // -- the rest have no renderer at all in vanilla: pure block models with
    //    an inventory or a tick behind them.
    ("minecraft:furnace", BlockEntityKind::ModelIsEnough),
    ("minecraft:jukebox", BlockEntityKind::ModelIsEnough),
    ("minecraft:dispenser", BlockEntityKind::ModelIsEnough),
    ("minecraft:dropper", BlockEntityKind::ModelIsEnough),
    ("minecraft:brewing_stand", BlockEntityKind::ModelIsEnough),
    ("minecraft:daylight_detector", BlockEntityKind::ModelIsEnough),
    ("minecraft:hopper", BlockEntityKind::ModelIsEnough),
    ("minecraft:comparator", BlockEntityKind::ModelIsEnough),
    ("minecraft:command_block", BlockEntityKind::ModelIsEnough),
    ("minecraft:barrel", BlockEntityKind::ModelIsEnough),
    ("minecraft:smoker", BlockEntityKind::ModelIsEnough),
    ("minecraft:blast_furnace", BlockEntityKind::ModelIsEnough),
    ("minecraft:beehive", BlockEntityKind::ModelIsEnough),
    ("minecraft:sculk_sensor", BlockEntityKind::ModelIsEnough),
    ("minecraft:calibrated_sculk_sensor", BlockEntityKind::ModelIsEnough),
    ("minecraft:sculk_catalyst", BlockEntityKind::ModelIsEnough),
    ("minecraft:sculk_shrieker", BlockEntityKind::ModelIsEnough),
    ("minecraft:crafter", BlockEntityKind::ModelIsEnough),
    ("minecraft:test_block", BlockEntityKind::ModelIsEnough),
    // A 26.2 addition with an ordinary block model and no renderer under
    // `client/renderer/blockentity/` — caught by `resolve`'s unclassified
    // check on the first run, which is the whole point of that check.
    ("minecraft:potent_sulfur", BlockEntityKind::ModelIsEnough),
    ("minecraft:test_instance_block", BlockEntityKind::ModelIsEnough),
];

/// Type-id → classification, resolved once against the live registry.
#[derive(Debug)]
pub struct BlockEntityRegistry {
    by_id: HashMap<i32, BlockEntityKind>,
    /// Names, for diagnostics — an unknown id logs what it was next to.
    names: HashMap<i32, String>,
}

impl BlockEntityRegistry {
    /// Resolve [`TYPE_TABLE`] against `minecraft:block_entity_type`'s entries.
    ///
    /// Fails loud in **both** directions, which is what makes this a boundary
    /// rather than a lookup: a table entry the registry does not contain means
    /// the pin drifted, and a registry entry the table does not classify means
    /// a new block-entity type shipped and nobody decided what it looks like.
    pub fn resolve(entries: &[(String, i32)]) -> Result<Self, String> {
        let mut by_id = HashMap::with_capacity(entries.len());
        let mut names = HashMap::with_capacity(entries.len());
        let table: HashMap<&str, BlockEntityKind> = TYPE_TABLE.iter().copied().collect();
        if table.len() != TYPE_TABLE.len() {
            return Err("block_entities: TYPE_TABLE has a duplicate name".into());
        }
        for (name, id) in entries {
            let kind = table.get(name.as_str()).copied().ok_or_else(|| {
                format!(
                    "block_entities: {name:?} is registered but not classified — add it to \
                     TYPE_TABLE (Invisible if its block model has no elements, ModelIsEnough \
                     if the block draws itself)"
                )
            })?;
            by_id.insert(*id, kind);
            names.insert(*id, name.clone());
        }
        for (name, _) in TYPE_TABLE {
            if !entries.iter().any(|(n, _)| n == name) {
                return Err(format!(
                    "block_entities: TYPE_TABLE classifies {name:?} but the registry has no \
                     such type — the pinned version drifted"
                ));
            }
        }
        let invisible = by_id
            .values()
            .filter(|k| **k == BlockEntityKind::Invisible)
            .count();
        log::info!(
            "rewo-world: {} block-entity types classified, {invisible} still invisible",
            by_id.len()
        );
        Ok(Self { by_id, names })
    }

    /// The classification of a type id, or `None` for an id outside the
    /// registry (a modded or corrupt payload).
    pub fn kind(&self, type_id: i32) -> Option<BlockEntityKind> {
        self.by_id.get(&type_id).copied()
    }

    pub fn name(&self, type_id: i32) -> Option<&str> {
        self.names.get(&type_id).map(|s| s.as_str())
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// How many registered types still render as nothing — the size of the gap
    /// this milestone measured.
    pub fn invisible_count(&self) -> usize {
        self.by_id
            .values()
            .filter(|k| **k == BlockEntityKind::Invisible)
            .count()
    }
}

/// One face of a sign's text, from its block-entity NBT (M25e).
///
/// `SignText.DIRECT_CODEC` stores `messages` as four text components plus a
/// `color` and `has_glowing_text`. The components are flattened to plain text
/// here: Rewo's font pass has one colour per draw, so a per-run style would
/// have nowhere to go, and flattening is what `Component.getString()` does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignFace {
    /// The four lines, in order. A missing or short list pads with empties,
    /// because `AbstractSignRenderer` always draws exactly four.
    pub lines: [String; 4],
    /// `color` — a dye name, or `None` for the default black.
    pub color: Option<String>,
    /// `has_glowing_text`. Not rendered (glow is an outline pass Rewo does not
    /// have), but decoded so the exclusion is visible rather than silent.
    pub glowing: bool,
}

impl SignFace {
    /// Read one `front_text` / `back_text` compound.
    pub fn from_nbt(tag: &Nbt) -> Option<SignFace> {
        let msgs = tag.get("messages")?;
        let Nbt::List(items) = msgs else {
            return None;
        };
        let mut lines: [String; 4] = Default::default();
        for (i, slot) in lines.iter_mut().enumerate() {
            if let Some(m) = items.get(i) {
                // Each message is a text component — sometimes a bare JSON
                // string, sometimes a compound. `to_plain_text` handles both,
                // and a plain string that *looks* like JSON is left alone
                // because the server already resolved it to a component.
                *slot = m.to_plain_text();
            }
        }
        Some(SignFace {
            lines,
            color: tag.get("color").and_then(Nbt::as_str).map(str::to_string),
            glowing: tag
                .get("has_glowing_text")
                .and_then(Nbt::as_i64)
                .is_some_and(|v| v != 0),
        })
    }

    /// Whether this face has anything to draw.
    pub fn is_blank(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }
}

impl BlockEntity {
    /// The sign text on this block entity, if it carries any.
    ///
    /// Returns `(front, back)`; either may be blank. A block entity that is
    /// not a sign simply has neither key.
    pub fn sign_text(&self) -> (Option<SignFace>, Option<SignFace>) {
        (
            self.data.get("front_text").and_then(SignFace::from_nbt),
            self.data.get("back_text").and_then(SignFace::from_nbt),
        )
    }
}

/// `ChestLidController` — the client-side lid clock, verbatim.
///
/// ```text
/// tickLid():  oOpenness = openness;
///             if (!shouldBeOpen && openness > 0) openness = max(openness - 0.1, 0);
///             else if (shouldBeOpen && openness < 1) openness = min(openness + 0.1, 1);
/// getOpenness(a) = Mth.lerp(a, oOpenness, openness)
/// ```
///
/// Like every other clock this client keeps, the *openness* is not
/// transmitted: the server sends one `block_event` saying how many players
/// have the chest open, and the client animates the ten ticks itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChestLid {
    /// `shouldBeOpen`, set by `triggerEvent(1, viewerCount)`.
    pub should_be_open: bool,
    /// `openness` — the current value, 0..1.
    pub openness: f32,
    /// `oOpenness` — the previous tick's, for the render lerp.
    pub old_openness: f32,
}

impl ChestLid {
    /// `ChestLidController.tickLid`.
    pub fn tick(&mut self) {
        self.old_openness = self.openness;
        if !self.should_be_open && self.openness > 0.0 {
            self.openness = (self.openness - 0.1).max(0.0);
        } else if self.should_be_open && self.openness < 1.0 {
            self.openness = (self.openness + 0.1).min(1.0);
        }
    }

    /// `ChestLidController.getOpenness(partialTicks)`.
    pub fn openness(self, alpha: f32) -> f32 {
        self.old_openness + alpha * (self.openness - self.old_openness)
    }

    /// Whether this lid is doing anything — a fully closed, not-opening lid
    /// needs no entry at all.
    pub fn is_idle_closed(self) -> bool {
        !self.should_be_open && self.openness == 0.0 && self.old_openness == 0.0
    }
}

/// The block entities of one world, keyed by absolute position.
///
/// A flat map rather than per-column storage: block entities are sparse (a
/// village chunk holds a handful), and the render pass wants them by area, not
/// by column. Column unload removes them explicitly — see
/// [`Self::remove_column`].
#[derive(Default)]
pub struct BlockEntities {
    map: HashMap<BlockEntityPos, BlockEntity>,
    /// Lid clocks, for the positions that have one. Absent is
    /// [`ChestLid::default`] — closed and staying closed — so a chest nobody
    /// has opened costs nothing.
    lids: HashMap<BlockEntityPos, ChestLid>,
}

impl BlockEntities {
    pub fn insert(&mut self, pos: BlockEntityPos, be: BlockEntity) {
        self.map.insert(pos, be);
    }

    pub fn get(&self, pos: BlockEntityPos) -> Option<&BlockEntity> {
        self.map.get(&pos)
    }

    pub fn remove(&mut self, pos: BlockEntityPos) -> Option<BlockEntity> {
        self.map.remove(&pos)
    }

    /// Drop every block entity in a chunk column.
    ///
    /// Load-bearing on the *replacement* path too, not just unload: a re-sent
    /// level-chunk packet is the authoritative list for that column, so the old
    /// contents must go first or a block entity the player broke would linger.
    pub fn remove_column(&mut self, cx: i32, cz: i32) {
        self.map.retain(|p, _| p.x >> 4 != cx || p.z >> 4 != cz);
        self.lids.retain(|p, _| p.x >> 4 != cx || p.z >> 4 != cz);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&BlockEntityPos, &BlockEntity)> {
        self.map.iter()
    }

    /// Every block entity, in a deterministic order. For gates and dumps —
    /// `HashMap` iteration order is not reproducible across runs.
    pub fn sorted(&self) -> Vec<(BlockEntityPos, &BlockEntity)> {
        let mut v: Vec<_> = self.map.iter().map(|(p, b)| (*p, b)).collect();
        v.sort_by_key(|(p, _)| *p);
        v
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.lids.clear();
    }

    /// `ChestBlockEntity.triggerEvent(b0, b1)`:
    ///
    /// ```text
    /// if (b0 == 1) { this.chestLidController.shouldBeOpen(b1 > 0); return true; }
    /// ```
    ///
    /// `b1` is the *viewer count*, not a boolean — the server sends how many
    /// players have the container open, and any non-zero count means open.
    ///
    /// Returns whether the event was consumed. A `b0` other than 1 is some
    /// other block's event (a note block's pitch, a piston's direction) and is
    /// left alone rather than guessed at.
    pub fn trigger_lid_event(&mut self, pos: BlockEntityPos, b0: u8, b1: u8) -> bool {
        if b0 != 1 {
            return false;
        }
        // Vanilla routes the event through the *block entity* at the position,
        // so a position with none ignores it — the same existence rule
        // `set_block_entity_data` keeps.
        if self.map.get(&pos).is_none() {
            return false;
        }
        self.lids.entry(pos).or_default().should_be_open = b1 > 0;
        true
    }

    /// Advance every lid one tick, dropping the ones that have settled shut.
    pub fn tick_lids(&mut self) {
        self.lids.retain(|_, l| {
            l.tick();
            !l.is_idle_closed()
        });
    }

    /// The lid state at a position — closed if it has never been opened.
    pub fn lid(&self, pos: BlockEntityPos) -> ChestLid {
        self.lids.get(&pos).copied().unwrap_or_default()
    }

    pub fn open_lid_count(&self) -> usize {
        self.lids.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_xz_puts_x_in_the_high_nibble() {
        // `(x & 15) << 4 | (z & 15)`. Transposing this is the obvious mistake:
        // the section-biome index packs the other way round.
        let p = BlockEntityPos::from_packed(0, 0, 0x3A, 64);
        assert_eq!((p.x, p.y, p.z), (3, 64, 10));
        // ...and it is relative to the chunk origin, with y absolute.
        let p = BlockEntityPos::from_packed(-2, 5, 0x0F, -13);
        assert_eq!((p.x, p.y, p.z), (-32, -13, 95));
    }

    #[test]
    fn negative_y_survives_the_i16_round_trip() {
        // The deepest overworld block entity sits at y = -64.
        let p = BlockEntityPos::from_packed(0, 0, 0, -64);
        assert_eq!(p.y, -64);
    }

    #[test]
    fn removing_a_column_leaves_its_neighbours_alone() {
        let mut be = BlockEntities::default();
        let mk = |x, y, z| {
            (
                BlockEntityPos { x, y, z },
                BlockEntity {
                    type_id: 1,
                    data: Nbt::End,
                },
            )
        };
        for (p, b) in [mk(0, 64, 0), mk(15, 64, 15), mk(16, 64, 0), mk(-1, 64, 0)] {
            be.insert(p, b);
        }
        assert_eq!(be.len(), 4);
        be.remove_column(0, 0);
        // (0,0) and (15,15) are chunk 0,0; (16,0) is chunk 1,0; (-1,0) is
        // chunk -1,0 — the arithmetic shift is what makes the negative work.
        assert_eq!(be.len(), 2);
        assert!(be.get(BlockEntityPos { x: 16, y: 64, z: 0 }).is_some());
        assert!(be.get(BlockEntityPos { x: -1, y: 64, z: 0 }).is_some());
    }

    #[test]
    fn the_registry_rejects_an_unclassified_type() {
        let entries = vec![
            ("minecraft:chest".to_string(), 1),
            ("minecraft:brand_new_thing".to_string(), 2),
        ];
        let err = BlockEntityRegistry::resolve(&entries).unwrap_err();
        assert!(err.contains("brand_new_thing"), "{err}");
    }

    #[test]
    fn the_registry_rejects_a_table_entry_the_registry_lost() {
        let entries = vec![("minecraft:chest".to_string(), 1)];
        let err = BlockEntityRegistry::resolve(&entries).unwrap_err();
        assert!(err.contains("drifted"), "{err}");
    }
}
