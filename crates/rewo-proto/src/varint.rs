//! VarInt / VarLong: 7-bit little-endian groups, continuation bit 0x80.
//! VarInt is at most 5 bytes (i32 as u32), VarLong at most 10 (i64 as u64).

use std::io::Read;

use crate::{ProtoError, Result};

/// Encoded byte length of a VarInt.
pub fn varint_len(v: i32) -> usize {
    let mut x = v as u32;
    let mut n = 1;
    while x >= 0x80 {
        x >>= 7;
        n += 1;
    }
    n
}

pub fn write_varint(out: &mut Vec<u8>, v: i32) {
    let mut x = v as u32;
    loop {
        if x < 0x80 {
            out.push(x as u8);
            return;
        }
        out.push((x as u8 & 0x7f) | 0x80);
        x >>= 7;
    }
}

pub fn write_varlong(out: &mut Vec<u8>, v: i64) {
    let mut x = v as u64;
    loop {
        if x < 0x80 {
            out.push(x as u8);
            return;
        }
        out.push((x as u8 & 0x7f) | 0x80);
        x >>= 7;
    }
}

/// Read a VarInt from a byte slice, advancing `pos`.
pub fn read_varint(buf: &[u8], pos: &mut usize) -> Result<i32> {
    let mut value: u32 = 0;
    for i in 0..5 {
        let Some(&byte) = buf.get(*pos) else {
            return Err(ProtoError::Eof {
                needed: 1,
                remaining: 0,
            });
        };
        *pos += 1;
        value |= ((byte & 0x7f) as u32) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(value as i32);
        }
    }
    Err(ProtoError::VarIntTooLong)
}

pub fn read_varlong(buf: &[u8], pos: &mut usize) -> Result<i64> {
    let mut value: u64 = 0;
    for i in 0..10 {
        let Some(&byte) = buf.get(*pos) else {
            return Err(ProtoError::Eof {
                needed: 1,
                remaining: 0,
            });
        };
        *pos += 1;
        value |= ((byte & 0x7f) as u64) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(value as i64);
        }
    }
    Err(ProtoError::VarIntTooLong)
}

/// Read a VarInt byte-at-a-time from a blocking reader (frame length prefix).
pub fn read_varint_io(r: &mut impl Read) -> Result<i32> {
    let mut value: u32 = 0;
    for i in 0..5 {
        let mut byte = [0u8; 1];
        r.read_exact(&mut byte)?;
        value |= ((byte[0] & 0x7f) as u32) << (7 * i);
        if byte[0] & 0x80 == 0 {
            return Ok(value as i32);
        }
    }
    Err(ProtoError::VarIntTooLong)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical VarInt vectors from the community protocol docs.
    const VECTORS: &[(i32, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (2, &[0x02]),
        (127, &[0x7f]),
        (128, &[0x80, 0x01]),
        (255, &[0xff, 0x01]),
        (25565, &[0xdd, 0xc7, 0x01]),
        (2097151, &[0xff, 0xff, 0x7f]),
        (2147483647, &[0xff, 0xff, 0xff, 0xff, 0x07]),
        (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
        (-2147483648, &[0x80, 0x80, 0x80, 0x80, 0x08]),
    ];

    #[test]
    fn varint_roundtrip_canonical() {
        for &(value, bytes) in VECTORS {
            let mut out = Vec::new();
            write_varint(&mut out, value);
            assert_eq!(out, bytes, "encode {value}");
            assert_eq!(varint_len(value), bytes.len(), "len {value}");
            let mut pos = 0;
            assert_eq!(read_varint(bytes, &mut pos).unwrap(), value, "decode {value}");
            assert_eq!(pos, bytes.len());
        }
    }

    #[test]
    fn varlong_roundtrip() {
        for v in [0i64, 1, 127, 128, 255, i64::MAX, -1, i64::MIN, 1234567890123] {
            let mut out = Vec::new();
            write_varlong(&mut out, v);
            let mut pos = 0;
            assert_eq!(read_varlong(&out, &mut pos).unwrap(), v);
            assert_eq!(pos, out.len());
        }
    }

    #[test]
    fn varint_io_matches_slice() {
        for &(value, bytes) in VECTORS {
            let mut cursor = std::io::Cursor::new(bytes);
            assert_eq!(read_varint_io(&mut cursor).unwrap(), value);
        }
    }

    #[test]
    fn overlong_varint_rejected() {
        let bytes = [0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
        let mut pos = 0;
        assert!(matches!(
            read_varint(&bytes, &mut pos),
            Err(ProtoError::VarIntTooLong)
        ));
    }
}
