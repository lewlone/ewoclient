//! Client-side chunk cache — the storage + eviction layer (M52g).
//!
//! A vanilla server sends chunks only inside its own view distance, so a client
//! that wants to render further than the server serves has to remember columns
//! it has already been sent. Bobby (LGPL-3.0) established the shape of that
//! idea; this is an independent implementation against Rewo's own
//! [`Column`](crate::chunk::Column) — no Bobby code was read or copied, and the
//! on-disk format below is Rewo's, not Bobby's.
//!
//! **This module is storage only.** Nothing here is wired into the live chunk
//! loader or the renderer; `put` and `get` are called by nobody yet. Serving a
//! cached column into a running world needs live verification (the light
//! engine, the mesher, and the server's own unload timing all have opinions),
//! and that is deliberately a separate step.
//!
//! # The failure mode this format is built around
//!
//! Almost every mistake available here produces *plausible* output rather than
//! an obvious failure, which is why so much of the header is redundant checks:
//!
//! - A column decoded against the **wrong dimension shape** yields the right
//!   number of *bytes* and the wrong number of *sections* — a Nether column
//!   read as an Overworld one is terrain, just not the terrain that is there.
//!   Hence `min_y` / `height` in the header, verified against the caller's
//!   shape.
//! - A column decoded under the **wrong coordinates** renders perfectly, in the
//!   wrong place. Hence `cx` / `cz` in the header, verified against the key the
//!   caller asked for.
//! - A cache written by an **older build** decodes into something structurally
//!   valid and semantically stale — the single most expensive bug available in
//!   this file, because it looks like a world bug rather than a cache bug.
//!   Hence [`FORMAT_VERSION`], which is a hard equality check in both
//!   directions: a *newer* file is rejected too, so downgrading a build cannot
//!   silently misread it.
//! - A **truncated or corrupted** file decodes into whatever the trailing bytes
//!   happen to say. Hence the body length and the body hash, and hence every
//!   read in [`ByteReader`] being bounds-checked rather than indexing.
//!
//! Every one of those is a rejection, never a repair and never a panic: a cache
//! miss costs a chunk request, a bad hit costs a debugging session.
//!
//! # Layout
//!
//! ```text
//! <config>/EwoClient/rewo/chunks/<world-key>/c.<cx>.<cz>.rwc
//! ```
//!
//! and each file is
//!
//! ```text
//! offset size  field
//!      0    4  magic  b"RWCC"
//!      4    4  format version (u32 LE) — must equal FORMAT_VERSION exactly
//!      8    4  cx (i32 LE)
//!     12    4  cz (i32 LE)
//!     16    4  dimension min_y (i32 LE)
//!     20    4  dimension height (i32 LE)
//!     24    4  body length (u32 LE)
//!     28    8  body hash (u64 LE, FNV-1a over the body bytes)
//!     36    n  body
//! ```
//!
//! The body is the column: section count, then per section the non-empty count,
//! the two paletted containers, the two optional light nibble arrays and the
//! block-update overrides; then `sky_full_above`, the optional
//! `MOTION_BLOCKING` heightmap, and the block entities.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rewo_proto::nbt::Nbt;

use crate::block_entities::{BlockEntity, BlockEntityPos};
use crate::chunk::{Column, Section};
use crate::dimension::DimensionShape;
use crate::palette::Container;

/// `b"RWCC"` — Rewo Chunk Cache.
const MAGIC: [u8; 4] = *b"RWCC";

/// Bump on **any** change to the body encoding, to `Column`/`Section`/
/// `Container`'s fields, or to what those fields mean.
///
/// A stale entry that still parses is the expensive failure this guards, so the
/// check is equality rather than `>=`: neither an older nor a newer file is
/// accepted, and the cost of being wrong is one re-request.
pub const FORMAT_VERSION: u32 = 1;

/// Bytes before the body.
const HEADER_LEN: usize = 36;

/// Why a cached column was not usable. Every variant is "treat this as a cache
/// miss"; none is recoverable, and none of them is a reason to panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheError {
    /// Not a cache file at all.
    BadMagic,
    /// Written by a different build of this format.
    Version { found: u32, expected: u32 },
    /// Stored coordinates disagree with the key the caller asked for — a
    /// mis-keyed index, a copied file, or a renamed directory.
    WrongCoords {
        found: (i32, i32),
        expected: (i32, i32),
    },
    /// Stored dimension shape disagrees with the world asking for it.
    WrongShape {
        found: DimensionShape,
        expected: DimensionShape,
    },
    /// A read ran off the end of the buffer, or trailing bytes were left over.
    Truncated,
    /// The body hash does not match its bytes.
    Corrupt,
    /// A field held a value the decoder cannot represent (a negative count, an
    /// impossible bit width, a light array that is not 2048 bytes).
    Malformed(&'static str),
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheError::BadMagic => write!(f, "not a chunk-cache file"),
            CacheError::Version { found, expected } => {
                write!(f, "cache format {found}, this build writes {expected}")
            }
            CacheError::WrongCoords { found, expected } => write!(
                f,
                "cached column is ({}, {}), asked for ({}, {})",
                found.0, found.1, expected.0, expected.1
            ),
            CacheError::WrongShape { found, expected } => write!(
                f,
                "cached for min_y {} height {}, world is min_y {} height {}",
                found.min_y, found.height, expected.min_y, expected.height
            ),
            CacheError::Truncated => write!(f, "truncated or over-long"),
            CacheError::Corrupt => write!(f, "body hash mismatch"),
            CacheError::Malformed(what) => write!(f, "malformed: {what}"),
        }
    }
}

impl std::error::Error for CacheError {}

// ---------------------------------------------------------------------------
// byte plumbing
// ---------------------------------------------------------------------------

