//! Paletted container decode — the crux of chunk parsing.
//!
//! Wire format (confirmed against the decompiled 26.2 `PalettedContainer`
//! + `SimpleBitStorage`, REWO_PLAN.md §11):
//! ```text
//! u8  bits_per_entry
//! palette:
//!   bits == 0            → single value: one VarInt (the global id)
//!   states  1..=8        → list: VarInt count, then `count` VarInt ids
//!   biomes  1..=3        → list: same
//!   bits above the max   → no palette; storage holds global ids directly
//! data: FIXED-size i64 array (NO length prefix), holding `entry_count`
//!   indices at `bits` each, packed `floor(64/bits)` per long, no spanning.
//! ```
//!
//! Two subtleties that bite:
//! 1. The long array is *not* length-prefixed — its length is derived.
//! 2. The server never emits every raw `bits` value: for block states it
//!    snaps 1..=4 up to a 4-bit linear palette, so a byte of 1/2/3 shouldn't
//!    occur — but we key purely off the byte and derive storage width from
//!    it, so we're robust either way.

use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

/// Which container we're decoding — determines the linear/hashmap threshold
/// and how a "direct" (no-palette) storage is sized.
#[derive(Clone, Copy)]
pub enum ContainerKind {
    /// 16×16×16 block states.
    BlockStates { global_bits: u32 },
    /// 4×4×4 biome cells.
    Biomes { global_bits: u32 },
}

impl ContainerKind {
    fn entry_count(self) -> usize {
        match self {
            ContainerKind::BlockStates { .. } => 4096,
            ContainerKind::Biomes { .. } => 64,
        }
    }

    fn global_bits(self) -> u32 {
        match self {
            ContainerKind::BlockStates { global_bits } | ContainerKind::Biomes { global_bits } => {
                global_bits
            }
        }
    }

    /// Highest `bits` value still served from an indirect (list) palette.
    fn max_indirect_bits(self) -> u32 {
        match self {
            ContainerKind::BlockStates { .. } => 8,
            ContainerKind::Biomes { .. } => 3,
        }
    }
}

/// A decoded container: a global-id lookup indexed by cell position.
pub struct Container {
    /// `None` = single value fills the whole container.
    single: Option<u32>,
    /// Palette (indirect) or empty (direct — storage holds global ids).
    palette: Vec<u32>,
    /// Unpacked per-cell values: either palette indices (indirect) or global
    /// ids (direct/single). Length is `entry_count`.
    cells: Vec<u32>,
    direct: bool,
}

impl Container {
    /// A single-value container (whole section is one state) — for building
    /// synthetic sections without a wire packet.
    pub fn single(value: u32) -> Self {
        Self {
            single: Some(value),
            palette: Vec::new(),
            cells: Vec::new(),
            direct: false,
        }
    }

    pub fn read(r: &mut PacketReader, kind: ContainerKind) -> Result<Self> {
        let bits = r.u8()? as u32;
        let entry_count = kind.entry_count();

        // bits == 0: single-value palette, no data array.
        if bits == 0 {
            let value = r.varint()? as u32;
            return Ok(Self {
                single: Some(value),
                palette: Vec::new(),
                cells: Vec::new(),
                direct: false,
            });
        }

        let indirect = bits <= kind.max_indirect_bits();
        let palette = if indirect {
            let count = r.count("palette", 1)?;
            let mut p = Vec::with_capacity(count);
            for _ in 0..count {
                p.push(r.varint()? as u32);
            }
            p
        } else {
            Vec::new()
        };

        // Direct storage uses the global-palette width regardless of the
        // byte the server sent; indirect uses the byte value.
        let storage_bits = if indirect { bits } else { kind.global_bits() };
        let cells = read_bit_storage(r, storage_bits, entry_count)?;

        Ok(Self {
            single: None,
            palette,
            cells,
            direct: !indirect,
        })
    }

