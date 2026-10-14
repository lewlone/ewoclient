//! Chunk column + section decode from the Level Chunk With Light packet.
//!
//! Packet body (decompiled `ClientboundLevelChunkWithLightPacket`):
//! ```text
//! i32 x, i32 z
//! chunk data:
//!   heightmaps: map<VarInt Types, LONG_ARRAY>   (VarInt count, entries)
//!   VarInt size, u8[size] sections_blob
//!   block entities: VarInt count, [u8 packedXZ, i16 y, VarInt type, NBT]
//! light data:
//!   4 × BitSet (VarInt long-count + longs)
//!   sky updates:   VarInt count, [VarInt len(=2048), u8[2048]]
//!   block updates: same
//! ```
//! Each section in the blob: `i16 non_empty_block_count`, block-state
//! container, biome container. Light nibble arrays are indexed per section.

use rewo_data::blocks::Blocks;
use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

use crate::block_entities::{BlockEntity, BlockEntityPos};
use crate::dimension::DimensionShape;
use crate::palette::{Container, ContainerKind};

/// One 16³ section.
///
/// The non-`pub` fields are `pub(crate)` **only** so [`crate::chunk_cache`] can
/// destructure them exhaustively — see the note on [`Container`].
#[derive(Clone)]
pub struct Section {
    pub non_empty: i16,
    pub(crate) states: Container,
    /// 4×4×4 biome cells (`PalettedContainer` biome strategy). Cell index is
    /// `(y<<2 | z)<<2 | x` with x/y/z ∈ 0..3 (`QuartPos` local). The palette
    /// values are global biome-registry indices.
    pub(crate) biomes: Container,
    /// 2048-byte nibble arrays (4096 cells), if present for this section.
    pub(crate) block_light: Option<Vec<u8>>,
    pub(crate) sky_light: Option<Vec<u8>>,
    /// A block update wrote into this section after decode. The server's
    /// `non_empty` count is not maintained through edits (it would need the
    /// air-state set), so an edited section is never treated as trivial.
    pub(crate) edited: bool,
}

impl Section {
    pub fn block_state(&self, x: i32, y: i32, z: i32) -> u32 {
        // Cell index order: (y << 8) | (z << 4) | x  (matches server Strategy).
        let idx = ((y as usize) << 8) | ((z as usize) << 4) | x as usize;
        self.states.get(idx)
    }

    fn nibble(arr: &Option<Vec<u8>>, x: i32, y: i32, z: i32) -> Option<u8> {
        let arr = arr.as_ref()?;
        let idx = ((y as usize) << 8) | ((z as usize) << 4) | x as usize;
        let byte = *arr.get(idx / 2)?;
        Some(if idx & 1 == 0 { byte & 0x0f } else { byte >> 4 })
    }

    pub fn block_light(&self, x: i32, y: i32, z: i32) -> u8 {
        Self::nibble(&self.block_light, x, y, z).unwrap_or(0)
    }

    pub fn sky_light(&self, x: i32, y: i32, z: i32) -> u8 {
        Self::nibble(&self.sky_light, x, y, z).unwrap_or(0)
    }

    /// Write one nibble, allocating the layer on first write. A section the
    /// server sent no data for is all-zero, which is the correct starting
    /// point for both channels (the sky pass seeds its own sources).
    fn set_nibble(arr: &mut Option<Vec<u8>>, x: i32, y: i32, z: i32, level: u8) {
        let arr = arr.get_or_insert_with(|| vec![0u8; 2048]);
        let idx = ((y as usize) << 8) | ((z as usize) << 4) | x as usize;
        let Some(byte) = arr.get_mut(idx / 2) else {
            return;
        };
        let level = level & 0x0f;
        *byte = if idx & 1 == 0 {
            (*byte & 0xf0) | level
        } else {
            (*byte & 0x0f) | (level << 4)
        };
    }

    pub fn set_block_light(&mut self, x: i32, y: i32, z: i32, level: u8) {
        Self::set_nibble(&mut self.block_light, x, y, z, level);
    }

    pub fn set_sky_light(&mut self, x: i32, y: i32, z: i32, level: u8) {
        Self::set_nibble(&mut self.sky_light, x, y, z, level);
    }

    /// Biome registry index at a **section-local** quart (`qx`,`qy`,`qz` ∈ 0..3).
    /// Cell index is `(y<<2 | z)<<2 | x` (`PalettedContainer` biome strategy).
    pub fn biome_at(&self, qx: i32, qy: i32, qz: i32) -> u16 {
        let idx = (((qy & 3) << 2 | (qz & 3)) << 2 | (qx & 3)) as usize;
        self.biomes.get(idx) as u16
    }

    /// Replace this section's biome container (the `chunks_biomes` packet).
    pub fn set_biomes(&mut self, biomes: Container) {
        self.biomes = biomes;
    }
}

