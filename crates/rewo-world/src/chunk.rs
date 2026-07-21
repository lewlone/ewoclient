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
}

pub struct Column {
    pub cx: i32,
    pub cz: i32,
    sections: Vec<Section>,
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
        section.block_light(x, ly, z).max(section.sky_light(x, ly, z))
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

/// Decode a full Level Chunk With Light packet body (reader positioned right
/// after the packet id).
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
    read_light_into(r, &mut sections)?;

    Ok(Column { cx, cz, sections })
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

fn read_light_into(r: &mut PacketReader, sections: &mut [Section]) -> Result<()> {
    let sky_mask = read_bitset(r)?;
    let block_mask = read_bitset(r)?;
    let _empty_sky = read_bitset(r)?;
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
    Ok(())
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
