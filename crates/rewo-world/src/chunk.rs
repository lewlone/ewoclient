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

use crate::dimension::DimensionShape;
use crate::palette::{Container, ContainerKind};

#[derive(Clone)]
pub struct Section {
    pub non_empty: i16,
    states: Container,
    /// 2048-byte nibble arrays (4096 cells), if present for this section.
    block_light: Option<Vec<u8>>,
    sky_light: Option<Vec<u8>>,
    /// Post-decode block edits (Block Update packets) keyed by packed
    /// section-local index `(y<<8)|(z<<4)|x`. See `Column::set_block`.
    overrides: std::collections::HashMap<u16, u32>,
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
}

#[derive(Clone)]
pub struct Column {
    pub cx: i32,
    pub cz: i32,
    sections: Vec<Section>,
    /// Section index at/above which a *missing* sky array means full-bright.
    ///
    /// The server sends sky arrays only for sections near terrain: bits set in
    /// `sky_mask` carry data, bits in `empty_sky_mask` are explicitly zero, and
    /// sections in **neither** mask are simply "everything above the terrain".
    /// Vanilla's `SkyLightSectionStorage` reads those as 15, so a client that
    /// defaults them to 0 renders the whole sky-lit volume above the terrain as
    /// pitch black. Set from the masks in `read_light_into`.
    sky_full_above: usize,
}

impl Column {
    pub fn block_state_at(&self, shape: &DimensionShape, lx: i32, y: i32, lz: i32) -> u32 {
        let Some(si) = shape.section_index(y) else {
            return 0;
        };
        let Some(section) = self.sections.get(si) else {
            return 0;
        };
        // Consult post-decode edits (Block Update packets) first — the query
        // must reflect the live world, not just the chunk snapshot.
        section.block_state_with_overrides(lx, y & 15, lz)
    }