#[derive(Clone)]
pub struct Column {
    pub cx: i32,
    pub cz: i32,
    pub(crate) sections: Vec<Section>,
    /// Section index at/above which a *missing* sky array means full-bright.
    ///
    /// The server sends sky arrays only for sections near terrain: bits set in
    /// `sky_mask` carry data, bits in `empty_sky_mask` are explicitly zero, and
    /// sections in **neither** mask are simply "everything above the terrain".
    /// Vanilla's `SkyLightSectionStorage` reads those as 15, so a client that
    /// defaults them to 0 renders the whole sky-lit volume above the terrain as
    /// pitch black. Set from the masks in `read_light_into`.
    ///
    /// Vanilla's equivalent is the column's `topSections` entry (one above
    /// the highest stored sky layer, `SkyLightSectionStorage.onNodeAdded`).
    /// After decode every section **below** it holds an array — gaps are
    /// materialised the way vanilla reads them (`fill_sky_gaps`) — so a
    /// missing array means 15 and nothing else. A `light_update` only names
    /// the sections it changed, so it can raise this but never lower it.
    pub(crate) sky_full_above: usize,
    /// The `MOTION_BLOCKING` heightmap, as sent — one absolute Y per (x, z),
    /// where the value is the first **free** cell above the terrain
    /// (`Heightmap.getFirstAvailable`, not the topmost solid block).
    ///
    /// M33 needs it: weather columns run from the terrain height upward, so
    /// without it rain falls through the ground. Decoded here because that is
    /// where vanilla's client gets it — `Heightmap.setRawData` off the chunk
    /// packet.
    ///
    /// **Deviation, recorded rather than hidden**: vanilla's
    /// `LevelChunk.setBlockState` keeps the heightmaps current as blocks
    /// change, and Rewo does not. A column stays at its as-sent height until
    /// the chunk is resent, so rain over a freshly-dug hole falls from the old
    /// surface. Nothing else reads this, so the error is confined to weather.
    pub motion_blocking: Option<Box<[i32; 256]>>,
    /// The column's block entities, as sent (M25).
    ///
    /// Carried on the `Column` rather than inserted straight into the world's
    /// map because a level-chunk packet is the **authoritative** list for its
    /// column: the caller drops the column's previous entries before taking
    /// these, or a block entity the player broke would linger.
    pub block_entities: Vec<(BlockEntityPos, BlockEntity)>,
}

impl Column {
    pub fn block_state_at(&self, shape: &DimensionShape, lx: i32, y: i32, lz: i32) -> u32 {
        let Some(si) = shape.section_index(y) else {
            return 0;
        };
        let Some(section) = self.sections.get(si) else {
            return 0;
        };
        section.block_state(lx, y & 15, lz)
    }

    /// Apply a block edit (Block Update packets), written straight into the
    /// section's paletted storage.
    pub fn set_block(&mut self, shape: &DimensionShape, lx: i32, y: i32, lz: i32, state: u32) {
        if let Some(si) = shape.section_index(y) {
            if let Some(section) = self.sections.get_mut(si) {
                let idx = ((y & 15) as usize) << 8 | (lz as usize) << 4 | lx as usize;
                section.states.set(idx, state, 4096);
                section.edited = true;
            }
        }
    }

    /// Combined light 0..15 (max of block + sky) at section-local x/z,
    /// world y. Above the world reads full-bright; below reads dark.
    pub fn brightness_at(&self, shape: &DimensionShape, lx: i32, y: i32, lz: i32) -> u8 {
        let Some(si) = shape.section_index(y) else {
            return if y >= shape.min_y + shape.height { 15 } else { 0 };
        };
        let Some(section) = self.sections.get(si) else {
            return 15;
        };
        let (x, ly, z) = (lx, y & 15, lz);
        section
            .block_light(x, ly, z)
            .max(self.sky_light_in(si, section, x, ly, z))
    }

    /// Separate (block, sky) light at section-local x/z — the F3 readout and
    /// anything that needs the two sources apart, unlike `brightness_at`
    /// which is their max. Above the build height sky is full; below the
    /// world both are 0.
    /// Write one channel's light at a column-local `(lx, y, lz)`.
    /// Returns whether the stored value actually changed, so the light engine
    /// can track exactly which columns need remeshing.
    pub fn set_light(
        &mut self,
        shape: &DimensionShape,
        ch: crate::light::Channel,
        lx: i32,
        y: i32,
        lz: i32,
        level: u8,
    ) -> bool {
        let Some(si) = shape.section_index(y) else {
            return false;
        };
        let ly = y.rem_euclid(16);
        let implicit_sky = Self::implicit_sky(self.sky_full_above, si);
        let Some(section) = self.sections.get_mut(si) else {
            return false;
        };
        // What a section with no array reads as: block light is 0, sky light
        // follows the `sky_full_above` rule. The comparison must use the value
        // a reader sees (`light_at`), not the raw nibble: in an omitted sky
        // section above the terrain the raw read is 0 while the cell reads 15,
        // so a darkening `set(.., 0)` there used to be skipped as "unchanged".
        let (arr, implicit) = match ch {
            crate::light::Channel::Block => (&mut section.block_light, 0),
            crate::light::Channel::Sky => (&mut section.sky_light, implicit_sky),
        };
        let before = Section::nibble(arr, lx, ly, lz).unwrap_or(implicit);
        if before == level {
            return false;
        }
        if arr.is_none() {
            // Materialise the layer holding what every cell already read, so
            // the other 4095 cells keep their value. Vanilla does the same
            // when a section gains storage (`SkyLightSectionStorage.
            // createDataLayer`, :93-112: `new DataLayer(15)` above the top).
            *arr = Some(vec![implicit * 0x11; 2048]);
        }
        Section::set_nibble(arr, lx, ly, lz, level);
        true
    }

    pub fn light_at(&self, shape: &DimensionShape, lx: i32, y: i32, lz: i32) -> (u8, u8) {
        let Some(si) = shape.section_index(y) else {
            return if y >= shape.min_y + shape.height { (0, 15) } else { (0, 0) };
        };
        let Some(section) = self.sections.get(si) else {
            return (0, 0);
        };
        let (x, ly, z) = (lx, y & 15, lz);
        (
            section.block_light(x, ly, z),
            self.sky_light_in(si, section, x, ly, z),
        )
    }

