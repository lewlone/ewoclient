//! Minimal owned NBT reader — network variant (1.20.2+): the root value is
//! a bare tag byte + payload, with no root name. Enough to consume registry
//! data and flatten text components; a full writer comes when a packet
//! needs one.

use crate::reader::PacketReader;
use crate::{ProtoError, Result};

/// `NbtAccounter`'s `MAX_STACK_DEPTH`: compounds and lists push one level.
const MAX_DEPTH: u32 = 512;

/// `NbtAccounter.DEFAULT_NBT_QUOTA` — the heap budget vanilla's untrusted
/// network codecs (`ByteBufCodecs.TAG` / `COMPOUND_TAG`) read one tag under.
pub const DEFAULT_QUOTA: u64 = 2 * 1024 * 1024;

/// `NbtAccounter`: a per-root heap estimate, charged with vanilla's own
/// per-tag costs *before* allocating, so a small packet cannot expand into a
/// huge tree (e.g. a list claiming millions of one-byte elements).
struct Accounter {
    quota: u64,
    usage: u64,
    depth: u32,
}

impl Accounter {
    fn account(&mut self, size: u64) -> Result<()> {
        if self.usage.saturating_add(size) > self.quota {
            return Err(ProtoError::Nbt(format!(
                "tag too big: {} + {size} bytes over quota {}",
                self.usage, self.quota
            )));
        }
        self.usage += size;
        Ok(())
    }

    fn account_n(&mut self, per: u64, count: u64) -> Result<()> {
        self.account(per.saturating_mul(count))
    }

    fn push(&mut self) -> Result<()> {
        if self.depth >= MAX_DEPTH {
            return Err(ProtoError::Nbt(format!("depth > {MAX_DEPTH}")));
        }
        self.depth += 1;
        Ok(())
    }