    pub fn set_block(&mut self, shape: &DimensionShape, lx: i32, y: i32, lz: i32, state: u32) {
        // M1: block edits are recorded via a small override map so a Block
        // Update repaints correctly without rebuilding the paletted storage.
        // (A full palette-aware writer lands with the mesher's remesh path.)
        if let Some(si) = shape.section_index(y) {
            if let Some(section) = self.sections.get_mut(si) {
                section
                    .overrides
                    .insert(((y & 15) as u16) << 8 | (lz as u16) << 4 | lx as u16, state);
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
        let Some(section) = self.sections.get_mut(si) else {
            return false;
        };
        let before = match ch {
            crate::light::Channel::Block => section.block_light(lx, ly, lz),
            crate::light::Channel::Sky => section.sky_light(lx, ly, lz),
        };
        if before == level {
            return false;
        }
        match ch {
            crate::light::Channel::Block => section.set_block_light(lx, ly, lz, level),
            crate::light::Channel::Sky => section.set_sky_light(lx, ly, lz, level),
        }
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

    /// Sky light, applying the "no array above the terrain means 15" rule.
    fn sky_light_in(&self, si: usize, section: &Section, x: i32, y: i32, z: i32) -> u8 {
        match Section::nibble(&section.sky_light, x, y, z) {
            Some(v) => v,
            None if si >= self.sky_full_above => 15,
            None => 0,
        }
    }

    /// True when the section at index has no visible content — lets the
    /// mesher skip air. Overrides count as content.
    pub fn section_is_trivial(&self, idx: usize) -> bool {
        self.sections
            .get(idx)
            .map(|s| s.non_empty == 0 && s.overrides.is_empty())
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
            if section.states.is_uniform_zero() && section.overrides.is_empty() {
                continue;
            }
            crate::fnv(&mut h, si as u64);
            // Sample every state — deterministic, exhaustive, order-fixed.
            for idx in 0..4096u32 {
                let x = (idx & 15) as i32;
                let z = ((idx >> 4) & 15) as i32;
                let y = (idx >> 8) as i32;
                let state = section.block_state_with_overrides(x, y, z);
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
    fn block_state_with_overrides(&self, x: i32, y: i32, z: i32) -> u32 {
        let key = ((y as u16) << 8) | ((z as u16) << 4) | x as u16;
        if let Some(&s) = self.overrides.get(&key) {
            return s;
        }
        self.block_state(x, y, z)
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
            block_light: None,
            sky_light: Some(vec![0xFF; 2048]), // both nibbles = 15
            overrides: std::collections::HashMap::new(),
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
        }
    }
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
    // A light update re-states the masks, so the sky boundary moves with it
    // (building a roof pushes the topmost data section upward).
    col.sky_full_above = read_light_into(r, &mut col.sections)?;
    Ok(())
}

pub fn read_level_chunk(
    r: &mut PacketReader,
    shape: &DimensionShape,
    blocks: &Blocks,
) -> Result<Column> {
    read_level_chunk_bits(r, shape, blocks.global_palette_bits)
}

/// Same decode, taking the global-palette width directly (for callers that
/// don't hold a `Blocks` table — the play session stores the number).
pub fn read_level_chunk_bits(
    r: &mut PacketReader,
    shape: &DimensionShape,
    global_bits: u32,
) -> Result<Column> {
    let cx = r.i32()?;
    let cz = r.i32()?;

    // Heightmaps: VarInt count, then [VarInt type_id, long_array].
    let hm_count = r.count("heightmaps", 1)?;
    for _ in 0..hm_count {
        let _type_id = r.varint()?;
        let _data = r.long_array()?;
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
        // Biomes container follows; decode + discard for M1 (biome tint is
        // M4). global_bits is a safe upper bound — a section using a *direct*
        // biome palette (>8 distinct biomes in 64 cells) never occurs in the
        // flat-world test; registry-derived biome bits lands with M4 tint.
        let _biomes = Container::read(&mut sr, ContainerKind::Biomes { global_bits: 7 })?;
        sections.push(Section {
            non_empty,
            states,
            block_light: None,
            sky_light: None,
            overrides: std::collections::HashMap::new(),
        });
    }

    // Block entities: VarInt count, [u8 packedXZ, i16 y, VarInt type, NBT].
    let be_count = r.count("block entities", 1)?;
    for _ in 0..be_count {
        let _packed_xz = r.u8()?;
        let _y = r.i16()?;
        let _type = r.varint()?;
        let _nbt = r.nbt()?;
    }

    // Light data: 4 BitSets + 2 update lists. Distribute nibble arrays to
    // sections using the sky/block Y masks. The masks cover
    // `section_count + 2` bits (one below, one above the buildable range).
    let sky_full_above = read_light_into(r, &mut sections)?;

    Ok(Column {
        cx,
        cz,
        sections,
        sky_full_above,
    })
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
/// full-bright — see `Column::sky_full_above`.
fn read_light_into(r: &mut PacketReader, sections: &mut [Section]) -> Result<usize> {
    let sky_mask = read_bitset(r)?;
    let block_mask = read_bitset(r)?;
    let empty_sky = read_bitset(r)?;
    let _empty_block = read_bitset(r)?;

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
    distribute(&sky_mask, &sky_arrays, sections, true);
    distribute(&block_mask, &block_arrays, sections, false);

    // Everything above the topmost section the server said anything about is
    // open sky. Bit `i` is section `i - 1`, so the highest mentioned section
    // is `top_bit - 1` and the first implicit-full one is `top_bit`.
    let top_bit = (0..sky_mask.len().max(empty_sky.len()) * 64)
        .filter(|b| bitset_get(&sky_mask, *b) || bitset_get(&empty_sky, *b))
        .max()
        .unwrap_or(0);
    Ok(top_bit)
}

fn distribute(mask: &[u64], arrays: &[Vec<u8>], sections: &mut [Section], sky: bool) {
    let mut next = 0usize;
    let total_bits = mask.len() * 64;
    for bit in 0..total_bits {
        if !bitset_get(mask, bit) {
            continue;
        }
        let Some(arr) = arrays.get(next) else {
            break;
        };
        next += 1;
        // bit 0 is below the world; section index = bit - 1.
        if bit == 0 {
            continue;
        }
        if let Some(section) = sections.get_mut(bit - 1) {
            if sky {
                section.sky_light = Some(arr.clone());
            } else {
                section.block_light = Some(arr.clone());
            }
        }
    }
}