    /// Raw quart→biome lookup within this column (`LevelChunk.getNoiseBiome`).
    /// `qx`/`qz` are **world** quart coords whose chunk is this column; `qy` is
    /// a world quart, clamped into the build range like vanilla. Returns the
    /// global biome-registry index, or 0 above/below a sensible fallback.
    pub fn noise_biome_at_quart(&self, shape: &DimensionShape, qx: i32, qy: i32, qz: i32) -> u16 {
        let min_quart = shape.min_y >> 2; // QuartPos.fromBlock(min_y)
        let max_quart = min_quart + (shape.height >> 2) - 1;
        let clamped_y = qy.clamp(min_quart, max_quart);
        let Some(si) = shape.section_index(clamped_y << 2) else {
            return 0;
        };
        let Some(section) = self.sections.get(si) else {
            return 0;
        };
        section.biome_at(qx, clamped_y, qz)
    }

    /// Replace this column's per-section biome containers (`chunks_biomes`
    /// packet). `new_biomes` is one container per section, bottom-to-top; extra
    /// or missing entries are ignored so a partial packet can't panic.
    pub fn set_biomes(&mut self, new_biomes: Vec<crate::palette::Container>) {
        for (section, biomes) in self.sections.iter_mut().zip(new_biomes) {
            section.set_biomes(biomes);
        }
    }

    /// Sky light, applying the "no array above the terrain means 15" rule.
    fn sky_light_in(&self, si: usize, section: &Section, x: i32, y: i32, z: i32) -> u8 {
        Section::nibble(&section.sky_light, x, y, z)
            .unwrap_or_else(|| Self::implicit_sky(self.sky_full_above, si))
    }

    /// The sky level of a cell in section `si` when that section has no
    /// array. The single source of the rule, shared by the reader and by
    /// [`Self::set_light`]'s materialisation, so the two cannot drift.
    fn implicit_sky(sky_full_above: usize, si: usize) -> u8 {
        if si >= sky_full_above {
            15
        } else {
            0
        }
    }

    /// True when the section at index has no visible content — lets the
    /// mesher skip air. Overrides count as content.
    pub fn section_is_trivial(&self, idx: usize) -> bool {
        self.sections
            .get(idx)
            .map(|s| s.non_empty == 0 && !s.edited)
            .unwrap_or(true)
    }

    /// World-y range [min, max) of sections with content, for cull AABBs.
    pub fn content_y_range(&self, shape: &DimensionShape) -> Option<(i32, i32)> {
        let mut lo = None;
        let mut hi = None;
        for (i, _) in self.sections.iter().enumerate() {
            if !self.section_is_trivial(i) {
                let base = shape.min_y + (i as i32) * 16;
                if lo.is_none() {
                    lo = Some(base);
                }
                hi = Some(base + 16);
            }
        }
        Some((lo?, hi?))
    }

    pub fn digest(&self, shape: &DimensionShape, mut h: u64) -> u64 {
        for (si, section) in self.sections.iter().enumerate() {
            if section.states.is_uniform_zero() {
                continue;
            }
            crate::fnv(&mut h, si as u64);
            // Sample every state — deterministic, exhaustive, order-fixed.
            for idx in 0..4096u32 {
                let x = (idx & 15) as i32;
                let z = ((idx >> 4) & 15) as i32;
                let y = (idx >> 8) as i32;
                let state = section.block_state(x, y, z);
                if state != 0 {
                    crate::fnv(&mut h, ((idx as u64) << 20) | state as u64);
                }
            }
        }
        let _ = shape;
        h
    }
}

impl Section {
    /// An all-air section at full sky light — the building block for
    /// synthetic worlds (the `demo` showcase). `set_block` overrides then
    /// populate it; the full skylight makes the scene render lit.
    pub fn empty_lit() -> Self {
        Section {
            non_empty: 0,
            states: Container::single(0),
            // Synthetic scenes carry no biome data — a single-value container of
            // registry index 0. With no biome context attached this is inert.
            biomes: Container::single(0),
            block_light: None,
            sky_light: Some(vec![0xFF; 2048]), // both nibbles = 15
            edited: false,
        }
    }
}

impl Column {
    /// An all-air, fully-lit column for synthetic scenes. Blocks are added
    /// via `Column::set_block` (through `World::set_block`).
    pub fn empty_lit(shape: &DimensionShape, cx: i32, cz: i32) -> Self {
        let sections = (0..shape.section_count())
            .map(|_| Section::empty_lit())
            .collect();
        Column {
            cx,
            cz,
            sections,
            // Synthetic columns carry explicit full-bright sky arrays, so the
            // "missing means 15" rule never applies.
            sky_full_above: usize::MAX,
            motion_blocking: None,
            block_entities: Vec::new(),
        }
    }
}

/// `Heightmap.Types.MOTION_BLOCKING`'s wire id — the enum's explicit `id`
/// field, which `ByteBufCodecs.idMapper(BY_ID, t -> t.id)` writes, not its
/// ordinal.
pub const HEIGHTMAP_MOTION_BLOCKING: i32 = 4;

/// Unpack one `SimpleBitStorage` heightmap: 256 entries of
/// `ceillog2(height + 1)` bits, packed without straddling a long boundary, each
/// storing `y - minY`.
///
/// Returns `None` when the array is not the length that bit width implies. A
/// wrong-sized heightmap is better ignored than read as garbage heights: the
/// only consumer is weather, which falls back to the camera's own band.
fn unpack_heightmap(data: &[i64], shape: &DimensionShape) -> Option<Box<[i32; 256]>> {
    let bits = (crate::biome::ceil_log2((shape.height + 1) as usize) as usize).max(1);
    let per_long = 64 / bits;
    let expected = 256usize.div_ceil(per_long);
    if data.len() != expected {
        return None;
    }
    let mask = (1u64 << bits) - 1;
    let mut out = Box::new([0i32; 256]);
    for (i, slot) in out.iter_mut().enumerate() {
        let word = data[i / per_long] as u64;
        let shift = (i % per_long) * bits;
        // `getFirstAvailable` adds `minY` back on read.
        *slot = ((word >> shift) & mask) as i32 + shape.min_y;
    }
    Some(out)
}

