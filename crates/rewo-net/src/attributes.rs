//! `ClientboundUpdateAttributesPacket` — the wire half of entity attributes
//! (M52).
//!
//! Body, in wire order:
//!
//! ```text
//! VarInt entityId
//! VarInt snapshotCount                 // ByteBufCodecs.list(128)
//!   VarInt attributeHolder             // holderRegistry — RAW 0-based id
//!   f64    base                        // big-endian, ByteBufCodecs.DOUBLE
//!   VarInt modifierCount               // ByteBufCodecs.collection(ArrayList::new)
//!     String id                        // Identifier.STREAM_CODEC = STRING_UTF8
//!     f64    amount
//!     VarInt operation                 // idMapper — a VarInt, not a byte
//! ```
//!
//! **The attribute holder is `holderRegistry`, so the id is raw and 0-based.**
//! `Attribute.STREAM_CODEC = ByteBufCodecs.holderRegistry(Registries.ATTRIBUTE)`
//! resolves through `registry(...)`, whose decode is a bare `VarInt.read` into
//! `byIdOrThrow` — there is no `id + 1` and no `0 = inline`, which is
//! `ByteBufCodecs.holder`'s different scheme. Reading one as the other shifts
//! every attribute by one, which here would silently turn `max_health` into
//! `max_absorption`. The same distinction already cost this project the M14
//! play-login dimension holder and was called out again for the M21 damage
//! type.
//!
//! Two further details that are not guessable from the field list:
//!
//! * The **operation is a VarInt**, because `AttributeModifier.Operation
//!   .STREAM_CODEC` is `ByteBufCodecs.idMapper`, whose decode is
//!   `VarInt.read(input)` — not the single byte a three-valued enum invites.
//! * An **out-of-range operation id is not an error**: `BY_ID` is
//!   `ByIdMap.continuous(..., OutOfBoundsStrategy.ZERO)`, so it yields
//!   `ADD_VALUE`. See [`rewo_world::attributes::Operation::from_id`].

use rewo_proto::reader::PacketReader;
use rewo_world::attributes::{Modifier, Operation};

/// One `ClientboundUpdateAttributesPacket.AttributeSnapshot`.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// Raw `minecraft:attribute` registry id.
    pub attribute: i32,
    pub base: f64,
    pub modifiers: Vec<Modifier>,
}

/// A decoded `update_attributes`.
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateAttributes {
    pub entity_id: i32,
    pub snapshots: Vec<Snapshot>,
}

/// `ByteBufCodecs.list(128)` — the snapshot list's declared maximum. A larger
/// count is a `DecoderException` in vanilla, so it is a rejected packet here.
const MAX_SNAPSHOTS: usize = 128;