/// A bounds-checked cursor. Every read returns `Result`; nothing here indexes a
/// slice directly, because a truncated cache file is an ordinary occurrence
/// (a half-written file after a crash) and must not be a panic.
struct ByteReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], CacheError> {
        let end = self.pos.checked_add(n).ok_or(CacheError::Truncated)?;
        let slice = self.buf.get(self.pos..end).ok_or(CacheError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, CacheError> {
        Ok(self.take(1)?[0])
    }

    fn i16(&mut self) -> Result<i16, CacheError> {
        let b = self.take(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, CacheError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i32(&mut self) -> Result<i32, CacheError> {
        Ok(self.u32()? as i32)
    }

    fn u64(&mut self) -> Result<u64, CacheError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn i64(&mut self) -> Result<i64, CacheError> {
        Ok(self.u64()? as i64)
    }

    fn f32(&mut self) -> Result<f32, CacheError> {
        Ok(f32::from_bits(self.u32()?))
    }

    fn f64(&mut self) -> Result<f64, CacheError> {
        Ok(f64::from_bits(self.u64()?))
    }

    /// A length that will be used to size a `Vec`. Rejected when it exceeds the
    /// bytes actually remaining at `min_elem_bytes` each, so a corrupt length
    /// cannot make the decoder allocate gigabytes before it fails.
    fn count(&mut self, min_elem_bytes: usize) -> Result<usize, CacheError> {
        let n = self.u32()? as usize;
        let remaining = self.buf.len() - self.pos;
        if min_elem_bytes > 0 && n.saturating_mul(min_elem_bytes) > remaining {
            return Err(CacheError::Truncated);
        }
        Ok(n)
    }

    fn is_empty(&self) -> bool {
        self.pos == self.buf.len()
    }
}

fn put_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}
fn put_i16(out: &mut Vec<u8>, v: i16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_i64(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// FNV-1a. Not a cryptographic checksum and not meant to be — its whole job is
/// to catch a truncated write or a flipped bit before the bytes are believed.
fn body_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// ---------------------------------------------------------------------------
// bit-packed u32 runs
// ---------------------------------------------------------------------------

/// A section's 4096 block-state cells stored one-per-u32 would be 16 KiB, and a
/// full Overworld column has 24 of them. Packing at the width the values
/// actually need takes a typical indirect palette to ~2 KiB — losslessly, since
/// the width is derived from the maximum value present and written down.
///
/// The packing convention is the wire's (`floor(64/bits)` values per word, no
/// value straddling a word boundary), so it reads the same way
/// `palette::read_bit_storage` does.
fn put_packed(out: &mut Vec<u8>, values: &[u32]) {
    let bits = bits_needed(values.iter().copied().max().unwrap_or(0));
    put_u8(out, bits as u8);
    put_u32(out, values.len() as u32);
    let per_word = (64 / bits) as usize;
    for chunk in values.chunks(per_word) {
        let mut word: u64 = 0;
        for (i, &v) in chunk.iter().enumerate() {
            word |= (v as u64) << (i as u32 * bits);
        }
        put_u64(out, word);
    }
}

fn read_packed(r: &mut ByteReader) -> Result<Vec<u32>, CacheError> {
    let bits = r.u8()? as u32;
    if !(1..=32).contains(&bits) {
        return Err(CacheError::Malformed("bit width out of range"));
    }
    let per_word = (64 / bits) as usize;
    // Each *word* is 8 bytes and carries `per_word` values, so the byte floor
    // for `n` values is `ceil(n / per_word) * 8`. Bounding on that stops a
    // corrupt length from reserving a huge Vec.
    let len = r.u32()? as usize;
    let words = len.div_ceil(per_word);
    if words.saturating_mul(8) > r.buf.len() - r.pos {
        return Err(CacheError::Truncated);
    }
    let mask = if bits == 32 { u32::MAX } else { (1u32 << bits) - 1 };
    let mut out = Vec::with_capacity(len);
    for _ in 0..words {
        let word = r.u64()?;
        for slot in 0..per_word {
            if out.len() == len {
                break;
            }
            out.push(((word >> (slot as u32 * bits)) as u32) & mask);
        }
    }
    Ok(out)
}

/// Bits needed to hold `max`, clamped to 1 so an all-zero run still has a
/// non-zero width (a 0-bit packing would divide by zero on the way back).
fn bits_needed(max: u32) -> u32 {
    (32 - max.leading_zeros()).max(1)
}

// ---------------------------------------------------------------------------
// NBT
// ---------------------------------------------------------------------------

/// `rewo_proto::nbt` is a reader only — it has no writer, because until now
/// nothing needed to *emit* NBT. The cache does, so it carries its own
/// encoding, which is deliberately **not** the wire format: it is a private
/// form whose only contract is round-tripping [`Nbt`], and it is versioned by
/// [`FORMAT_VERSION`] along with everything else here.
fn put_nbt(out: &mut Vec<u8>, nbt: &Nbt) {
    match nbt {
        Nbt::End => put_u8(out, 0),
        Nbt::Byte(v) => {
            put_u8(out, 1);
            put_u8(out, *v as u8);
        }
        Nbt::Short(v) => {
            put_u8(out, 2);
            put_i16(out, *v);
        }
        Nbt::Int(v) => {
            put_u8(out, 3);
            put_i32(out, *v);
        }
        Nbt::Long(v) => {
            put_u8(out, 4);
            put_i64(out, *v);
        }
        // f32/f64 go through `to_bits`, not a decimal formatting: a NaN payload
        // and a signed zero both have to survive, since the tag they came from
        // is compared for equality elsewhere.
        Nbt::Float(v) => {
            put_u8(out, 5);
            put_u32(out, v.to_bits());
        }
        Nbt::Double(v) => {
            put_u8(out, 6);
            put_u64(out, v.to_bits());
        }
        Nbt::ByteArray(b) => {
            put_u8(out, 7);
            put_u32(out, b.len() as u32);
            out.extend_from_slice(b);
        }
        Nbt::String(s) => {
            put_u8(out, 8);
            put_u32(out, s.len() as u32);
            out.extend_from_slice(s.as_bytes());
        }
        Nbt::List(items) => {
            put_u8(out, 9);
            put_u32(out, items.len() as u32);
            for item in items {
                put_nbt(out, item);
            }
        }
        Nbt::Compound(entries) => {
            put_u8(out, 10);
            put_u32(out, entries.len() as u32);
            for (k, v) in entries {
                put_u32(out, k.len() as u32);
                out.extend_from_slice(k.as_bytes());
                put_nbt(out, v);
            }
        }
        Nbt::IntArray(v) => {
            put_u8(out, 11);
            put_u32(out, v.len() as u32);
            for i in v {
                put_i32(out, *i);
            }
        }
        Nbt::LongArray(v) => {
            put_u8(out, 12);
            put_u32(out, v.len() as u32);
            for i in v {
                put_i64(out, *i);
            }
        }
    }
}

/// Mirrors [`put_nbt`]. `depth` bounds recursion for the same reason the wire
/// reader does: a corrupt file must not blow the stack.
const NBT_MAX_DEPTH: u32 = 128;

fn read_nbt(r: &mut ByteReader, depth: u32) -> Result<Nbt, CacheError> {
    if depth > NBT_MAX_DEPTH {
        return Err(CacheError::Malformed("nbt nested too deep"));
    }
    let tag = r.u8()?;
    Ok(match tag {
        0 => Nbt::End,
        1 => Nbt::Byte(r.u8()? as i8),
        2 => Nbt::Short(r.i16()?),
        3 => Nbt::Int(r.i32()?),
        4 => Nbt::Long(r.i64()?),
        5 => Nbt::Float(r.f32()?),
        6 => Nbt::Double(r.f64()?),
        7 => {
            let n = r.count(1)?;
            Nbt::ByteArray(r.take(n)?.to_vec())
        }
        8 => Nbt::String(read_str(r)?),
        9 => {
            // One byte is the smallest an element can be (a bare tag).
            let n = r.count(1)?;
            let mut items = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                items.push(read_nbt(r, depth + 1)?);
            }
            Nbt::List(items)
        }
        10 => {
            // 4 bytes of key length + 1 tag byte is the smallest entry.
            let n = r.count(5)?;
            let mut entries = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                let key = read_str(r)?;
                entries.push((key, read_nbt(r, depth + 1)?));
            }
            Nbt::Compound(entries)
        }
        11 => {
            let n = r.count(4)?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(r.i32()?);
            }
            Nbt::IntArray(v)
        }
        12 => {
            let n = r.count(8)?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(r.i64()?);
            }
            Nbt::LongArray(v)
        }
        _ => return Err(CacheError::Malformed("unknown nbt tag")),
    })
}