/// Decode a full Level Chunk With Light packet body (reader positioned right
/// after the packet id).
/// Apply a `light_update` packet to an existing column.
///
/// The server sends these whenever lighting changes without the chunk being
/// resent — a torch placed, a block mined into a cave, sky light re-flooding.
/// Without it the client's light is only ever correct as of chunk load, so a
/// freshly lit cave stays pitch black. The payload after the chunk coords is
/// byte-for-byte the same structure `level_chunk_with_light` carries, so the
/// same reader handles it.
pub fn apply_light_update(r: &mut PacketReader, col: &mut Column) -> Result<()> {
    // The masks name only the sections whose light changed
    // (`ClientboundLightUpdatePacketData`'s changed-section filters), so the
    // packet's topmost bit is not the column's top: the boundary may rise
    // (a roof built above the terrain) but a packet that names only low
    // sections must not pull it down.
    col.sky_full_above = read_light_into(r, &mut col.sections, Some(col.sky_full_above))?;
    Ok(())
}

pub fn read_level_chunk(
    r: &mut PacketReader,
    shape: &DimensionShape,
    blocks: &Blocks,
) -> Result<Column> {
    // 7 is a safe biome upper bound for 26.2: `Mth.ceillog2(66)` = 7 for the
    // full vanilla biome registry, so a *direct* biome palette never needs
    // more. Callers holding the real biome registry width pass it to
    // `read_level_chunk_bits2`.
    read_level_chunk_bits2(r, shape, blocks.global_palette_bits, 7)
}

/// Same decode, taking the block-state global-palette width directly (for
/// callers that don't hold a `Blocks` table — the play session stores the
/// number). Uses a safe biome-bits upper bound.
pub fn read_level_chunk_bits(
    r: &mut PacketReader,
    shape: &DimensionShape,
    global_bits: u32,
) -> Result<Column> {
    read_level_chunk_bits2(r, shape, global_bits, 7)
}

/// Decode with both the block-state and biome global-palette widths. `biome_bits`
/// is the biome registry's `ceil_log2(count)` (`BiomeRegistry::global_bits`);
/// it only matters for a *direct* biome container (>3 distinct biomes in a
/// section's 64 cells), which single-biome flat worlds never produce.
pub fn read_level_chunk_bits2(
    r: &mut PacketReader,
    shape: &DimensionShape,
    global_bits: u32,
    biome_bits: u32,
) -> Result<Column> {
    let cx = r.i32()?;
    let cz = r.i32()?;

    // Heightmaps: VarInt count, then [VarInt type_id, long_array].
    let hm_count = r.count("heightmaps", 1)?;
    let mut motion_blocking = None;
    for _ in 0..hm_count {
        let type_id = r.varint()?;
        let data = r.long_array()?;
        if type_id == HEIGHTMAP_MOTION_BLOCKING {
            motion_blocking = unpack_heightmap(&data, shape);
        }
    }

    // Sections blob (length-delimited): parse sections out of a sub-reader.
    let size = r.varint()?;
    if size < 0 {
        return Err(rewo_proto::ProtoError::Frame("negative chunk size".into()));
    }
    let blob = r.take(size as usize)?;
    let mut sr = PacketReader::new(blob);
    let section_count = shape.section_count();
    let mut sections = Vec::with_capacity(section_count);
    for _ in 0..section_count {
        // Two shorts: non-empty block count, then fluid count (decompiled
        // LevelChunkSection.read — getSerializedSize's leading `4` = both).
        let non_empty = sr.i16()?;
        let _fluid_count = sr.i16()?;
        let states = Container::read(&mut sr, ContainerKind::BlockStates { global_bits })?;
        // Biomes container follows — a 4×4×4 quart grid. Retained for biome
        // tint (M14); the palette maps to global biome-registry indices.
        let biomes = Container::read(
            &mut sr,
            ContainerKind::Biomes {
                global_bits: biome_bits,
            },
        )?;
        sections.push(Section {
            non_empty,
            states,
            biomes,
            block_light: None,
            sky_light: None,
            edited: false,
        });
    }

    // Block entities: VarInt count, [u8 packedXZ, i16 y, VarInt type, NBT].
    //
    // Walked for alignment since M1, and **retained** since M25 — before that
    // every field was read and dropped, which is why a chest had never been
    // anything but empty space (see `crate::block_entities`).
    let be_count = r.count("block entities", 1)?;
    let mut block_entities = Vec::with_capacity(be_count.min(256));
    for _ in 0..be_count {
        let packed_xz = r.u8()?;
        let y = r.i16()?;
        let type_id = r.varint()?;
        let data = r.nbt()?;
        block_entities.push((
            BlockEntityPos::from_packed(cx, cz, packed_xz, y),
            BlockEntity { type_id, data },
        ));
    }

    // Light data: 4 BitSets + 2 update lists. Distribute nibble arrays to
    // sections using the sky/block Y masks. The masks cover
    // `section_count + 2` bits (one below, one above the buildable range).
    let sky_full_above = read_light_into(r, &mut sections, None)?;

    Ok(Column {
        cx,
        cz,
        sections,
        sky_full_above,
        motion_blocking,
        block_entities,
    })
}

/// Parse the `chunks_biomes` per-chunk buffer: one biome `Container` per
/// section, bottom-to-top, with no framing between them (decompiled
/// `ClientboundChunksBiomesPacket` writes `section.getBiomes().write(buffer)`
/// for every section back-to-back).
pub fn read_chunk_biomes(
    r: &mut PacketReader,
    shape: &DimensionShape,
    biome_bits: u32,
) -> Result<Vec<Container>> {
    let n = shape.section_count();
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(Container::read(
            r,
            ContainerKind::Biomes {
                global_bits: biome_bits,
            },
        )?);
    }
    Ok(out)
}

