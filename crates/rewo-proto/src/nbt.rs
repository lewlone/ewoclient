//! Minimal owned NBT reader — network variant (1.20.2+): the root value is
//! a bare tag byte + payload, with no root name. Enough to consume registry
//! data and flatten text components; a full writer comes when a packet
//! needs one.

use crate::reader::PacketReader;
use crate::{ProtoError, Result};

const MAX_DEPTH: u32 = 128;

#[derive(Debug, Clone, PartialEq)]
pub enum Nbt {
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(Vec<Nbt>),
    Compound(Vec<(String, Nbt)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Nbt {
    /// Read a network-NBT value: tag byte, then payload (no name).
    pub fn read_network(r: &mut PacketReader) -> Result<Nbt> {
        let tag = r.u8()?;
        read_payload(r, tag, 0)
    }

    pub fn get(&self, key: &str) -> Option<&Nbt> {
        match self {
            Nbt::Compound(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Nbt::Byte(v) => Some(*v as i64),
            Nbt::Short(v) => Some(*v as i64),
            Nbt::Int(v) => Some(*v as i64),
            Nbt::Long(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Nbt::String(s) => Some(s),
            _ => None,
        }
    }

    /// Best-effort flatten of a text component to plain text (disconnect
    /// reasons, log lines). Full styling lands in M3.
    pub fn to_plain_text(&self) -> String {
        match self {
            Nbt::String(s) => s.clone(),
            Nbt::Compound(_) => {
                let mut out = String::new();
                if let Some(t) = self.get("text").and_then(Nbt::as_str) {
                    out.push_str(t);
                }
                if let Some(key) = self.get("translate").and_then(Nbt::as_str) {
                    if out.is_empty() {
                        out.push_str(key);
                    }
                }
                if let Some(Nbt::List(extra)) = self.get("extra") {
                    for part in extra {
                        out.push_str(&part.to_plain_text());
                    }
                }
                out
            }
            Nbt::List(parts) => parts.iter().map(Nbt::to_plain_text).collect(),
            _ => String::new(),
        }
    }
}

fn read_string(r: &mut PacketReader) -> Result<String> {
    let len = r.u16()? as usize;
    let bytes = r.take(len)?;
    // Java "modified UTF-8" — treat as UTF-8 with lossy fallback; the
    // difference (surrogate pairs, encoded NUL) never matters for our uses.
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

fn read_payload(r: &mut PacketReader, tag: u8, depth: u32) -> Result<Nbt> {
    if depth > MAX_DEPTH {
        return Err(ProtoError::Nbt("depth limit exceeded".into()));
    }
    Ok(match tag {
        0 => Nbt::End,
        1 => Nbt::Byte(r.i8()?),
        2 => Nbt::Short(r.i16()?),
        3 => Nbt::Int(r.i32()?),
        4 => Nbt::Long(r.i64()?),
        5 => Nbt::Float(r.f32()?),
        6 => Nbt::Double(r.f64()?),
        7 => {
            let raw_len = r.i32()?;
            let len = checked_len(r, raw_len, 1)?;
            Nbt::ByteArray(r.take(len)?.to_vec())
        }
        8 => Nbt::String(read_string(r)?),
        9 => {
            let elem_tag = r.u8()?;
            let raw_len = r.i32()?;
            let len = checked_len(r, raw_len, 1)?;
            let mut items = Vec::with_capacity(len.min(4096));
            for _ in 0..len {
                items.push(read_payload(r, elem_tag, depth + 1)?);
            }
            Nbt::List(items)
        }
        10 => {
            let mut entries = Vec::new();
            loop {
                let child_tag = r.u8()?;
                if child_tag == 0 {
                    break;
                }
                let name = read_string(r)?;
                let value = read_payload(r, child_tag, depth + 1)?;
                entries.push((name, value));
            }
            Nbt::Compound(entries)
        }
        11 => {
            let raw_len = r.i32()?;
            let len = checked_len(r, raw_len, 4)?;
            let mut items = Vec::with_capacity(len);
            for _ in 0..len {
                items.push(r.i32()?);
            }
            Nbt::IntArray(items)
        }
        12 => {
            let raw_len = r.i32()?;
            let len = checked_len(r, raw_len, 8)?;
            let mut items = Vec::with_capacity(len);
            for _ in 0..len {
                items.push(r.i64()?);
            }
            Nbt::LongArray(items)
        }
        other => return Err(ProtoError::Nbt(format!("unknown tag {other}"))),
    })
}

/// Guard NBT array lengths against the bytes actually present.
fn checked_len(r: &PacketReader, len: i32, min_elem: usize) -> Result<usize> {
    if len < 0 || (len as usize).saturating_mul(min_elem) > r.remaining() {
        return Err(ProtoError::Nbt(format!(
            "array length {len} exceeds remaining {}",
            r.remaining()
        )));
    }
    Ok(len as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-built network NBT: compound { text: "hi", n: 7b }.
    #[test]
    fn compound_roundtrip() {
        let mut buf: Vec<u8> = vec![10]; // root tag: compound
        buf.push(8); // string tag
        buf.extend_from_slice(&(4u16).to_be_bytes());
        buf.extend_from_slice(b"text");
        buf.extend_from_slice(&(2u16).to_be_bytes());
        buf.extend_from_slice(b"hi");
        buf.push(1); // byte tag
        buf.extend_from_slice(&(1u16).to_be_bytes());
        buf.extend_from_slice(b"n");
        buf.push(7);
        buf.push(0); // end

        let mut r = PacketReader::new(&buf);
        let nbt = Nbt::read_network(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(nbt.get("text").and_then(Nbt::as_str), Some("hi"));
        assert_eq!(nbt.get("n").and_then(Nbt::as_i64), Some(7));
        assert_eq!(nbt.to_plain_text(), "hi");
    }

    #[test]
    fn hostile_array_length_rejected() {
        // int_array claiming 2^30 entries with 4 bytes present.
        let mut buf: Vec<u8> = vec![11];
        buf.extend_from_slice(&(1i32 << 30).to_be_bytes());
        buf.extend_from_slice(&[0, 0, 0, 1]);
        let mut r = PacketReader::new(&buf);
        assert!(Nbt::read_network(&mut r).is_err());
    }
}