fn read_str(r: &mut ByteReader) -> Result<String, CacheError> {
    let n = r.count(1)?;
    let bytes = r.take(n)?;
    // `Nbt::String` is already the product of a lossy decode on the wire, so by
    // the time a string reaches the cache it is valid UTF-8 and this branch
    // does not fire. Erroring rather than replacing keeps that assumption
    // checkable instead of silently papering over a corrupt length.
    String::from_utf8(bytes.to_vec()).map_err(|_| CacheError::Malformed("nbt string not utf-8"))
}

// ---------------------------------------------------------------------------
// container / section / column
// ---------------------------------------------------------------------------

fn put_container(out: &mut Vec<u8>, c: &Container) {
    // Destructured, not field-accessed: adding a field to `Container` breaks
    // this line, which is the point. See the note on the type.
    let Container {
        single,
        palette,
        cells,
        direct,
    } = c;
    match single {
        Some(v) => {
            put_u8(out, 0);
            put_u32(out, *v);
        }
        None => {
            put_u8(out, if *direct { 2 } else { 1 });
            if !*direct {
                put_u32(out, palette.len() as u32);
                for v in palette {
                    put_u32(out, *v);
                }
            }
            put_packed(out, cells);
        }
    }
}

fn read_container(r: &mut ByteReader) -> Result<Container, CacheError> {
    match r.u8()? {
        0 => Ok(Container {
            single: Some(r.u32()?),
            palette: Vec::new(),
            cells: Vec::new(),
            direct: false,
        }),
        kind @ (1 | 2) => {
            let direct = kind == 2;
            let palette = if direct {
                Vec::new()
            } else {
                let n = r.count(4)?;
                let mut p = Vec::with_capacity(n);
                for _ in 0..n {
                    p.push(r.u32()?);
                }
                p
            };
            Ok(Container {
                single: None,
                palette,
                cells: read_packed(r)?,
                direct,
            })
        }
        _ => Err(CacheError::Malformed("container kind")),
    }
}

/// A light nibble array is exactly 2048 bytes when present. Storing the length
/// and checking it on the way back keeps a short array from being accepted and
/// then read as zeros past its end — which would render as a dark band rather
/// than as an error.
fn put_light(out: &mut Vec<u8>, arr: &Option<Vec<u8>>) {
    match arr {
        None => put_u8(out, 0),
        Some(bytes) => {
            put_u8(out, 1);
            put_u32(out, bytes.len() as u32);
            out.extend_from_slice(bytes);
        }
    }
}

fn read_light(r: &mut ByteReader) -> Result<Option<Vec<u8>>, CacheError> {
    match r.u8()? {
        0 => Ok(None),
        1 => {
            let n = r.count(1)?;
            if n != 2048 {
                return Err(CacheError::Malformed("light array is not 2048 bytes"));
            }
            Ok(Some(r.take(n)?.to_vec()))
        }
        _ => Err(CacheError::Malformed("light presence flag")),
    }
}

fn put_section(out: &mut Vec<u8>, s: &Section) {
    let Section {
        non_empty,
        states,
        biomes,
        block_light,
        sky_light,
        overrides,
    } = s;
    put_i16(out, *non_empty);
    put_container(out, states);
    put_container(out, biomes);
    put_light(out, block_light);
    put_light(out, sky_light);
    // Sorted, so encoding a column twice produces identical bytes. A `HashMap`
    // iterates in an unspecified order, and a format whose bytes depend on
    // allocator state cannot be compared byte-for-byte in a test.
    let mut keys: Vec<u16> = overrides.keys().copied().collect();
    keys.sort_unstable();
    put_u32(out, keys.len() as u32);
    for k in keys {
        out.extend_from_slice(&k.to_le_bytes());
        put_u32(out, overrides[&k]);
    }
}

fn read_section(r: &mut ByteReader) -> Result<Section, CacheError> {
    let non_empty = r.i16()?;
    let states = read_container(r)?;
    let biomes = read_container(r)?;
    let block_light = read_light(r)?;
    let sky_light = read_light(r)?;
    let n = r.count(6)?; // u16 key + u32 value
    let mut overrides = HashMap::with_capacity(n);
    for _ in 0..n {
        let b = r.take(2)?;
        let key = u16::from_le_bytes([b[0], b[1]]);
        overrides.insert(key, r.u32()?);
    }
    Ok(Section {
        non_empty,
        states,
        biomes,
        block_light,
        sky_light,
        overrides,
    })
}

/// Encode a column into a complete cache file, header and all.
///
/// `shape` is recorded so [`decode_column`] can refuse to hand the column to a
/// world of a different vertical shape — see the module note.
pub fn encode_column(col: &Column, shape: &DimensionShape) -> Vec<u8> {
    let Column {
        cx,
        cz,
        sections,
        sky_full_above,
        motion_blocking,
        block_entities,
    } = col;

    let mut body = Vec::new();
    put_u32(&mut body, sections.len() as u32);
    for s in sections {
        put_section(&mut body, s);
    }
    // `usize` on the wire is fixed at 64 bits: a cache written on one build
    // must not become unreadable on a 32-bit one, and `usize::MAX` is a real
    // value here (`Column::empty_lit` uses it for "never full-bright").
    put_u64(&mut body, *sky_full_above as u64);
    match motion_blocking {
        None => put_u8(&mut body, 0),
        Some(heights) => {
            put_u8(&mut body, 1);
            for h in heights.iter() {
                put_i32(&mut body, *h);
            }
        }
    }
    put_u32(&mut body, block_entities.len() as u32);
    for (pos, be) in block_entities {
        put_i32(&mut body, pos.x);
        put_i32(&mut body, pos.y);
        put_i32(&mut body, pos.z);
        put_i32(&mut body, be.type_id);
        put_nbt(&mut body, &be.data);
    }

    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.extend_from_slice(&MAGIC);
    put_u32(&mut out, FORMAT_VERSION);
    put_i32(&mut out, *cx);
    put_i32(&mut out, *cz);
    put_i32(&mut out, shape.min_y);
    put_i32(&mut out, shape.height);
    put_u32(&mut out, body.len() as u32);
    put_u64(&mut out, body_hash(&body));
    out.extend_from_slice(&body);
    debug_assert_eq!(out.len(), HEADER_LEN + body.len());
    out
}