fn read_bitset(r: &mut PacketReader) -> Result<Vec<u64>> {
    let n = r.count("bitset", 8)?;
    let mut words = Vec::with_capacity(n);
    for _ in 0..n {
        words.push(r.u64()?);
    }
    Ok(words)
}

fn bitset_get(words: &[u64], i: usize) -> bool {
    words.get(i / 64).map(|w| (w >> (i % 64)) & 1 == 1).unwrap_or(false)
}

/// Decode the four light bitsets + the nibble arrays into `sections`.
///
/// Returns the section index at/above which a missing sky array means
/// full-bright — see `Column::sky_full_above`. `prev_top` is the column's
/// current boundary for a `light_update` (`None` for a full chunk, whose
/// masks describe every stored section).
fn read_light_into(
    r: &mut PacketReader,
    sections: &mut [Section],
    prev_top: Option<usize>,
) -> Result<usize> {
    let sky_mask = read_bitset(r)?;
    let block_mask = read_bitset(r)?;
    let empty_sky = read_bitset(r)?;
    let empty_block = read_bitset(r)?;

    // Sky updates, then block updates: VarInt count, each [VarInt 2048, bytes].
    let read_arrays = |r: &mut PacketReader| -> Result<Vec<Vec<u8>>> {
        let count = r.count("light arrays", 2048)?;
        let mut arrays = Vec::with_capacity(count);
        for _ in 0..count {
            let arr = r.byte_array(2048)?;
            arrays.push(arr.to_vec());
        }
        Ok(arrays)
    };
    let sky_arrays = read_arrays(r)?;
    let block_arrays = read_arrays(r)?;

    // Mask bit `i` corresponds to section index `i - 1` (bit 0 = the section
    // below the world). Arrays appear in mask-bit order.
    distribute(&sky_mask, &empty_sky, sky_arrays, sections, true);
    distribute(&block_mask, &empty_block, block_arrays, sections, false);

    // Everything above the topmost section the server said anything about is
    // open sky. Bit `i` is section `i - 1`, so the highest mentioned section
    // is `top_bit - 1` and the first implicit-full one is `top_bit`.
    let top_bit = (0..sky_mask.len().max(empty_sky.len()) * 64)
        .filter(|b| bitset_get(&sky_mask, *b) || bitset_get(&empty_sky, *b))
        .max()
        .unwrap_or(0);
    let top = prev_top.map_or(top_bit, |prev| prev.max(top_bit));
    fill_sky_gaps(sections, top);
    Ok(top)
}

