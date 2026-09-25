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
use rewo_proto::{ProtoError, Result};

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
///
/// The fields are `pub(crate)` **only** so [`crate::chunk_cache`] can
/// destructure them exhaustively. That destructuring is the guard: add a field
/// here and the cache's encoder stops compiling, rather than quietly writing a
/// cache entry that decodes into a *plausible* container missing the new state.
#[derive(Clone)]
pub struct Container {
    /// `None` = single value fills the whole container.
    pub(crate) single: Option<u32>,
    /// Palette (indirect) or empty (direct — storage holds global ids).
    pub(crate) palette: Vec<u32>,
    /// Unpacked per-cell values: either palette indices (indirect) or global
    /// ids (direct/single). Length is `entry_count`.
    ///
    /// `u16` is enough for both: a palette has at most 4096 entries and the
    /// largest global block-state id in 26.2 is 32365, so a non-uniform
    /// section costs 8 KiB here instead of 16. [`Container::get`] widens on
    /// read; nothing outside this type sees the width.
    pub(crate) cells: Vec<u16>,
    pub(crate) direct: bool,
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
        // byte the server sent; indirect uses the byte value. The width is
        // clamped to >= 1: a single-biome registry has `ceil_log2(1) == 0`, and
        // although such a section never sends a direct container, a 0-bit
        // `read_bit_storage` would divide by zero.
        let storage_bits = if indirect { bits } else { kind.global_bits().max(1) };
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
        // The cells are `u16`; the API stays `u32`, so widen here.
        let raw = self.cells.get(index).copied().unwrap_or(0) as u32;
        if self.direct {
            raw
        } else {
            self.palette.get(raw as usize).copied().unwrap_or(0)
        }
    }

    /// Write one cell's global id (`PalettedContainer.set`). A single-value
    /// container expands into a one-entry palette; an indirect palette grows
    /// by appending (the in-memory cells hold full indices, so it never needs
    /// re-packing); a direct container stores the id.
    ///
    /// A direct container can only hold ids that fit its `u16` cells: an id
    /// above `u16::MAX` is **not** written (never truncated to a plausible
    /// wrong state) — it is logged and the cell keeps its old value. Palette
    /// indices need no such guard, since a palette has at most 4096 entries.
    pub fn set(&mut self, index: usize, value: u32, entry_count: usize) {
        if let Some(v) = self.single {
            if v == value {
                return;
            }
            self.single = None;
            self.palette = vec![v];
            self.cells = vec![0; entry_count];
            self.direct = false;
        }
        let Some(cell) = self.cells.get_mut(index) else {
            return;
        };
        if self.direct {
            let Ok(id) = u16::try_from(value) else {
                log::warn!("palette: state id {value} exceeds u16, leaving cell {index} unchanged");
                return;
            };
            *cell = id;
            return;
        }
        let idx = match self.palette.iter().position(|&p| p == value) {
            Some(i) => i,
            None => {
                self.palette.push(value);
                self.palette.len() - 1
            }
        };
        debug_assert!(
            idx <= u16::MAX as usize,
            "palette index {idx} does not fit a u16 cell"
        );
        *cell = idx as u16;
    }

    /// True if this container is uniformly air (state 0) — lets the column
    /// skip empty sections in queries + digests.
    pub fn is_uniform_zero(&self) -> bool {
        matches!(self.single, Some(0))
    }
}