/// Decode a cache file, rejecting anything that is not exactly the column the
/// caller asked for at exactly the shape they asked for.
pub fn decode_column(
    bytes: &[u8],
    expect_coords: (i32, i32),
    expect_shape: &DimensionShape,
) -> Result<Column, CacheError> {
    let mut r = ByteReader::new(bytes);
    if r.take(4)? != MAGIC {
        return Err(CacheError::BadMagic);
    }
    let version = r.u32()?;
    if version != FORMAT_VERSION {
        return Err(CacheError::Version {
            found: version,
            expected: FORMAT_VERSION,
        });
    }
    let cx = r.i32()?;
    let cz = r.i32()?;
    if (cx, cz) != expect_coords {
        return Err(CacheError::WrongCoords {
            found: (cx, cz),
            expected: expect_coords,
        });
    }
    let shape = DimensionShape {
        min_y: r.i32()?,
        height: r.i32()?,
    };
    if shape != *expect_shape {
        return Err(CacheError::WrongShape {
            found: shape,
            expected: *expect_shape,
        });
    }
    let body_len = r.u32()? as usize;
    let hash = r.u64()?;
    let body = r.take(body_len)?;
    if !r.is_empty() {
        // Trailing bytes mean the file is not what the header says it is —
        // two entries concatenated, or a partial overwrite of a longer one.
        return Err(CacheError::Truncated);
    }
    if body_hash(body) != hash {
        return Err(CacheError::Corrupt);
    }

    let mut b = ByteReader::new(body);
    let section_count = b.count(2)?;
    // The shape already matched, so the section count is fully determined; a
    // disagreement means the body does not belong to this header.
    if section_count != shape.section_count() {
        return Err(CacheError::Malformed("section count vs dimension shape"));
    }
    let mut sections = Vec::with_capacity(section_count);
    for _ in 0..section_count {
        sections.push(read_section(&mut b)?);
    }
    let sky_full_above = b.u64()? as usize;
    let motion_blocking = match b.u8()? {
        0 => None,
        1 => {
            let mut heights = Box::new([0i32; 256]);
            for h in heights.iter_mut() {
                *h = b.i32()?;
            }
            Some(heights)
        }
        _ => return Err(CacheError::Malformed("heightmap presence flag")),
    };
    let be_count = b.count(17)?; // 4 coords/type × 4 bytes + 1 tag byte
    let mut block_entities = Vec::with_capacity(be_count.min(4096));
    for _ in 0..be_count {
        let pos = BlockEntityPos {
            x: b.i32()?,
            y: b.i32()?,
            z: b.i32()?,
        };
        let type_id = b.i32()?;
        let data = read_nbt(&mut b, 0)?;
        block_entities.push((pos, BlockEntity { type_id, data }));
    }
    if !b.is_empty() {
        return Err(CacheError::Truncated);
    }

    Ok(Column {
        cx,
        cz,
        sections,
        sky_full_above,
        motion_blocking,
        block_entities,
    })
}

// ---------------------------------------------------------------------------
// the store
// ---------------------------------------------------------------------------

/// `<config>/EwoClient/rewo/chunks` — the parent of every world's cache
/// directory, matching `rewo_data::DataPaths`' `<config>/EwoClient/rewo` root.
pub fn cache_root() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("rewo");
    p.push("chunks");
    Some(p)
}

/// A path-safe directory name for one world.
///
/// The inputs are a server address and a dimension key, neither of which is
/// under this client's control, so everything outside `[A-Za-z0-9._-]` becomes
/// `_`. That is lossy on purpose: two servers that collide under it share a
/// cache directory and, at worst, miss each other's entries — every stored
/// column still carries its own coordinates and shape, so a collision can
/// never *serve* one world's terrain to another.
///
/// `.` survives because addresses are full of it, which leaves `.` and `..`
/// reachable as whole keys; the result is therefore checked to be a single
/// ordinary path component and replaced wholesale if it is not. A directory
/// name is not the place to discover that a substitution was insufficient.
pub fn world_key(server: &str, dimension: &str) -> String {
    let mut out = String::with_capacity(server.len() + dimension.len() + 1);
    for part in [server, dimension] {
        if !out.is_empty() {
            out.push('.');
        }
        for ch in part.chars() {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                out.push(ch);
            } else {
                out.push('_');
            }
        }
    }
    let single_component = matches!(
        Path::new(&out).components().collect::<Vec<_>>().as_slice(),
        [std::path::Component::Normal(_)]
    );
    if !single_component {
        return "unknown".to_string();
    }
    out
}

/// One entry's bookkeeping. `bytes` is the file size as written, which is what
/// the budget is spent in; `used` is the LRU stamp.
#[derive(Debug, Clone, Copy)]
struct CacheEntry {
    bytes: u64,
    used: u64,
}

/// A size-bounded, LRU-evicting store of cached columns for one world.
///
/// Not thread-safe and not meant to be: it owns a directory and an index of it,
/// and a second handle on the same directory would evict against a stale view.
/// Wrap it if it ever needs sharing.
pub struct ChunkCache {
    root: PathBuf,
    max_bytes: u64,
    total_bytes: u64,
    /// Monotonic LRU stamp. A counter rather than a clock: two writes inside
    /// one filesystem timestamp tick are indistinguishable by mtime on
    /// Windows, and eviction wants a total order.
    clock: u64,
    entries: HashMap<(i32, i32), CacheEntry>,
}

impl ChunkCache {
    /// Open (creating if needed) a cache directory and index what is already
    /// there.
    ///
    /// Recency is rebuilt from file mtime, which is a **hint**, not a record: a
    /// column read a thousand times and written once looks as cold as one
    /// written and never read, because [`get`](Self::get) deliberately performs
    /// no write. The size bound never depends on it — only which entry is
    /// dropped first does, and dropping the wrong one costs a chunk request.
    ///
    /// Files that are not cache entries, and entries whose names do not parse,
    /// are ignored rather than deleted: this directory is the user's, and a
    /// cache is not licensed to remove things it does not recognise.
    pub fn open(root: impl Into<PathBuf>, max_bytes: u64) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;