    /// Global id at a linear cell index (`y*W*W + z*W + x`, W=16 or 4).
    pub fn get(&self, index: usize) -> u32 {
        if let Some(v) = self.single {
            return v;
        }
        let raw = self.cells.get(index).copied().unwrap_or(0);
        if self.direct {
            raw
        } else {
            self.palette.get(raw as usize).copied().unwrap_or(0)
        }
    }

    /// True if this container is uniformly air (state 0) — lets the column
    /// skip empty sections in queries + digests.
    pub fn is_uniform_zero(&self) -> bool {
        matches!(self.single, Some(0))
    }
}

/// Read a fixed-size packed bit array: `entry_count` values at `bits` each,
/// `floor(64/bits)` values per long, low-to-high, no cross-long spanning.
fn read_bit_storage(r: &mut PacketReader, bits: u32, entry_count: usize) -> Result<Vec<u32>> {
    debug_assert!((1..=32).contains(&bits));
    let values_per_long = (64 / bits) as usize;
    let long_count = entry_count.div_ceil(values_per_long);
    let mask = if bits == 32 { u32::MAX } else { (1u32 << bits) - 1 };

    let mut out = Vec::with_capacity(entry_count);
    for _ in 0..long_count {
        let word = r.u64()?;
        for slot in 0..values_per_long {
            if out.len() == entry_count {
                break;
            }
            let shifted = word >> (slot as u32 * bits);
            out.push((shifted as u32) & mask);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_proto::writer::PacketWriter;

    /// Pack `values` the way SimpleBitStorage does, for round-trip tests.
    fn pack(values: &[u32], bits: u32) -> Vec<u8> {
        let vpl = (64 / bits) as usize;
        let mut longs = Vec::new();
        for chunk in values.chunks(vpl) {
            let mut word: u64 = 0;
            for (i, &v) in chunk.iter().enumerate() {
                word |= (v as u64) << (i as u32 * bits);
            }
            longs.push(word);
        }
        let mut out = Vec::new();
        for l in longs {
            out.extend_from_slice(&l.to_be_bytes());
        }
        out
    }

    #[test]
    fn single_value_container() {
        let mut w = PacketWriter::default();
        w.u8(0).varint(42);
        let mut r = PacketReader::new(&w.buf);
        let c = Container::read(&mut r, ContainerKind::BlockStates { global_bits: 15 }).unwrap();
        assert_eq!(c.get(0), 42);
        assert_eq!(c.get(4095), 42);
    }

    #[test]
    fn indirect_palette_roundtrip() {
        // 4-bit linear palette [air, stone, dirt], cell 0=stone, cell 1=dirt.
        let mut indices = vec![0u32; 4096];
        indices[0] = 1;
        indices[1] = 2;
        let mut w = PacketWriter::default();
        w.u8(4).varint(3).varint(0).varint(10).varint(20);
        w.raw(&pack(&indices, 4));
        let mut r = PacketReader::new(&w.buf);
        let c = Container::read(&mut r, ContainerKind::BlockStates { global_bits: 15 }).unwrap();
        assert!(r.is_empty(), "consumed the whole container");
        assert_eq!(c.get(0), 10); // palette[1]
        assert_eq!(c.get(1), 20); // palette[2]
        assert_eq!(c.get(2), 0); // palette[0] = air
    }

    #[test]
    fn direct_storage_roundtrip() {
        // bits=15 > 8 → direct; cell 5 holds global id 12345.
        let mut vals = vec![0u32; 4096];
        vals[5] = 12345;
        let mut w = PacketWriter::default();
        w.u8(15);
        w.raw(&pack(&vals, 15));
        let mut r = PacketReader::new(&w.buf);
        let c = Container::read(&mut r, ContainerKind::BlockStates { global_bits: 15 }).unwrap();
        assert_eq!(c.get(5), 12345);
        assert_eq!(c.get(6), 0);
    }
}