/// Give every sky-less section below `top` the layer vanilla reads there.
///
/// `SkyLightSectionStorage.getLightValue` (decompile :26-54): below the
/// column's top, a section with no stored layer reads the **bottom row** of
/// the nearest stored layer above it (`BlockPos.getFlatIndex` drops the
/// in-section y), or 15 if it reaches the top first. `createDataLayer`
/// (:93-127) materialises exactly that with `repeatFirstLayer` when such a
/// section gains storage. Doing it here keeps one rule for an absent array
/// (`Column::implicit_sky`: 15) instead of a reader that has to scan, and is
/// what lets the light engine materialise a layer without changing what any
/// *other* section reads.
///
/// Such gaps are omitted air runs under a stored section higher up: a
/// floating build, or a `light_update` that raised the top. A full chunk of
/// ordinary terrain has none.
fn fill_sky_gaps(sections: &mut [Section], top: usize) {
    let end = top.min(sections.len());
    // Top-down, so a gap under a gap copies the row the upper one copied.
    for si in (0..end).rev() {
        if sections[si].sky_light.is_some() {
            continue;
        }
        let first_row = sections
            .get(si + 1)
            .filter(|_| si + 1 < end)
            .and_then(|above| above.sky_light.as_ref())
            .and_then(|above| above.get(..128));
        sections[si].sky_light = Some(match first_row {
            Some(row) => row.repeat(16),
            None => vec![0xFF; 2048],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::World;
    use rewo_proto::writer::PacketWriter;

    /// Pack `values` the SimpleBitStorage way (mirrors palette.rs test helper).
    fn pack(values: &[u32], bits: u32) -> Vec<u8> {
        let vpl = (64 / bits) as usize;
        let mut out = Vec::new();
        for chunk in values.chunks(vpl) {
            let mut word: u64 = 0;
            for (i, &v) in chunk.iter().enumerate() {
                word |= (v as u64) << (i as u32 * bits);
            }
            out.extend_from_slice(&word.to_be_bytes());
        }
        out
    }

    /// Biome cell index `(y<<2 | z)<<2 | x` for section-local quart coords.
    fn bidx(qx: usize, qy: usize, qz: usize) -> usize {
        (qy << 2 | qz) << 2 | qx
    }

    /// A biome container buffer holding `cells` (64 entries) as a direct palette
    /// at `bits` wide (bits > 3 → the direct/global strategy).
    fn direct_biome_buf(cells: &[u32; 64], bits: u32) -> Vec<u8> {
        let mut w = PacketWriter::default();
        w.u8(bits as u8);
        w.raw(&pack(cells, bits));
        w.buf
    }

    fn shape_1section() -> DimensionShape {
        DimensionShape { min_y: 0, height: 16 }
    }

    /// x/y/z axes are read independently and in the exact vanilla order.
    #[test]
    fn biome_container_xyz_orientation() {
        let mut cells = [0u32; 64];
        cells[bidx(1, 0, 0)] = 11; // +x
        cells[bidx(0, 0, 1)] = 22; // +z
        cells[bidx(0, 1, 0)] = 33; // +y
        cells[bidx(3, 3, 3)] = 44; // far corner
        let buf = direct_biome_buf(&cells, 15);
        let mut r = PacketReader::new(&buf);
        let containers = read_chunk_biomes(&mut r, &shape_1section(), 15).unwrap();
        assert_eq!(containers.len(), 1);

        let shape = shape_1section();
        let mut world = World::new(shape);
        world.ensure_column(0, 0);
        world.apply_chunks_biomes(0, 0, containers);

        // World quart (qx,qy,qz) in chunk 0 → the same section-local cell.
        assert_eq!(world.noise_biome_at_quart(1, 0, 0), 11);
        assert_eq!(world.noise_biome_at_quart(0, 0, 1), 22);
        assert_eq!(world.noise_biome_at_quart(0, 1, 0), 33);
        assert_eq!(world.noise_biome_at_quart(3, 3, 3), 44);
        assert_eq!(world.noise_biome_at_quart(0, 0, 0), 0);
    }

    /// The real 26.2 direct biome width is 7 bits (`ceil_log2(66)`). Encode a
    /// 7-bit direct palette carrying id 65 (the highest that a 7-bit registry
    /// index can hold before 66's ceil pushes to 7) and read it back — a
    /// fake bits=15 fixture would hide a width-derivation bug.
    #[test]
    fn biome_direct_palette_7_bit_id_65() {
        let mut cells = [0u32; 64];
        cells[bidx(2, 1, 3)] = 65; // a high biome id, still ≤ 2^7 - 1
        cells[bidx(0, 0, 0)] = 1;
        let buf = direct_biome_buf(&cells, 7);
        let mut r = PacketReader::new(&buf);
        // Read with the registry-derived width (7), not a padded fixture.
        let containers = read_chunk_biomes(&mut r, &shape_1section(), 7).unwrap();
        assert!(r.is_empty(), "7-bit direct storage consumed exactly");
        let mut world = World::new(shape_1section());
        world.ensure_column(0, 0);
        world.apply_chunks_biomes(0, 0, containers);
        assert_eq!(world.noise_biome_at_quart(2, 1, 3), 65);
        assert_eq!(world.noise_biome_at_quart(0, 0, 0), 1);
    }

    /// Negative world quarts resolve to the correct column + local quart.
    #[test]
    fn biome_negative_world_quart() {
        // Chunk -1 covers world quarts -4..-1; local x = qx & 3.
        let mut cells = [0u32; 64];
        cells[bidx(3, 0, 3)] = 77; // local (3,0,3) = world quart (-1,0,-1)
        let buf = direct_biome_buf(&cells, 15);
        let mut r = PacketReader::new(&buf);
        let containers = read_chunk_biomes(&mut r, &shape_1section(), 15).unwrap();

        let mut world = World::new(shape_1section());
        world.ensure_column(-1, -1);
        world.apply_chunks_biomes(-1, -1, containers);
        assert_eq!(world.noise_biome_at_quart(-1, 0, -1), 77);
        // A quart in an unloaded column falls back to 0.
        assert_eq!(world.noise_biome_at_quart(50, 0, 50), 0);
    }

    /// The build-range Y clamp: a quart above the top reads the top section's
    /// quart, below the bottom reads the bottom (vanilla `getNoiseBiome` clamp).
    #[test]
    fn biome_vertical_clamp() {
        let mut cells = [0u32; 64];
        cells[bidx(0, 3, 0)] = 99; // top quart of the (only) section
        cells[bidx(0, 0, 0)] = 5; // bottom quart
        let buf = direct_biome_buf(&cells, 15);
        let mut r = PacketReader::new(&buf);
        let containers = read_chunk_biomes(&mut r, &shape_1section(), 15).unwrap();
        let mut world = World::new(shape_1section());
        world.ensure_column(0, 0);
        world.apply_chunks_biomes(0, 0, containers);
        // max_quart = 3; qy=100 clamps to 3.
        assert_eq!(world.noise_biome_at_quart(0, 100, 0), 99);
        // min_quart = 0; qy=-100 clamps to 0.
        assert_eq!(world.noise_biome_at_quart(0, -100, 0), 5);
    }

    /// Single-value + indirect biome palettes decode (chunks_biomes replacement).
    #[test]
    fn biome_single_and_indirect_palette() {
        // Single-value container: bits=0, one varint = registry id 7.
        let mut w = PacketWriter::default();
        w.u8(0).varint(7);
        let mut r = PacketReader::new(&w.buf);
        let c = read_chunk_biomes(&mut r, &shape_1section(), 7).unwrap();
        let mut world = World::new(shape_1section());
        world.ensure_column(0, 0);
        world.apply_chunks_biomes(0, 0, c);
        assert_eq!(world.noise_biome_at_quart(2, 1, 3), 7, "single fills the grid");

        // Indirect (bits=1) palette [3, 9]; cell 0 → index 1 (=9), rest 0 (=3).
        let mut indices = [0u32; 64];
        indices[bidx(1, 0, 0)] = 1;
        let mut w2 = PacketWriter::default();
        w2.u8(1).varint(2).varint(3).varint(9);
        w2.raw(&pack(&indices, 1));
        let mut r2 = PacketReader::new(&w2.buf);
        let c2 = read_chunk_biomes(&mut r2, &shape_1section(), 7).unwrap();
        world.apply_chunks_biomes(0, 0, c2); // replacement
        assert_eq!(world.noise_biome_at_quart(1, 0, 0), 9); // palette[1]
        assert_eq!(world.noise_biome_at_quart(0, 0, 0), 3); // palette[0]
    }
}

/// Install one channel's layers: `ClientPacketListener.readSectionList`
/// (decompile :2537-2560). A section in `mask` takes the next array; a
/// section in `empty` takes `new DataLayer()` — an explicit all-zero layer
/// that **replaces** whatever the section held (a removed torch's glow, a
/// roofed-over sky). Sections in neither are left alone.
///
/// The zero sky layer is stored as a real array rather than dropped: an
/// absent sky array reads 15 (see `Column::implicit_sky`), and vanilla's
/// zero layer is *stored*, so it also stops `fill_sky_gaps` looking through
/// it. An absent block array already reads 0, so that channel just drops it.
fn distribute(mask: &[u64], empty: &[u64], arrays: Vec<Vec<u8>>, sections: &mut [Section], sky: bool) {
    let mut arrays = arrays.into_iter();
    let total_bits = mask.len().max(empty.len()) * 64;
    for bit in 0..total_bits {
        let have_data = bitset_get(mask, bit);
        if !have_data && !bitset_get(empty, bit) {
            continue;
        }
        let layer = if have_data {
            let Some(arr) = arrays.next() else {
                break;
            };
            Some(arr)
        } else {
            None
        };
        // bit 0 is below the world; section index = bit - 1.
        if bit == 0 {
            continue;
        }
        if let Some(section) = sections.get_mut(bit - 1) {
            if sky {
                section.sky_light = Some(layer.unwrap_or_else(|| vec![0u8; 2048]));
            } else {
                section.block_light = layer;
            }
        }
    }
}

#[cfg(test)]
mod heightmap_tests {
    use super::*;

    /// Pack 256 heights the way `SimpleBitStorage` does, so the reader is
    /// graded against an independent writer rather than against itself.
    fn pack(heights: &[i32; 256], shape: &DimensionShape) -> Vec<i64> {
        let bits = (crate::biome::ceil_log2((shape.height + 1) as usize) as usize).max(1);
        let per_long = 64 / bits;
        let mut out = vec![0i64; 256usize.div_ceil(per_long)];
        for (i, h) in heights.iter().enumerate() {
            let v = (h - shape.min_y) as u64;
            out[i / per_long] |= ((v << ((i % per_long) * bits)) as i64) as i64;
        }
        out
    }

    #[test]
    fn the_overworld_heightmap_round_trips_through_nine_bit_storage() {
        let shape = DimensionShape::OVERWORLD;
        // 384 + 1 needs 9 bits, 7 per long, 37 longs -- not a whole number of
        // entries per long, which is what makes the packing worth testing.
        assert_eq!(crate::biome::ceil_log2((shape.height + 1) as usize), 9);
        let mut heights = [0i32; 256];
        for (i, h) in heights.iter_mut().enumerate() {
            *h = shape.min_y + (i as i32 * 7) % (shape.height - 1);
        }
        let packed = pack(&heights, &shape);
        assert_eq!(packed.len(), 37);
        let got = unpack_heightmap(&packed, &shape).expect("well-formed");
        assert_eq!(&got[..], &heights[..]);
    }

    /// A heightmap whose length does not match the bit width is ignored rather
    /// than read as garbage -- weather would otherwise fall from nonsense
    /// heights across the whole column set.
    #[test]
    fn a_wrong_sized_heightmap_is_rejected() {
        let shape = DimensionShape::OVERWORLD;
        assert!(unpack_heightmap(&[0i64; 36], &shape).is_none());
        assert!(unpack_heightmap(&[0i64; 38], &shape).is_none());
        assert!(unpack_heightmap(&[], &shape).is_none());
    }

    /// The Nether is 0..256, so its bit width differs -- a reader that assumed
    /// the Overworld's 9 bits would mis-read every Nether column.
    #[test]
    fn the_bit_width_follows_the_dimension_shape() {
        let nether = DimensionShape::NETHER;
        let bits = crate::biome::ceil_log2((nether.height + 1) as usize);
        assert_eq!(bits, 9, "257 still needs 9 bits");
        let heights = [nether.min_y + 5; 256];
        let packed = pack(&heights, &nether);
        let got = unpack_heightmap(&packed, &nether).expect("well-formed");
        assert!(got.iter().all(|h| *h == nether.min_y + 5));
    }
}

#[cfg(test)]
mod light_mask_tests {
    use super::*;
    use crate::World;
    use rewo_proto::writer::PacketWriter;

    /// A `BitSet` on the wire: VarInt long count, then the longs.
    fn bitset(w: &mut PacketWriter, bits: &[usize]) {
        let mut words = vec![0u64; bits.iter().map(|b| b / 64 + 1).max().unwrap_or(0)];
        for b in bits {
            words[b / 64] |= 1 << (b % 64);
        }
        w.varint(words.len() as i32);
        for word in words {
            w.i64(word as i64);
        }
    }

    /// The light half of a chunk / `light_update` packet, written the way
    /// `ClientboundLightUpdatePacketData.write` lays it out. Each data entry
    /// is `(mask bit, 2048-byte layer)`, in ascending bit order.
    fn payload(
        sky: &[(usize, Vec<u8>)],
        block: &[(usize, Vec<u8>)],
        empty_sky: &[usize],
        empty_block: &[usize],
    ) -> Vec<u8> {
        let mut w = PacketWriter::default();
        bitset(&mut w, &sky.iter().map(|(b, _)| *b).collect::<Vec<_>>());
        bitset(&mut w, &block.iter().map(|(b, _)| *b).collect::<Vec<_>>());
        bitset(&mut w, empty_sky);
        bitset(&mut w, empty_block);
        for list in [sky, block] {
            w.varint(list.len() as i32);
            for (_, layer) in list {
                w.varint(layer.len() as i32);
                w.raw(layer);
            }
        }
        w.buf
    }

    fn filled(level: u8) -> Vec<u8> {
        vec![level * 0x11; 2048]
    }

    fn update(col: &mut Column, bytes: &[u8]) {
        let mut r = PacketReader::new(bytes);
        apply_light_update(&mut r, col).unwrap();
        assert!(r.is_empty(), "the whole payload is consumed");
    }

    const SHAPE: DimensionShape = DimensionShape::OVERWORLD;

    /// `ClientPacketListener.readSectionList` (decompile :2547-2559) queues
    /// `new DataLayer()` for a section in the empty mask, replacing its old
    /// layer. Rewo used to discard the empty-block mask and use the empty-sky
    /// one only for the boundary, so a removed torch's glow (or a roofed-over
    /// sky) stayed in that section until the chunk was resent.
    #[test]
    fn an_empty_mask_zeroes_a_section_that_had_light() {
        let mut w = World::new(SHAPE);
        w.ensure_column(0, 0);
        let col = w.column_mut(0, 0).unwrap();
        // Sections 5 and 6 (bits 6 and 7) get light; y = 16 is section 5.
        update(
            col,
            &payload(&[(6, filled(9)), (7, filled(8))], &[(6, filled(14)), (7, filled(13))], &[], &[]),
        );
        assert_eq!(w.light_at(3, 16, 4), (14, 9));
        assert_eq!(w.light_at(3, 32, 4), (13, 8));

        // The torch is gone and the section is roofed: both empty bits for
        // section 5. Section 6 is named in neither mask and must keep its data.
        update(w.column_mut(0, 0).unwrap(), &payload(&[], &[], &[6], &[6]));
        for y in 16..32 {
            assert_eq!(w.light_at(3, y, 4), (0, 0), "y {y} is explicitly dark");
        }
        assert_eq!(w.light_at(3, 32, 4), (13, 8), "an unnamed section is untouched");
    }

    /// An explicit zero sky layer in a column whose top was lower is still
    /// dark: it is *stored*, unlike an absent array, which reads 15.
    #[test]
    fn an_empty_sky_section_above_the_old_top_reads_dark() {
        let mut w = World::new(SHAPE);
        w.ensure_column(0, 0);
        let col = w.column_mut(0, 0).unwrap();
        for s in &mut col.sections[8..] {
            s.sky_light = None;
        }
        col.sky_full_above = 8;
        assert_eq!(w.light_at(0, 80, 0).1, 15, "section 9 is omitted open sky");
        update(w.column_mut(0, 0).unwrap(), &payload(&[], &[], &[10], &[]));
        assert_eq!(w.column(0, 0).unwrap().sky_full_above, 10);
        assert_eq!(w.light_at(0, 80, 0).1, 0, "section 9 named empty is dark");
        // Section 8 sat between the old top and the new one with no array:
        // it reads the bottom row of section 9 above it, as vanilla's
        // `getLightValue` scan does — here that row is the explicit zero.
        assert_eq!(w.light_at(0, 70, 0).1, 0, "the gap below reads the stored row above");
        assert_eq!(w.light_at(0, 96, 0).1, 15, "above the new top is still open sky");
    }

    /// A `light_update` names only the sections whose light changed, so its
    /// highest bit is not the column's top. One that names a low section must
    /// leave the boundary where it was.
    #[test]
    fn a_light_update_naming_only_low_sections_keeps_the_sky_top() {
        let mut w = World::new(SHAPE);
        w.ensure_column(0, 0);
        let col = w.column_mut(0, 0).unwrap();
        for s in &mut col.sections[8..] {
            s.sky_light = None;
        }
        col.sky_full_above = 8;
        update(w.column_mut(0, 0).unwrap(), &payload(&[(3, filled(4))], &[], &[], &[]));
        assert_eq!(w.column(0, 0).unwrap().sky_full_above, 8);
        assert_eq!(w.light_at(0, -32, 0).1, 4, "section 2 took the data");
        assert_eq!(w.light_at(0, 80, 0).1, 15, "section 9 is still open sky");
    }

    /// A gap in a full chunk — omitted sections under a stored one, as over a
    /// floating build — reads the bottom row of the nearest stored layer
    /// above it (`SkyLightSectionStorage.getLightValue`, :31-49), not 0.
    #[test]
    fn a_sky_gap_reads_the_bottom_row_of_the_layer_above() {
        let mut sections: Vec<Section> = (0..SHAPE.section_count())
            .map(|_| Section {
                non_empty: 0,
                states: Container::single(0),
                biomes: Container::single(0),
                block_light: None,
                sky_light: None,
                edited: false,
            })
            .collect();
        // Section 6's bottom row varies by (x, z); its upper rows are 3.
        let mut top_layer = filled(3);
        for z in 0..16usize {
            for x in 0..16usize {
                let idx = (z << 4) | x; // y = 0
                let v = ((x + z) % 16) as u8;
                let b = &mut top_layer[idx / 2];
                *b = if idx & 1 == 0 { (*b & 0xF0) | v } else { (*b & 0x0F) | (v << 4) };
            }
        }
        let bytes = payload(&[(3, filled(0)), (7, top_layer)], &[], &[], &[]);
        let mut r = PacketReader::new(&bytes);
        let top = read_light_into(&mut r, &mut sections, None).unwrap();
        assert_eq!(top, 7);
        let col = Column {
            cx: 0,
            cz: 0,
            sections,
            sky_full_above: top,
            motion_blocking: None,
            block_entities: Vec::new(),
        };
        // Sections 3..=5 (y -16..32) are the gap; section 2 is dark data.
        for (x, z) in [(0, 0), (5, 7), (15, 15), (9, 2)] {
            for y in [-16, 0, 31] {
                assert_eq!(col.light_at(&SHAPE, x, y, z).1, ((x + z) % 16) as u8, "({x},{y},{z})");
            }
            assert_eq!(col.light_at(&SHAPE, x, 33, z).1, 3, "section 6 itself");
            assert_eq!(col.light_at(&SHAPE, x, 48, z).1, 15, "above the top");
            assert_eq!(col.light_at(&SHAPE, x, -20, z).1, 0, "section 2's zero data");
            assert_eq!(col.light_at(&SHAPE, x, -60, z).1, 0, "below the lowest stored layer");
        }
    }
}