/// Parse an `update_attributes` body.
///
/// Returns `None` on truncation, on a snapshot count above vanilla's own 128
/// limit, or on any malformed field. The whole body is walked — a decoder that
/// stops early is a decoder that desyncs — and because each packet arrives in
/// its own buffer, a `None` leaves the caller's stream state untouched.
pub fn parse(body: &[u8]) -> Option<UpdateAttributes> {
    let mut r = PacketReader::new(body);
    let entity_id = r.varint().ok()?;

    // Guarded against the buffer: the smallest snapshot is a 1-byte holder, an
    // 8-byte double and a 1-byte count.
    let count = r.count("attribute snapshots", 10).ok()?;
    if count > MAX_SNAPSHOTS {
        return None;
    }

    let mut snapshots = Vec::with_capacity(count.min(MAX_SNAPSHOTS));
    for _ in 0..count {
        let attribute = r.varint().ok()?;
        let base = r.f64().ok()?;
        // The smallest modifier is a 1-byte empty string, an 8-byte double and
        // a 1-byte operation.
        let n = r.count("attribute modifiers", 10).ok()?;
        let mut modifiers = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            let id = r.identifier().ok()?;
            let amount = r.f64().ok()?;
            let operation = Operation::from_id(r.varint().ok()?);
            modifiers.push(Modifier {
                id,
                amount,
                operation,
            });
        }
        snapshots.push(Snapshot {
            attribute,
            base,
            modifiers,
        });
    }

    Some(UpdateAttributes {
        entity_id,
        snapshots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a body independently of any writer under test.
    fn varint(v: i32, out: &mut Vec<u8>) {
        let mut n = v as u32;
        loop {
            let b = (n & 0x7F) as u8;
            n >>= 7;
            if n == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
    }

    fn body(entity: i32, snaps: &[(i32, f64, &[(&str, f64, i32)])]) -> Vec<u8> {
        let mut out = Vec::new();
        varint(entity, &mut out);
        varint(snaps.len() as i32, &mut out);
        for (attr, base, mods) in snaps {
            varint(*attr, &mut out);
            out.extend_from_slice(&base.to_be_bytes());
            varint(mods.len() as i32, &mut out);
            for (id, amount, op) in *mods {
                varint(id.len() as i32, &mut out);
                out.extend_from_slice(id.as_bytes());
                out.extend_from_slice(&amount.to_be_bytes());
                varint(*op, &mut out);
            }
        }
        out
    }

    #[test]
    fn a_bare_snapshot_decodes() {
        let p = parse(&body(7, &[(23, 20.0, &[])])).expect("decode");
        assert_eq!(p.entity_id, 7);
        assert_eq!(p.snapshots.len(), 1);
        assert_eq!(p.snapshots[0].attribute, 23);
        assert_eq!(p.snapshots[0].base, 20.0);
        assert!(p.snapshots[0].modifiers.is_empty());
    }

    #[test]
    fn modifiers_decode_with_their_identifier_and_operation() {
        let p = parse(&body(
            1,
            &[(23, 20.0, &[("minecraft:test", 0.5, 2), ("minecraft:other", 4.0, 0)])],
        ))
        .expect("decode");
        let m = &p.snapshots[0].modifiers;
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].id, "minecraft:test");
        assert_eq!(m[0].amount, 0.5);
        assert_eq!(m[0].operation, Operation::AddMultipliedTotal);
        assert_eq!(m[1].operation, Operation::AddValue);
    }

    #[test]
    fn an_out_of_range_operation_decodes_as_add_value() {
        let p = parse(&body(1, &[(23, 20.0, &[("t:x", 1.0, 9)])])).expect("decode");
        assert_eq!(p.snapshots[0].modifiers[0].operation, Operation::AddValue);
    }

    #[test]
    fn a_truncated_body_is_rejected() {
        let full = body(1, &[(23, 20.0, &[("t:x", 1.0, 0)])]);
        for cut in 0..full.len() {
            assert!(
                parse(&full[..cut]).is_none(),
                "a body truncated to {cut} bytes must not decode"
            );
        }
        assert!(parse(&full).is_some());
    }

    #[test]
    fn more_than_128_snapshots_is_rejected() {
        // `ByteBufCodecs.list(128)` throws a DecoderException above 128, so a
        // well-formed body carrying 129 is still a rejected packet.
        let snaps: Vec<(i32, f64, &[(&str, f64, i32)])> =
            (0..129).map(|i| (i, 1.0, &[][..])).collect();
        assert!(parse(&body(1, &snaps)).is_none());
        let ok: Vec<(i32, f64, &[(&str, f64, i32)])> =
            (0..128).map(|i| (i, 1.0, &[][..])).collect();
        assert!(parse(&body(1, &ok)).is_some());
    }

    #[test]
    fn several_snapshots_keep_their_order_and_ids() {
        let p = parse(&body(3, &[(23, 20.0, &[]), (1, 5.0, &[]), (0, 1.0, &[])]))
            .expect("decode");
        let ids: Vec<i32> = p.snapshots.iter().map(|s| s.attribute).collect();
        assert_eq!(ids, vec![23, 1, 0]);
    }
}