        let mut found: Vec<((i32, i32), u64, std::time::SystemTime)> = Vec::new();
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(key) = parse_entry_name(name) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            found.push((key, meta.len(), mtime));
        }
        found.sort_by_key(|(_, _, mtime)| *mtime);

        let mut entries = HashMap::with_capacity(found.len());
        let mut total_bytes = 0u64;
        for (i, (key, bytes, _)) in found.iter().enumerate() {
            total_bytes += bytes;
            entries.insert(
                *key,
                CacheEntry {
                    bytes: *bytes,
                    used: i as u64,
                },
            );
        }

        let mut cache = Self {
            root,
            max_bytes,
            total_bytes,
            clock: found.len() as u64,
            entries,
        };
        // A budget that shrank between runs is enforced now rather than on the
        // next write, so opening a cache never leaves it over its bound.
        cache.evict_to_fit();
        Ok(cache)
    }

    /// Open the cache for one world under [`cache_root`]. `None` when the
    /// platform has no config directory.
    pub fn open_for_world(world_key: &str, max_bytes: u64) -> Option<io::Result<Self>> {
        let root = cache_root()?.join(world_key);
        Some(Self::open(root, max_bytes))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, cx: i32, cz: i32) -> bool {
        self.entries.contains_key(&(cx, cz))
    }

    /// Read a cached column back.
    ///
    /// Every failure — missing file, unreadable file, wrong version, wrong
    /// shape, corrupt body — is a plain `None`, because there is no failure
    /// here a caller can do anything with other than ask the server. A file
    /// that exists but does not decode is **dropped**, so a permanently bad
    /// entry cannot hold its share of the budget forever.
    pub fn get(&mut self, cx: i32, cz: i32, shape: &DimensionShape) -> Option<Column> {
        if !self.entries.contains_key(&(cx, cz)) {
            return None;
        }
        let path = self.path_for(cx, cz);
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                log::debug!("chunk cache: read {} failed: {e}", path.display());
                self.forget(cx, cz);
                return None;
            }
        };
        match decode_column(&bytes, (cx, cz), shape) {
            Ok(col) => {
                self.clock += 1;
                let clock = self.clock;
                if let Some(entry) = self.entries.get_mut(&(cx, cz)) {
                    entry.used = clock;
                }
                Some(col)
            }
            Err(e) => {
                log::debug!("chunk cache: rejecting {}: {e}", path.display());
                self.forget(cx, cz);
                None
            }
        }
    }

    /// Store a column, evicting least-recently-used entries until the total
    /// fits the budget.
    ///
    /// The write goes to a temporary file and is renamed into place, so a crash
    /// mid-write leaves either the old entry or none — never a half-file that
    /// the hash check would have to catch on every subsequent read.
    pub fn put(&mut self, col: &Column, shape: &DimensionShape) -> io::Result<()> {
        let (cx, cz) = (col.cx, col.cz);
        let bytes = encode_column(col, shape);
        let path = self.path_for(cx, cz);
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, &bytes)?;
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }

        self.clock += 1;
        let entry = CacheEntry {
            bytes: bytes.len() as u64,
            used: self.clock,
        };
        if let Some(old) = self.entries.insert((cx, cz), entry) {
            self.total_bytes -= old.bytes;
        }
        self.total_bytes += entry.bytes;
        self.evict_to_fit();
        Ok(())
    }

    /// Drop one entry, file and all.
    pub fn remove(&mut self, cx: i32, cz: i32) {
        let path = self.path_for(cx, cz);
        if let Err(e) = fs::remove_file(&path) {
            if e.kind() != io::ErrorKind::NotFound {
                log::debug!("chunk cache: remove {} failed: {e}", path.display());
            }
        }
        self.forget(cx, cz);
    }

    /// Forget an entry's bookkeeping without touching the filesystem.
    fn forget(&mut self, cx: i32, cz: i32) {
        if let Some(old) = self.entries.remove(&(cx, cz)) {
            self.total_bytes -= old.bytes;
        }
    }

    fn path_for(&self, cx: i32, cz: i32) -> PathBuf {
        self.root.join(entry_name(cx, cz))
    }

    /// Evict least-recently-used entries until the total is within budget.
    ///
    /// A single entry larger than the whole budget is evicted like any other,
    /// which leaves the cache empty rather than permanently over its bound —
    /// the bound is the promise, and a column that cannot be cached simply is
    /// not cached.
    fn evict_to_fit(&mut self) {
        if self.total_bytes <= self.max_bytes {
            return;
        }
        let mut by_age: Vec<((i32, i32), u64)> = self
            .entries
            .iter()
            .map(|(k, v)| (*k, v.used))
            .collect();
        by_age.sort_unstable_by_key(|(_, used)| *used);
        for (key, _) in by_age {
            if self.total_bytes <= self.max_bytes {
                break;
            }
            self.remove(key.0, key.1);
        }
    }
}

/// `c.<cx>.<cz>.rwc`. Negative coordinates keep their `-`, which is why the
/// parser splits on `.` rather than scanning for digits.
fn entry_name(cx: i32, cz: i32) -> String {
    format!("c.{cx}.{cz}.rwc")
}