/// Read a fixed-size packed bit array: `entry_count` values at `bits` each,
/// `floor(64/bits)` values per long, low-to-high, no cross-long spanning.
///
/// `bits` is still accepted up to 32 on the wire (direct block storage is 15
/// in 26.2), but a decoded value that does not fit the `u16` cells is an
/// error rather than a silent truncation.
fn read_bit_storage(r: &mut PacketReader, bits: u32, entry_count: usize) -> Result<Vec<u16>> {
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
            let value = (shifted as u32) & mask;
            let Ok(value) = u16::try_from(value) else {
                return Err(ProtoError::LengthOutOfRange {
                    what: "bit storage value",
                    len: value as i64,
                    max: u16::MAX as usize,
                });
            };
            out.push(value);
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
    fn set_writes_through_every_representation() {
        // Single → a one-entry palette plus the new id.
        let mut c = Container::single(7);
        c.set(5, 7, 4096);
        assert!(c.single == Some(7), "an unchanged write keeps the single value");
        c.set(5, 9, 4096);
        assert_eq!((c.get(5), c.get(0), c.get(4095)), (9, 7, 7));
        assert_eq!(c.palette, vec![7, 9]);
        // An existing palette entry is reused, a new one appended.
        c.set(6, 7, 4096);
        c.set(7, 11, 4096);
        assert_eq!((c.get(6), c.get(7)), (7, 11));
        assert_eq!(c.palette.len(), 3);
        // Direct storage holds the id itself.
        let mut w = PacketWriter::default();
        w.u8(15);
        w.raw(&pack(&vec![0u32; 4096], 15));
        let mut r = PacketReader::new(&w.buf);
        let mut d = Container::read(&mut r, ContainerKind::BlockStates { global_bits: 15 }).unwrap();
        d.set(100, 23456, 4096);
        assert_eq!((d.get(100), d.get(101)), (23456, 0));
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

    #[test]
    fn read_rejects_value_above_u16() {
        // A direct container body at a 17-bit width (17 > 8, so the storage is
        // direct and follows the global width): 70000 fits 17 bits but not a
        // u16 cell. The body holds the full 4096 values (1366 longs at 3
        // values each), so a silently truncating decoder would produce a
        // container here — only the range check rejects it.
        let mut w = PacketWriter::default();
        w.u8(17);
        let mut words = vec![0u64; 4096usize.div_ceil(64 / 17)];
        words[0] = 70000;
        for word in words {
            w.raw(&word.to_be_bytes());
        }
        let mut r = PacketReader::new(&w.buf);
        match Container::read(&mut r, ContainerKind::BlockStates { global_bits: 17 }) {
            Err(ProtoError::LengthOutOfRange { len, .. }) => assert_eq!(len, 70000),
            Err(e) => panic!("expected a value-range rejection, got {e:?}"),
            Ok(_) => panic!("expected a rejection, got a decoded container"),
        }
    }

    #[test]
    fn set_direct_rejects_oversized_id() {
        // A direct container's cells hold the global id itself, so an id that
        // does not fit a cell must leave the cell alone rather than truncate
        // into a plausible wrong state.
        let mut w = PacketWriter::default();
        w.u8(15);
        w.raw(&pack(&vec![0u32; 4096], 15));
        let mut r = PacketReader::new(&w.buf);
        let mut d =
            Container::read(&mut r, ContainerKind::BlockStates { global_bits: 15 }).unwrap();
        let before = d.get(0);
        d.set(0, 70_000, 4096);
        assert_eq!(d.get(0), before, "the cell must keep its old value");
        // Control: the same cell still accepts an id that fits, so the pass
        // above is the guard firing and not `set` silently doing nothing.
        d.set(0, 700, 4096);
        assert_eq!(d.get(0), 700);
    }

    #[test]
    fn cells_are_u16() {
        // A decoded indirect container: the cells hold palette indices, at two
        // bytes each — which is the whole point of the `u16` storage.
        let mut indices = vec![0u32; 4096];
        indices[0] = 1;
        let mut w = PacketWriter::default();
        w.u8(4).varint(2).varint(0).varint(10);
        w.raw(&pack(&indices, 4));
        let mut r = PacketReader::new(&w.buf);
        let c = Container::read(&mut r, ContainerKind::BlockStates { global_bits: 15 }).unwrap();
        assert_eq!(std::mem::size_of_val(&c.cells[0]), 2);
    }
}
