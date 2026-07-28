//! Zero-copy packet reader over a decoded frame's bytes.

use crate::varint::{read_varint, read_varlong};
use crate::{nbt::Nbt, ProtoError, Result};

pub struct PacketReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> PacketReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// The read cursor, in bytes from the frame's start.
    ///
    /// Exposed so a caller can bracket a value whose length it does not know
    /// in advance — the component walk reads one value of an arbitrary codec
    /// and then takes the span it covered (M41).
    pub fn offset(&self) -> usize {
        self.pos
    }

    /// The bytes between two offsets from [`Self::offset`]. An inverted or
    /// out-of-range pair yields an empty slice rather than panicking.
    /// Rewind to an offset taken from [`Self::offset`].
    ///
    /// Only for a **peek**: read a discriminator, and put the cursor back so a
    /// generic reader can consume the whole structure from its start. Clamped,
    /// so a bad offset cannot move the cursor past the end.
    pub fn rewind_to(&mut self, offset: usize) {
        self.pos = offset.min(self.buf.len());
    }

    pub fn bytes_between(&self, from: usize, to: usize) -> &'a [u8] {
        if from > to || to > self.buf.len() {
            return &[];
        }
        &self.buf[from..to]
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(ProtoError::Eof {
                needed: n,
                remaining: self.remaining(),
            });
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }

    /// Everything left in the packet (e.g. plugin-message payloads).
    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.buf[self.pos..];
        self.pos = self.buf.len();
        s
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(self.i64()? as u64)
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn varint(&mut self) -> Result<i32> {
        read_varint(self.buf, &mut self.pos)
    }

    pub fn varlong(&mut self) -> Result<i64> {
        read_varlong(self.buf, &mut self.pos)
    }

    /// VarInt-length-prefixed UTF-8 string. `max_chars` mirrors the vanilla
    /// per-field limit; enforced loosely as `max_chars * 3` bytes.
    pub fn string(&mut self, max_chars: usize) -> Result<String> {
        let len = self.varint()?;
        if len < 0 || len as usize > max_chars.saturating_mul(3) {
            return Err(ProtoError::LengthOutOfRange {
                what: "string",
                len: len as i64,
                max: max_chars * 3,
            });
        }
        let bytes = self.take(len as usize)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ProtoError::InvalidUtf8)
    }

    /// Resource locations ("minecraft:overworld") — 32767-char limit.
    pub fn identifier(&mut self) -> Result<String> {
        self.string(32767)
    }

    pub fn uuid(&mut self) -> Result<u128> {
        let hi = self.u64()? as u128;
        let lo = self.u64()? as u128;
        Ok((hi << 64) | lo)
    }

    /// Packed block position (wire: x 26 bits | z 26 bits | y 12 bits,
    /// all signed). Returns (x, y, z).
    pub fn position(&mut self) -> Result<(i32, i32, i32)> {
        fn sext(val: u64, bits: u32) -> i32 {
            let shift = 64 - bits;
            (((val << shift) as i64) >> shift) as i32
        }
        let v = self.i64()? as u64;
        let x = sext(v >> 38, 26);
        let z = sext(v >> 12, 26);
        let y = sext(v, 12);
        Ok((x, y, z))
    }

    /// VarInt count guarded against the remaining buffer (each element is at
    /// least `min_elem_bytes`), so hostile counts can't OOM us.
    pub fn count(&mut self, what: &'static str, min_elem_bytes: usize) -> Result<usize> {
        let n = self.varint()?;
        let max = self.remaining() / min_elem_bytes.max(1) + 1;
        if n < 0 || n as usize > max {
            return Err(ProtoError::LengthOutOfRange {
                what,
                len: n as i64,
                max,
            });
        }
        Ok(n as usize)
    }

    /// VarInt-prefixed byte array.
    pub fn byte_array(&mut self, max: usize) -> Result<&'a [u8]> {
        let len = self.varint()?;
        if len < 0 || len as usize > max {
            return Err(ProtoError::LengthOutOfRange {
                what: "byte array",
                len: len as i64,
                max,
            });
        }
        self.take(len as usize)
    }

    /// VarInt-prefixed i64 array (BitSets, heightmap columns).
    pub fn long_array(&mut self) -> Result<Vec<i64>> {
        let n = self.count("long array", 8)?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(self.i64()?);
        }
        Ok(out)
    }

    /// Network NBT value (unnamed root).
    pub fn nbt(&mut self) -> Result<Nbt> {
        Nbt::read_network(self)
    }

    pub fn option<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        if self.bool()? {
            Ok(Some(f(self)?))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_position(x: i32, y: i32, z: i32) -> [u8; 8] {
        let v: u64 = (((x as i64 & 0x3ff_ffff) as u64) << 38)
            | (((z as i64 & 0x3ff_ffff) as u64) << 12)
            | ((y as i64 & 0xfff) as u64);
        v.to_be_bytes()
    }

    #[test]
    fn position_roundtrip_signed() {
        for (x, y, z) in [
            (0, 0, 0),
            (100, -60, -100),
            (-1, -64, 1),
            (7_500_000, 319, -8_000_000),
        ] {
            let bytes = encode_position(x, y, z);
            let mut r = PacketReader::new(&bytes);
            assert_eq!(r.position().unwrap(), (x, y, z));
        }
    }

    #[test]
    fn string_and_eof_guards() {
        let mut out = Vec::new();
        crate::varint::write_varint(&mut out, 5);
        out.extend_from_slice(b"hello");
        let mut r = PacketReader::new(&out);
        assert_eq!(r.string(16).unwrap(), "hello");
        assert!(r.is_empty());
        assert!(r.u8().is_err());
    }
}
