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

/// `ListTag.addAndUnwrap` — undo the wrapper a **heterogeneous** list is
/// written with.
///
/// An NBT list is homogeneous by construction: the payload is one element-type
/// byte and then that many bodies. A `ListTag` holding mixed types therefore
/// cannot be written as itself, and vanilla's answer is
/// `identifyRawElementType`, which returns the shared type if there is one and
/// otherwise **10** (`TAG_Compound`) — at which point `wrapIfNeeded` boxes
/// every element that is not already a plain compound as `{"": value}`.
/// `ListTag.load` unwraps each element back on the way in, which is what makes
/// the round trip exact.
///
/// **A reader that skips this does not fail; it produces a plausible wrong
/// tree.** The list is still a list and the elements are still tags, so
/// nothing errors — a consumer just sees a one-entry compound where the value
/// should be. That is how it survived from M1 to M125 unnoticed: the first
/// thing to read a heterogeneous list's non-compound element was a
/// translatable component's `with`, where a server sends
/// `[<count>, <item component>, <player component>]` and the count is the odd
/// one out. It rendered as "Gave  [Diamond Sword] to RewoLive", with the
/// number silently gone.
///
/// The `size() == 1` test is exact and load-bearing in both directions:
/// `isWrapper` is `size() == 1 && contains("")`, so a compound with an empty
/// key AND another key is a real component and is left alone, and a genuine
/// `{"": x}` element is written double-wrapped on the way out so that this
/// unwraps it back to `{"": x}` rather than to `x`.
fn unwrap_list_element(tag: Nbt) -> Nbt {
    match &tag {
        Nbt::Compound(entries) if entries.len() == 1 && entries[0].0.is_empty() => {
            match tag {
                Nbt::Compound(mut entries) => entries.remove(0).1,
                // Unreachable: the guard above already matched a compound.
                other => other,
            }
        }
        _ => tag,
    }
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
                items.push(unwrap_list_element(read_payload(r, elem_tag, depth + 1)?));
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

    // -- `ListTag.addAndUnwrap` -------------------------------------------

    /// A `TAG_List` of `elem_tag`, with each element's payload already encoded.
    fn wire_list(elem_tag: u8, payloads: &[Vec<u8>]) -> Vec<u8> {
        let mut buf = vec![9u8, elem_tag];
        buf.extend_from_slice(&(payloads.len() as i32).to_be_bytes());
        for p in payloads {
            buf.extend_from_slice(p);
        }
        buf
    }

    /// A `TAG_Compound` payload from already-encoded `(tag, name, payload)`.
    fn wire_compound(entries: &[(u8, &str, Vec<u8>)]) -> Vec<u8> {
        let mut buf = Vec::new();
        for (tag, name, payload) in entries {
            buf.push(*tag);
            buf.extend_from_slice(&(name.len() as u16).to_be_bytes());
            buf.extend_from_slice(name.as_bytes());
            buf.extend_from_slice(payload);
        }
        buf.push(0);
        buf
    }

    fn int_payload(v: i32) -> Vec<u8> {
        v.to_be_bytes().to_vec()
    }

    /// A homogeneous list is untouched — the unwrap must not fire on the
    /// common case.
    #[test]
    fn a_homogeneous_list_is_read_unchanged() {
        let buf = wire_list(3, &[int_payload(1), int_payload(2)]);
        let mut r = PacketReader::new(&buf);
        assert_eq!(
            Nbt::read_network(&mut r).unwrap(),
            Nbt::List(vec![Nbt::Int(1), Nbt::Int(2)])
        );
    }

    /// The heterogeneous case, which is the one that was silently wrong from
    /// M1 to M125. NBT lists are homogeneous, so a mixed list is written as a
    /// list of compounds with every non-compound element boxed as `{"": v}` —
    /// `ListTag.wrapIfNeeded` — and `load` unwraps each one back.
    ///
    /// Without the unwrap this decodes without error into a plausible wrong
    /// tree: element 0 reads as a one-entry compound instead of an integer,
    /// which is why nothing caught it until a translatable's `with` list held
    /// a count beside two components.
    #[test]
    fn a_wrapped_element_of_a_mixed_list_is_unwrapped() {
        let wrapped = wire_compound(&[(3, "", int_payload(1))]);
        let real = wire_compound(&[(8, "text", {
            let mut v = (2u16).to_be_bytes().to_vec();
            v.extend_from_slice(b"hi");
            v
        })]);
        let buf = wire_list(10, &[wrapped, real]);
        let mut r = PacketReader::new(&buf);
        assert_eq!(
            Nbt::read_network(&mut r).unwrap(),
            Nbt::List(vec![
                Nbt::Int(1),
                Nbt::Compound(vec![("text".into(), Nbt::String("hi".into()))]),
            ])
        );
    }

    /// `isWrapper` is `size() == 1 && contains("")`, so BOTH halves are
    /// load-bearing and each has its own way of being wrong.
    #[test]
    fn a_compound_that_is_not_a_wrapper_survives() {
        // Two entries, one of them empty-named: a real compound, not a box.
        // Unwrapping on the empty key alone would delete the other field.
        let two = wire_compound(&[(3, "", int_payload(1)), (3, "n", int_payload(2))]);
        let buf = wire_list(10, &[two]);
        let mut r = PacketReader::new(&buf);
        assert_eq!(
            Nbt::read_network(&mut r).unwrap(),
            Nbt::List(vec![Nbt::Compound(vec![
                ("".into(), Nbt::Int(1)),
                ("n".into(), Nbt::Int(2)),
            ])])
        );
        // One entry, but named: unwrapping on the count alone would strip it.
        let named = wire_compound(&[(3, "n", int_payload(1))]);
        let buf = wire_list(10, &[named]);
        let mut r = PacketReader::new(&buf);
        assert_eq!(
            Nbt::read_network(&mut r).unwrap(),
            Nbt::List(vec![Nbt::Compound(vec![("n".into(), Nbt::Int(1))])])
        );
    }

    /// A genuine `{"": x}` element is written DOUBLE-wrapped, because
    /// `wrapIfNeeded`'s test is `instanceof CompoundTag && !isWrapper(it)` —
    /// so it boxes a compound that is already a wrapper. Unwrapping exactly
    /// once is what returns the original.
    #[test]
    fn a_genuine_empty_keyed_compound_survives_one_unwrap() {
        let inner = wire_compound(&[(3, "", int_payload(5))]);
        let outer = wire_compound(&[(10, "", inner)]);
        let buf = wire_list(10, &[outer]);
        let mut r = PacketReader::new(&buf);
        assert_eq!(
            Nbt::read_network(&mut r).unwrap(),
            Nbt::List(vec![Nbt::Compound(vec![("".into(), Nbt::Int(5))])])
        );
    }

    /// The unwrap is per ELEMENT of a list, not a general compound rule: a
    /// `{"": v}` sitting as a field of a compound is not in a list and stays.
    #[test]
    fn the_unwrap_does_not_apply_outside_a_list() {
        let buf = {
            let mut b = vec![10u8];
            b.extend_from_slice(&wire_compound(&[(
                10,
                "f",
                wire_compound(&[(3, "", int_payload(1))]),
            )]));
            b
        };
        let mut r = PacketReader::new(&buf);
        assert_eq!(
            Nbt::read_network(&mut r).unwrap(),
            Nbt::Compound(vec![(
                "f".into(),
                Nbt::Compound(vec![("".into(), Nbt::Int(1))])
            )])
        );
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