    fn pop(&mut self) {
        self.depth -= 1;
    }
}

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
        Self::read_network_with_quota(r, DEFAULT_QUOTA)
    }

    /// [`Self::read_network`] under an explicit `NbtAccounter` quota.
    pub fn read_network_with_quota(r: &mut PacketReader, quota: u64) -> Result<Nbt> {
        let tag = r.u8()?;
        if tag == 0 {
            return Ok(Nbt::End);
        }
        let mut acc = Accounter { quota, usage: 0, depth: 0 };
        read_payload(r, tag, &mut acc)
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

/// The fewest wire bytes one payload of `tag` can occupy — the floor a list's
/// claimed count is checked against before anything is allocated.
fn min_payload_bytes(tag: u8) -> usize {
    match tag {
        0 => 0,
        1 => 1,
        2 => 2,
        3 | 5 => 4,
        4 | 6 => 8,
        7 | 11 | 12 => 4,
        8 => 2,
        9 => 5,
        _ => 1,
    }
}

fn read_payload(r: &mut PacketReader, tag: u8, acc: &mut Accounter) -> Result<Nbt> {
    Ok(match tag {
        0 => {
            acc.account(8)?;
            Nbt::End
        }
        1 => {
            acc.account(9)?;
            Nbt::Byte(r.i8()?)
        }
        2 => {
            acc.account(10)?;
            Nbt::Short(r.i16()?)
        }
        3 => {
            acc.account(12)?;
            Nbt::Int(r.i32()?)
        }
        4 => {
            acc.account(16)?;
            Nbt::Long(r.i64()?)
        }
        5 => {
            acc.account(12)?;
            Nbt::Float(r.f32()?)
        }
        6 => {
            acc.account(16)?;
            Nbt::Double(r.f64()?)
        }
        7 => {
            acc.account(24)?;
            let raw_len = r.i32()?;
            let len = checked_len(r, raw_len, 1)?;
            acc.account_n(1, len as u64)?;
            Nbt::ByteArray(r.take(len)?.to_vec())
        }
        8 => {
            acc.account(36)?;
            let s = read_string(r)?;
            acc.account_n(2, utf16_len(&s))?;
            Nbt::String(s)
        }
        9 => {
            acc.push()?;
            let list = read_list(r, acc);
            acc.pop();
            list?
        }
        10 => {
            acc.push()?;
            let compound = read_compound(r, acc);
            acc.pop();
            compound?
        }
        11 => {
            acc.account(24)?;
            let raw_len = r.i32()?;
            let len = checked_len(r, raw_len, 4)?;
            acc.account_n(4, len as u64)?;
            let mut items = Vec::with_capacity(len);
            for _ in 0..len {
                items.push(r.i32()?);
            }
            Nbt::IntArray(items)
        }
        12 => {
            acc.account(24)?;
            let raw_len = r.i32()?;
            let len = checked_len(r, raw_len, 8)?;
            acc.account_n(8, len as u64)?;
            let mut items = Vec::with_capacity(len);
            for _ in 0..len {
                items.push(r.i64()?);
            }
            Nbt::LongArray(items)
        }
        other => return Err(ProtoError::Nbt(format!("unknown tag {other}"))),
    })
}

/// `ListTag.loadList`.
fn read_list(r: &mut PacketReader, acc: &mut Accounter) -> Result<Nbt> {
    acc.account(36)?;
    let elem_tag = r.u8()?;
    let raw_len = r.i32()?;
    // `if (typeId == 0 && count > 0) throw "Missing type on ListTag"` — without
    // it a list of End elements costs no wire bytes per element.
    if elem_tag == 0 && raw_len > 0 {
        return Err(ProtoError::Nbt("missing type on list".into()));
    }
    let len = checked_len(r, raw_len, min_payload_bytes(elem_tag))?;
    acc.account_n(4, len as u64)?;
    let mut items = Vec::with_capacity(len.min(4096));
    for _ in 0..len {
        items.push(unwrap_list_element(read_payload(r, elem_tag, acc)?));
    }
    Ok(Nbt::List(items))
}

/// `CompoundTag.loadCompound`.
fn read_compound(r: &mut PacketReader, acc: &mut Accounter) -> Result<Nbt> {
    acc.account(48)?;
    let mut entries = Vec::new();
    loop {
        let child_tag = r.u8()?;
        if child_tag == 0 {
            break;
        }
        let name = read_string(r)?;
        acc.account(28)?;
        acc.account_n(2, utf16_len(&name))?;
        let value = read_payload(r, child_tag, acc)?;
        // Vanilla charges this only for a new key; always charging is the
        // cheap, conservative choice (duplicate keys are malformed anyway).
        acc.account(36)?;
        entries.push((name, value));
    }
    Ok(Nbt::Compound(entries))
}

/// Java `String.length()`.
fn utf16_len(s: &str) -> u64 {
    if s.is_ascii() {
        s.len() as u64
    } else {
        s.encode_utf16().count() as u64
    }
}

// Guard NBT array lengths against the bytes actually present.
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

    /// `ListTag.loadList`'s "Missing type on ListTag": an End-typed list with
    /// a positive count costs no wire bytes per element, so without this a
    /// few bytes could claim billions of elements.
    #[test]
    fn an_end_typed_list_with_elements_is_rejected() {
        let mut buf = vec![9u8, 0];
        buf.extend_from_slice(&(i32::MAX).to_be_bytes());
        assert!(Nbt::read_network(&mut PacketReader::new(&buf)).is_err());
        // An empty End-typed list is how vanilla writes `[]`, and is fine.
        let buf = wire_list(0, &[]);
        assert_eq!(
            Nbt::read_network(&mut PacketReader::new(&buf)).unwrap(),
            Nbt::List(vec![])
        );
    }

    /// The `NbtAccounter` quota: a list of one-byte tags costs 9 + 4 = 13
    /// accounted bytes per element, so ~200k elements fit in well under the
    /// 8 MiB packet cap on the wire yet exceed the 2 MiB quota.
    #[test]
    fn the_default_quota_bounds_a_wide_list() {
        let n = 200_000usize;
        let mut buf = vec![9u8, 1];
        buf.extend_from_slice(&(n as i32).to_be_bytes());
        buf.extend(std::iter::repeat_n(0u8, n));
        assert!(Nbt::read_network(&mut PacketReader::new(&buf)).is_err());
        // The same list reads under a larger explicit quota.
        let big = Nbt::read_network_with_quota(&mut PacketReader::new(&buf), 64 << 20).unwrap();
        assert!(matches!(big, Nbt::List(v) if v.len() == n));
        // And a modest one reads under the default.
        let small = wire_list(3, &vec![int_payload(1); 1000]);
        assert!(Nbt::read_network(&mut PacketReader::new(&small)).is_ok());
    }

    #[test]
    fn nesting_past_512_is_rejected() {
        // 513 nested lists of lists.
        let mut deep = Vec::new();
        deep.push(9u8);
        for _ in 0..512 {
            deep.push(9);
            deep.extend_from_slice(&1i32.to_be_bytes());
        }
        deep.push(0);
        deep.extend_from_slice(&0i32.to_be_bytes());
        assert!(Nbt::read_network(&mut PacketReader::new(&deep)).is_err());
        // 512 levels read fine.
        let mut ok = vec![9u8];
        for _ in 0..511 {
            ok.push(9);
            ok.extend_from_slice(&1i32.to_be_bytes());
        }
        ok.push(0);
        ok.extend_from_slice(&0i32.to_be_bytes());
        assert!(Nbt::read_network(&mut PacketReader::new(&ok)).is_ok());
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