fn parse_entry_name(name: &str) -> Option<(i32, i32)> {
    let rest = name.strip_prefix("c.")?.strip_suffix(".rwc")?;
    let (cx, cz) = rest.split_once('.')?;
    Some((cx.parse().ok()?, cz.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Column;

    // -- fixtures ---------------------------------------------------------

    fn shape() -> DimensionShape {
        DimensionShape {
            min_y: 0,
            height: 32,
        } // two sections — enough structure, small enough to read
    }

    fn indirect_container() -> Container {
        let mut cells = vec![0u32; 4096];
        cells[0] = 1;
        cells[1] = 2;
        cells[4095] = 2;
        Container {
            single: None,
            palette: vec![0, 10, 20],
            cells,
            direct: false,
        }
    }

    fn direct_container() -> Container {
        let mut cells = vec![0u32; 4096];
        cells[5] = 12345;
        cells[4095] = u32::MAX; // forces the 32-bit packing path
        Container {
            single: None,
            palette: Vec::new(),
            cells,
            direct: true,
        }
    }

    /// A column exercising every branch of the encoder at once: both container
    /// kinds, present and absent light on both channels, overrides, a
    /// heightmap, and block entities carrying every NBT tag.
    fn rich_column() -> Column {
        let section_a = Section {
            non_empty: 512,
            states: indirect_container(),
            biomes: Container::single(7),
            block_light: Some((0..2048).map(|i| (i % 251) as u8).collect()),
            sky_light: None,
            overrides: [(0u16, 99u32), (4095, 5), (17, 1)].into_iter().collect(),
        };
        let section_b = Section {
            non_empty: 0,
            states: direct_container(),
            biomes: indirect_container(),
            block_light: None,
            sky_light: Some(vec![0xFF; 2048]),
            overrides: HashMap::new(),
        };
        let mut heights = Box::new([0i32; 256]);
        for (i, h) in heights.iter_mut().enumerate() {
            *h = (i as i32) - 64;
        }
        Column {
            cx: -3,
            cz: 17,
            sections: vec![section_a, section_b],
            sky_full_above: usize::MAX,
            motion_blocking: Some(heights),
            block_entities: vec![
                (
                    BlockEntityPos {
                        x: -40,
                        y: 12,
                        z: 273,
                    },
                    BlockEntity {
                        type_id: 9,
                        data: every_nbt_tag(),
                    },
                ),
                (
                    BlockEntityPos { x: -33, y: 1, z: 272 },
                    BlockEntity {
                        type_id: 0,
                        data: Nbt::End,
                    },
                ),
            ],
        }
    }

    fn every_nbt_tag() -> Nbt {
        Nbt::Compound(vec![
            ("end".into(), Nbt::End),
            ("byte".into(), Nbt::Byte(-128)),
            ("short".into(), Nbt::Short(-32768)),
            ("int".into(), Nbt::Int(i32::MIN)),
            ("long".into(), Nbt::Long(i64::MIN)),
            ("float".into(), Nbt::Float(-0.0)),
            ("double".into(), Nbt::Double(f64::MIN_POSITIVE)),
            ("bytes".into(), Nbt::ByteArray(vec![0, 1, 255])),
            ("str".into(), Nbt::String("sign line ⛏".into())),
            (
                "list".into(),
                Nbt::List(vec![Nbt::Int(1), Nbt::Int(2), Nbt::Int(3)]),
            ),
            (
                "nested".into(),
                Nbt::Compound(vec![("inner".into(), Nbt::String(String::new()))]),
            ),
            ("ints".into(), Nbt::IntArray(vec![-1, 0, i32::MAX])),
            ("longs".into(), Nbt::LongArray(vec![i64::MAX, 0])),
        ])
    }

    /// Compare two columns field by field. Destructured on both sides so that a
    /// new field on `Column` or `Section` fails to compile here as well as in
    /// the encoder — a test that silently stops covering a field is worse than
    /// no test.
    fn assert_columns_equal(a: &Column, b: &Column) {
        let Column {
            cx: acx,
            cz: acz,
            sections: asec,
            sky_full_above: asky,
            motion_blocking: amb,
            block_entities: abe,
        } = a;
        let Column {
            cx: bcx,
            cz: bcz,
            sections: bsec,
            sky_full_above: bsky,
            motion_blocking: bmb,
            block_entities: bbe,
        } = b;
        assert_eq!((acx, acz), (bcx, bcz), "coords");
        assert_eq!(asky, bsky, "sky_full_above");
        assert_eq!(
            amb.as_ref().map(|h| h.to_vec()),
            bmb.as_ref().map(|h| h.to_vec()),
            "motion_blocking"
        );
        assert_eq!(abe, bbe, "block entities");
        assert_eq!(asec.len(), bsec.len(), "section count");
        for (i, (x, y)) in asec.iter().zip(bsec).enumerate() {
            let Section {
                non_empty: an,
                states: ast,
                biomes: abi,
                block_light: abl,
                sky_light: asl,
                overrides: aov,
            } = x;
            let Section {
                non_empty: bn,
                states: bst,
                biomes: bbi,
                block_light: bbl,
                sky_light: bsl,
                overrides: bov,
            } = y;
            assert_eq!(an, bn, "section {i} non_empty");
            assert_containers_equal(ast, bst, i, "states");
            assert_containers_equal(abi, bbi, i, "biomes");
            assert_eq!(abl, bbl, "section {i} block_light");
            assert_eq!(asl, bsl, "section {i} sky_light");
            assert_eq!(aov, bov, "section {i} overrides");
        }
    }

    fn assert_containers_equal(a: &Container, b: &Container, i: usize, what: &str) {
        let Container {
            single: as_,
            palette: ap,
            cells: ac,
            direct: ad,
        } = a;
        let Container {
            single: bs,
            palette: bp,
            cells: bc,
            direct: bd,
        } = b;
        assert_eq!(as_, bs, "section {i} {what} single");
        assert_eq!(ap, bp, "section {i} {what} palette");
        assert_eq!(ac, bc, "section {i} {what} cells");
        assert_eq!(ad, bd, "section {i} {what} direct");
    }

    fn tempdir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "rewo-chunk-cache-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&p);
        p
    }

    // -- round trip -------------------------------------------------------

    #[test]
    fn a_column_survives_the_round_trip_field_for_field() {
        let col = rich_column();
        let bytes = encode_column(&col, &shape());
        let back = decode_column(&bytes, (col.cx, col.cz), &shape()).expect("decodes");
        assert_columns_equal(&col, &back);
    }

    #[test]
    fn re_encoding_a_decoded_column_reproduces_the_bytes_exactly() {
        let bytes = encode_column(&rich_column(), &shape());
        let back = decode_column(&bytes, (-3, 17), &shape()).unwrap();
        assert_eq!(encode_column(&back, &shape()), bytes);
    }

    #[test]
    fn the_decoded_column_answers_block_light_and_biome_queries_identically() {
        // The field comparison proves the bytes; this proves the bytes mean
        // the same thing through the accessors a renderer actually calls.
        let shape = shape();
        let mut col = Column::empty_lit(&shape, 4, -9);
        col.set_block(&shape, 3, 5, 11, 4321);
        col.set_block(&shape, 0, 31, 15, 7);
        let bytes = encode_column(&col, &shape);
        let back = decode_column(&bytes, (4, -9), &shape).unwrap();

        for (x, y, z) in [(3, 5, 11), (0, 31, 15), (1, 1, 1), (15, 0, 15)] {
            assert_eq!(
                col.block_state_at(&shape, x, y, z),
                back.block_state_at(&shape, x, y, z),
                "state at {x},{y},{z}"
            );
            assert_eq!(
                col.light_at(&shape, x, y, z),
                back.light_at(&shape, x, y, z),
                "light at {x},{y},{z}"
            );
        }
        assert_eq!(
            col.noise_biome_at_quart(&shape, 16, 0, -36),
            back.noise_biome_at_quart(&shape, 16, 0, -36)
        );
    }

    #[test]
    fn an_all_air_column_with_no_optional_data_round_trips() {
        // The other fixture is deliberately maximal; this is the minimal one,
        // where every `Option` is `None` and every container is single-valued.
        let shape = shape();
        let col = Column {
            cx: 0,
            cz: 0,
            sections: (0..shape.section_count())
                .map(|_| Section {
                    non_empty: 0,
                    states: Container::single(0),
                    biomes: Container::single(0),
                    block_light: None,
                    sky_light: None,
                    overrides: HashMap::new(),
                })
                .collect(),
            sky_full_above: 0,
            motion_blocking: None,
            block_entities: Vec::new(),
        };
        let bytes = encode_column(&col, &shape);
        assert_columns_equal(&col, &decode_column(&bytes, (0, 0), &shape).unwrap());
    }

    #[test]
    fn bit_packing_round_trips_at_every_width_including_the_extremes() {
        for &max in &[0u32, 1, 2, 15, 16, 65535, u32::MAX] {
            // Spread across 0..=max in u64 so `max + 1` cannot wrap, and end on
            // `max` itself so the derived width is the one being tested.
            let span = max as u64 + 1;
            let mut values: Vec<u32> = (0..1000u64)
                .map(|i| (i.wrapping_mul(2_654_435_761) % span) as u32)
                .collect();
            values.push(max);
            let mut out = Vec::new();
            put_packed(&mut out, &values);
            let mut r = ByteReader::new(&out);
            assert_eq!(read_packed(&mut r).unwrap(), values, "max {max}");
            assert!(r.is_empty(), "max {max}: consumed exactly");
        }
    }

    #[test]
    fn an_empty_packed_run_round_trips_rather_than_dividing_by_zero() {
        let mut out = Vec::new();
        put_packed(&mut out, &[]);
        let mut r = ByteReader::new(&out);
        assert!(read_packed(&mut r).unwrap().is_empty());
    }

    /// Decode and require rejection, yielding the reason.
    ///
    /// `Column` implements neither `Debug` nor `PartialEq`, and deriving them
    /// on a public type purely to serve `assert_eq!` here would be the test
    /// shaping the code — so the rejection tests compare errors, and a
    /// wrongly-accepted file fails loudly on the `Ok` arm.
    fn reject(bytes: &[u8], coords: (i32, i32), shape: &DimensionShape) -> CacheError {
        match decode_column(bytes, coords, shape) {
            Ok(_) => panic!("expected a rejection, got a decoded column"),
            Err(e) => e,
        }
    }

    // -- version rejection ------------------------------------------------

    #[test]
    fn a_cache_written_by_an_older_build_is_rejected_not_misread() {
        let mut bytes = encode_column(&rich_column(), &shape());
        bytes[4..8].copy_from_slice(&(FORMAT_VERSION - 1).to_le_bytes());
        assert_eq!(
            reject(&bytes, (-3, 17), &shape()),
            CacheError::Version {
                found: FORMAT_VERSION - 1,
                expected: FORMAT_VERSION,
            }
        );
    }

    #[test]
    fn a_cache_written_by_a_newer_build_is_rejected_too() {
        // A downgrade must not read a format it has never seen; the check is
        // equality, not a minimum.
        let mut bytes = encode_column(&rich_column(), &shape());
        bytes[4..8].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert_eq!(
            reject(&bytes, (-3, 17), &shape()),
            CacheError::Version {
                found: FORMAT_VERSION + 1,
                expected: FORMAT_VERSION,
            }
        );
    }

    #[test]
    fn a_file_that_is_not_a_cache_entry_is_rejected_by_magic() {
        let mut bytes = encode_column(&rich_column(), &shape());
        bytes[0] = b'X';
        assert_eq!(reject(&bytes, (-3, 17), &shape()), CacheError::BadMagic);
        assert_eq!(reject(b"", (0, 0), &shape()), CacheError::Truncated);
    }

    // -- wrong-world rejection -------------------------------------------

    #[test]
    fn a_column_cached_at_other_coordinates_is_rejected() {
        let bytes = encode_column(&rich_column(), &shape());
        assert_eq!(
            reject(&bytes, (0, 0), &shape()),
            CacheError::WrongCoords {
                found: (-3, 17),
                expected: (0, 0),
            }
        );
    }

    #[test]
    fn a_column_cached_for_another_dimension_shape_is_rejected() {
        // The Nether is 0..256 and the Overworld -64..384; a column decoded
        // against the wrong one is terrain, just not this world's terrain.
        let bytes = encode_column(&rich_column(), &DimensionShape::NETHER);
        assert_eq!(
            reject(&bytes, (-3, 17), &DimensionShape::OVERWORLD),
            CacheError::WrongShape {
                found: DimensionShape::NETHER,
                expected: DimensionShape::OVERWORLD,
            }
        );
    }

    #[test]
    fn a_body_whose_section_count_disagrees_with_the_shape_is_rejected() {
        // Same shape in the header, wrong number of sections in the body —
        // the redundancy is the check.
        let col = rich_column();
        let bytes = encode_column(&col, &shape());
        let body = &bytes[HEADER_LEN..];
        let mut bad_body = body.to_vec();
        bad_body[0..4].copy_from_slice(&3u32.to_le_bytes()); // shape says 2
        let mut out = bytes[..HEADER_LEN].to_vec();
        out[28..36].copy_from_slice(&body_hash(&bad_body).to_le_bytes());
        out.extend_from_slice(&bad_body);
        assert_eq!(
            reject(&out, (-3, 17), &shape()),
            CacheError::Malformed("section count vs dimension shape")
        );
    }

    // -- corruption -------------------------------------------------------

    #[test]
    fn a_truncated_file_is_rejected_at_every_length_and_never_panics() {
        let bytes = encode_column(&rich_column(), &shape());
        for cut in 0..bytes.len() {
            // The prefix of a valid file can only fail for a structural reason:
            // it must never be accepted, and must never be reported as a
            // version or coordinate mismatch, which would mislead a caller.
            let err = reject(&bytes[..cut], (-3, 17), &shape());
            assert!(
                matches!(err, CacheError::Truncated | CacheError::Corrupt),
                "cut {cut}: {err:?}"
            );
        }
    }

    #[test]
    fn a_single_flipped_body_byte_is_caught_by_the_hash() {
        let bytes = encode_column(&rich_column(), &shape());
        for i in [HEADER_LEN, HEADER_LEN + 1, bytes.len() - 1] {
            let mut bad = bytes.clone();
            bad[i] ^= 0x01;
            assert_eq!(
                reject(&bad, (-3, 17), &shape()),
                CacheError::Corrupt,
                "flip at {i}"
            );
        }
    }

    #[test]
    fn trailing_bytes_after_the_body_are_rejected() {
        let mut bytes = encode_column(&rich_column(), &shape());
        bytes.push(0);
        assert_eq!(reject(&bytes, (-3, 17), &shape()), CacheError::Truncated);
    }

    #[test]
    fn a_corrupt_length_field_cannot_make_the_decoder_allocate_wildly() {
        // A body length of 4 GiB with 200 bytes behind it: the bound is the
        // bytes actually present, so this fails immediately rather than after
        // an allocation the machine cannot serve.
        let mut bytes = encode_column(&rich_column(), &shape());
        bytes[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(reject(&bytes, (-3, 17), &shape()), CacheError::Truncated);
    }

    #[test]
    fn a_body_full_of_random_bytes_is_rejected_rather_than_panicking() {
        // Not a hash test — the hash is recomputed so the body is *consistent*
        // and simply meaningless. This is the decoder's own bounds checking.
        let mut lcg: u64 = 0x9E37_79B9_7F4A_7C15;
        for len in [1usize, 7, 64, 512, 4096] {
            for _ in 0..64 {
                let body: Vec<u8> = (0..len)
                    .map(|_| {
                        lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1);
                        (lcg >> 33) as u8
                    })
                    .collect();
                let mut out = Vec::new();
                out.extend_from_slice(&MAGIC);
                put_u32(&mut out, FORMAT_VERSION);
                put_i32(&mut out, 0);
                put_i32(&mut out, 0);
                put_i32(&mut out, shape().min_y);
                put_i32(&mut out, shape().height);
                put_u32(&mut out, body.len() as u32);
                put_u64(&mut out, body_hash(&body));
                out.extend_from_slice(&body);
                // Whatever it decides, it must decide it without unwinding.
                let _ = decode_column(&out, (0, 0), &shape());
            }
        }
    }

    // -- the store --------------------------------------------------------

    #[test]
    fn a_stored_column_reads_back_and_a_missing_one_is_a_plain_miss() {
        let dir = tempdir("roundtrip");
        let mut cache = ChunkCache::open(&dir, 64 * 1024 * 1024).unwrap();
        let col = rich_column();
        assert!(cache.get(col.cx, col.cz, &shape()).is_none(), "starts empty");
        cache.put(&col, &shape()).unwrap();
        let back = cache.get(col.cx, col.cz, &shape()).expect("hit");
        assert_columns_equal(&col, &back);
        assert!(cache.get(999, 999, &shape()).is_none(), "unstored key misses");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_column_stored_for_one_shape_is_not_served_to_another() {
        let dir = tempdir("shape");
        let mut cache = ChunkCache::open(&dir, 64 * 1024 * 1024).unwrap();
        cache.put(&rich_column(), &shape()).unwrap();
        assert!(
            cache.get(-3, 17, &DimensionShape::OVERWORLD).is_none(),
            "a shape mismatch must be a miss, not other terrain"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_a_miss_and_is_dropped_rather_than_holding_budget() {
        let dir = tempdir("corrupt");
        let mut cache = ChunkCache::open(&dir, 64 * 1024 * 1024).unwrap();
        cache.put(&rich_column(), &shape()).unwrap();
        let path = dir.join(entry_name(-3, 17));
        let mut bytes = fs::read(&path).unwrap();
        bytes[HEADER_LEN + 3] ^= 0xFF;
        fs::write(&path, &bytes).unwrap();

        assert!(cache.get(-3, 17, &shape()).is_none(), "no panic, no hit");
        assert_eq!(cache.len(), 0, "the bad entry is forgotten");
        assert_eq!(cache.total_bytes(), 0, "and its bytes are released");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_deleted_behind_the_cache_s_back_is_a_miss_not_a_panic() {
        let dir = tempdir("vanished");
        let mut cache = ChunkCache::open(&dir, 64 * 1024 * 1024).unwrap();
        cache.put(&rich_column(), &shape()).unwrap();
        fs::remove_file(dir.join(entry_name(-3, 17))).unwrap();
        assert!(cache.get(-3, 17, &shape()).is_none());
        assert_eq!(cache.len(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A column small enough that a handful fit in a readable budget.
    fn small_column(cx: i32, cz: i32) -> Column {
        let shape = shape();
        let mut col = Column::empty_lit(&shape, cx, cz);
        col.set_block(&shape, 1, 1, 1, 42);
        col
    }

    #[test]
    fn eviction_drops_the_least_recently_used_entry_first() {
        let dir = tempdir("lru");
        let one = encode_column(&small_column(0, 0), &shape()).len() as u64;
        // Room for exactly three.
        let mut cache = ChunkCache::open(&dir, one * 3).unwrap();

        for i in 0..3 {
            cache.put(&small_column(i, 0), &shape()).unwrap();
        }
        assert_eq!(cache.len(), 3);

        // Touch 0, leaving 1 as the oldest use.
        assert!(cache.get(0, 0, &shape()).is_some());
        cache.put(&small_column(3, 0), &shape()).unwrap();

        assert_eq!(cache.len(), 3, "still within budget");
        assert!(!cache.contains(1, 0), "the least recently used went");
        for i in [0, 2, 3] {
            assert!(cache.contains(i, 0), "({i}, 0) should have survived");
        }
        assert!(
            !dir.join(entry_name(1, 0)).exists(),
            "eviction removes the file, not just the index entry"
        );
        assert!(cache.total_bytes() <= cache.max_bytes());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn re_storing_a_key_replaces_it_rather_than_double_counting_its_bytes() {
        let dir = tempdir("replace");
        let mut cache = ChunkCache::open(&dir, 64 * 1024 * 1024).unwrap();
        cache.put(&small_column(2, 2), &shape()).unwrap();
        let after_one = cache.total_bytes();
        cache.put(&small_column(2, 2), &shape()).unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.total_bytes(), after_one);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_budget_holds_across_many_writes() {
        let dir = tempdir("budget");
        let one = encode_column(&small_column(0, 0), &shape()).len() as u64;
        let mut cache = ChunkCache::open(&dir, one * 4 + one / 2).unwrap();
        for i in 0..40 {
            cache.put(&small_column(i, i * 2), &shape()).unwrap();
            assert!(
                cache.total_bytes() <= cache.max_bytes(),
                "over budget after {i} writes"
            );
        }
        assert_eq!(cache.len(), 4);
        // And the survivors are the four most recent.
        for i in 36..40 {
            assert!(cache.contains(i, i * 2), "({i}) should be resident");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_budget_of_zero_stores_nothing_and_leaves_no_files_behind() {
        let dir = tempdir("zero");
        let mut cache = ChunkCache::open(&dir, 0).unwrap();
        cache.put(&small_column(0, 0), &shape()).unwrap();
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.total_bytes(), 0);
        assert!(!dir.join(entry_name(0, 0)).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reopening_indexes_what_is_on_disk_and_enforces_a_shrunken_budget() {
        let dir = tempdir("reopen");
        let one = encode_column(&small_column(0, 0), &shape()).len() as u64;
        {
            let mut cache = ChunkCache::open(&dir, one * 8).unwrap();
            for i in 0..4 {
                cache.put(&small_column(i, 0), &shape()).unwrap();
            }
            assert_eq!(cache.len(), 4);
        }
        {
            let mut cache = ChunkCache::open(&dir, one * 8).unwrap();
            assert_eq!(cache.len(), 4, "index rebuilt from the directory");
            assert!(cache.get(2, 0, &shape()).is_some(), "and the entries read");
        }
        {
            // The budget shrank between runs: opening enforces it immediately
            // rather than waiting for the next write.
            let cache = ChunkCache::open(&dir, one * 2).unwrap();
            assert_eq!(cache.len(), 2);
            assert!(cache.total_bytes() <= cache.max_bytes());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unrecognised_files_in_the_cache_directory_are_left_alone() {
        let dir = tempdir("foreign");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("notes.txt"), b"the user's, not ours").unwrap();
        fs::write(dir.join("c.bogus.rwc"), b"unparseable name").unwrap();
        let cache = ChunkCache::open(&dir, 1024 * 1024).unwrap();
        assert_eq!(cache.len(), 0, "neither is indexed");
        assert!(dir.join("notes.txt").exists(), "and neither is deleted");
        assert!(dir.join("c.bogus.rwc").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    // -- naming -----------------------------------------------------------

    #[test]
    fn entry_names_round_trip_including_negative_coordinates() {
        for (cx, cz) in [(0, 0), (-1, -1), (i32::MIN, i32::MAX), (17, -3)] {
            assert_eq!(parse_entry_name(&entry_name(cx, cz)), Some((cx, cz)));
        }
        assert_eq!(parse_entry_name("c.1.rwc"), None);
        assert_eq!(parse_entry_name("c.a.b.rwc"), None);
        assert_eq!(parse_entry_name("1.2.rwc"), None);
        assert_eq!(parse_entry_name("c.1.2.txt"), None);
    }

    #[test]
    fn a_world_key_cannot_escape_its_directory() {
        // A server address is not under this client's control, so the only
        // characters that survive are ones that cannot be a path traversal or
        // a Windows-reserved separator.
        let key = world_key("../../etc:1234", "minecraft:the_nether");
        assert_eq!(key, ".._.._etc_1234.minecraft_the_nether");
        assert!(!key.contains('/') && !key.contains('\\') && !key.contains(':'));
        assert_eq!(Path::new(&key).components().count(), 1);
        // Because `.` survives the substitution, the relative directory names
        // are reachable — but only when the first part is empty, since
        // otherwise a separator is appended and `..` becomes `...`.
        assert_eq!(world_key("", ""), "unknown");
        assert_eq!(world_key("", ".."), "unknown");
        assert_eq!(world_key("", "."), "unknown");
        assert_eq!(world_key("..", ""), "...", "not traversal, so kept as-is");

        // Whatever the inputs, the result names one directory inside the root.
        for (a, b) in [
            ("", ""),
            ("", ".."),
            ("", "."),
            ("..", ""),
            ("..", ".."),
            ("/", "/"),
            ("\\", "C:"),
            ("play.example.com:25565", "minecraft:overworld"),
        ] {
            let k = world_key(a, b);
            assert!(
                matches!(
                    Path::new(&k).components().collect::<Vec<_>>().as_slice(),
                    [std::path::Component::Normal(_)]
                ),
                "({a:?}, {b:?}) produced {k:?}"
            );
        }
    }
}
