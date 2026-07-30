//! `ClientboundTrackedWaypointPacket` — the locator bar's wire half (M83).
//!
//! One packet, id **138**, carrying an *operation* and a **`TrackedWaypoint`**:
//! an identifier, an icon, a type tag, and a type-dependent body. The client
//! keeps a map of them and the HUD draws a dot per entry along a strip at the
//! bottom of the screen ([`rewo_gpu::locator_bar`]).
//!
//! # The four things that read backwards
//!
//! 1. **The operation is `ByIdMap.continuous(…, WRAP)`, not `readEnum`.**
//!    `Operation.BY_ID` wraps with `Mth.positiveModulo` (`Math.floorMod`), so
//!    **no id is rejected** and a *negative* one is legal — id `-1` is `UPDATE`,
//!    id `4` is `UNTRACK`. Rust's `%` is a remainder, not a modulus, so this
//!    must be `rem_euclid`. One field later, `TrackedWaypoint.Type` uses
//!    `byteBuf.readEnum`, which is `getEnumConstants()[readVarInt()]` — a bare
//!    array index that **throws** out of range. Two enums, one byte apart, with
//!    opposite out-of-range behaviour.
//!
//! 2. **The body's shape depends on that type tag**, and an untranscribed
//!    variant cannot be skipped — the reader would park mid-value. This is the
//!    `DataComponentPatch` hazard in miniature (M41), except that here the set
//!    is closed at four, so all four are transcribed and an unknown tag is a
//!    decode error rather than a silent truncation.
//!
//! 3. **An identifier is `Either<UUID, String>`**, and `FriendlyByteBuf.
//!    writeEither` writes **`true` for the *left*** — the UUID. A reader that
//!    took the flag to mean "the unusual case" reads sixteen bytes of a
//!    length-prefixed string, or a string out of a UUID.
//!
//! 4. **`UPDATE` is not an insert.** `ClientWaypointManager.updateWaypoint` is
//!    `this.waypoints.get(id).update(other)`, and each `update` override
//!    assigns **only its own position field**. The icon is `final` and the type
//!    is `final`, so an update *cannot* recolour a waypoint, restyle it, or
//!    change it from a chunk to a position — vanilla logs
//!    `"Unsupported Waypoint update operation"` and keeps the old value. An
//!    implementation that reused the `TRACK` path for `UPDATE` would apply all
//!    three and never report anything.
//!
//! Everything is transcribed from the 26.2 decompile:
//! `net/minecraft/network/protocol/game/ClientboundTrackedWaypointPacket.java`,
//! `net/minecraft/world/waypoints/TrackedWaypoint.java`,
//! `net/minecraft/client/waypoints/ClientWaypointManager.java`.

use std::collections::HashMap;

use rewo_proto::reader::PacketReader;

/// `ClientboundTrackedWaypointPacket.Operation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaypointOp {
    Track,
    Untrack,
    Update,
}

impl WaypointOp {
    /// Declaration order, which `Enum::ordinal` makes the id.
    pub const VALUES: [WaypointOp; 3] = [WaypointOp::Track, WaypointOp::Untrack, WaypointOp::Update];

    /// `ByIdMap.continuous(Enum::ordinal, values(), WRAP)` —
    /// `sortedValues[Mth.positiveModulo(id, 3)]`.
    ///
    /// Total by construction: WRAP has no rejecting branch. `rem_euclid` is
    /// the operator that matches `Math.floorMod`; `%` would panic on a
    /// negative index after the cast, and clamping would answer `UPDATE` where
    /// vanilla answers `UNTRACK`.
    pub fn by_id(id: i32) -> WaypointOp {
        let idx = id.rem_euclid(WaypointOp::VALUES.len() as i32) as usize;
        WaypointOp::VALUES[idx]
    }
}

/// `TrackedWaypoint.identifier` — `Either<UUID, String>`.
///
/// The map key, so it carries `Hash`/`Eq`. Vanilla's key is the `Either`
/// itself, which means a UUID and a string that happen to print the same are
/// *different* waypoints; the enum reproduces that for free.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WaypointId {
    /// The left alternative — every vanilla transmitter is an entity, so this
    /// is what a real server sends.
    Uuid(u128),
    /// The right alternative. Nothing in vanilla writes one; the codec allows
    /// it, so it is decoded rather than rejected.
    Name(String),
}

/// `Waypoint.Icon`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaypointIcon {
    /// `ResourceKey<WaypointStyleAsset>` — a plain `Identifier` on the wire.
    pub style: String,
    /// `Optional<Integer>` through `ByteBufCodecs.RGB_COLOR`: **three raw
    /// bytes**, not an int and not a var-int, reassembled by `ARGB.color(r, g,
    /// b)` — which is the *three*-argument overload, so alpha comes out 255.
    /// An absent colour is not black; it means "derive one from the id".
    pub color: Option<u32>,
}

/// `TrackedWaypoint.Type` plus its type-dependent body.
///
/// The tag is a var-int index into the declaration order below; `readEnum`
/// indexes the constant array directly, so anything outside `0..4` throws in
/// vanilla and is an `Err` here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WaypointContents {
    /// `EmptyWaypoint` — **no body at all**. What `removeWaypoint` builds, so
    /// an `UNTRACK` normally carries this; nothing stops a server tracking one.
    Empty,
    /// `Vec3iWaypoint` — three var-ints. A **block position**, not a float
    /// vector: the renderer centres it with `Vec3.atCenterOf`.
    Vec3i { x: i32, y: i32, z: i32 },
    /// `ChunkWaypoint` — two var-ints, x and **z** (there is no y).
    Chunk { x: i32, z: i32 },
    /// `AzimuthWaypoint` — one big-endian f32 **in radians**. The only field in
    /// the whole packet that is not a var-int, and the renderer's first act is
    /// to multiply it by `180/π`; reading it as degrees leaves every far-away
    /// player pinned within a few degrees of north.
    Azimuth { radians: f32 },
}

impl WaypointContents {
    /// The wire tag, i.e. `Type.ordinal()`.
    pub fn type_id(&self) -> i32 {
        match self {
            WaypointContents::Empty => 0,
            WaypointContents::Vec3i { .. } => 1,
            WaypointContents::Chunk { .. } => 2,
            WaypointContents::Azimuth { .. } => 3,
        }
    }

    /// Whether `TrackedWaypoint.update` would accept `other` in place of this.
    ///
    /// Vanilla's test is an `instanceof` against the *concrete* subclass, so
    /// this is type equality — and `EmptyWaypoint.update` is an empty method,
    /// which is a different thing again: it *accepts* and changes nothing.
    pub fn same_variant(&self, other: &WaypointContents) -> bool {
        self.type_id() == other.type_id()
    }
}

/// One entry of the client's waypoint map.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackedWaypoint {
    pub id: WaypointId,
    pub icon: WaypointIcon,
    pub contents: WaypointContents,
}

/// `ClientWaypointManager` — the map the HUD reads.
#[derive(Debug, Default)]
pub struct WaypointStore {
    map: HashMap<WaypointId, TrackedWaypoint>,
}

impl WaypointStore {
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// `hasWaypoints()` inverted. The locator bar is the contextual bar only
    /// while this is false.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get(&self, id: &WaypointId) -> Option<&TrackedWaypoint> {
        self.map.get(id)
    }

    /// Every entry, in **sorted id order**.
    ///
    /// Vanilla iterates a `ConcurrentHashMap.values()` and sorts it by
    /// `-distanceSquared`; the sort is stable but the source order is
    /// unspecified, so ties (two `AZIMUTH` waypoints, both at
    /// `+Infinity`) are already arbitrary in vanilla. Sorting by id here makes
    /// Rewo's arbitrary order *deterministic*, which is a strict improvement
    /// on an unspecified one and is what lets a gate assert a draw order at
    /// all. See [`rewo_gpu::locator_bar::markers`] for the distance sort that
    /// runs on top of it.
    pub fn iter_sorted(&self) -> Vec<&TrackedWaypoint> {
        let mut v: Vec<&TrackedWaypoint> = self.map.values().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// A dimension change or a disconnect. Vanilla's manager hangs off the
    /// `ClientPacketListener`, so it survives a respawn and dies with the
    /// connection; this exists for the latter.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// `ClientboundTrackedWaypointPacket.apply` — dispatch on the operation.
    pub fn apply(&mut self, op: WaypointOp, waypoint: TrackedWaypoint) {
        match op {
            // `WaypointManager::trackWaypoint` — `waypoints.put(id, waypoint)`.
            // A wholesale replace, so a re-`TRACK` is how a server changes an
            // icon or switches a waypoint between position/chunk/azimuth tiers.
            WaypointOp::Track => {
                self.map.insert(waypoint.id.clone(), waypoint);
            }
            // `WaypointManager::untrackWaypoint` — `waypoints.remove(id)`. Only
            // the identifier is read; the icon and body of an untrack are
            // ignored, which is why `removeWaypoint` can send an empty one.
            WaypointOp::Untrack => {
                self.map.remove(&waypoint.id);
            }
            // `WaypointManager::updateWaypoint` — `waypoints.get(id).update(o)`.
            //
            // Two deviations, both deliberate and both narrower than they look:
            //
            // * An id that is **not present** is an unguarded `null` deref in
            //   vanilla — `ConcurrentHashMap.get` returns null and `.update`
            //   throws. Rewo drops it. A server that sends `UPDATE` before
            //   `TRACK` disconnects a vanilla client with an NPE; reproducing
            //   that is not a feature.
            // * A **type mismatch** logs a warning and changes nothing. Rewo
            //   does the same silently.
            //
            // The load-bearing half is what an accepted update writes: the
            // position field, and *only* that. Icon and type are `final`.
            WaypointOp::Update => {
                if let Some(existing) = self.map.get_mut(&waypoint.id) {
                    if existing.contents.same_variant(&waypoint.contents) {
                        existing.contents = waypoint.contents;
                    }
                }
            }
        }
    }
}

/// `TrackedWaypoint.read` — the identifier, the icon, the type tag, the body.
pub fn read_tracked_waypoint(r: &mut PacketReader<'_>) -> Result<TrackedWaypoint, ()> {
    // `byteBuf.readEither(UUIDUtil.STREAM_CODEC, FriendlyByteBuf::readUtf)`.
    // True is the LEFT alternative — the UUID.
    let id = if r.bool().map_err(|_| ())? {
        WaypointId::Uuid(r.uuid().map_err(|_| ())?)
    } else {
        WaypointId::Name(r.string(32767).map_err(|_| ())?)
    };
    // `Waypoint.Icon.STREAM_CODEC` — a ResourceKey (an Identifier string) then
    // an optional colour.
    let style = r.identifier().map_err(|_| ())?;
    let color = r
        .option(|r| {
            let (rd, g, b) = (r.u8()?, r.u8()?, r.u8()?);
            // `ARGB.color(int red, int green, int blue)` = `color(255, r, g, b)`.
            Ok(0xFF00_0000 | ((rd as u32) << 16) | ((g as u32) << 8) | b as u32)
        })
        .map_err(|_| ())?;
    // `byteBuf.readEnum(Type.class)` — `getEnumConstants()[readVarInt()]`.
    let contents = match r.varint().map_err(|_| ())? {
        0 => WaypointContents::Empty,
        1 => {
            let (x, y, z) = (
                r.varint().map_err(|_| ())?,
                r.varint().map_err(|_| ())?,
                r.varint().map_err(|_| ())?,
            );
            WaypointContents::Vec3i { x, y, z }
        }
        2 => {
            let (x, z) = (r.varint().map_err(|_| ())?, r.varint().map_err(|_| ())?);
            WaypointContents::Chunk { x, z }
        }
        3 => WaypointContents::Azimuth {
            radians: r.f32().map_err(|_| ())?,
        },
        // Out of range: vanilla throws `ArrayIndexOutOfBoundsException` out of
        // `readEnum` and drops the connection. There is no skip — the body's
        // length is only knowable from the tag.
        _ => return Err(()),
    };
    Ok(TrackedWaypoint {
        id,
        icon: WaypointIcon { style, color },
        contents,
    })
}

/// The whole packet: `Operation.STREAM_CODEC` then `TrackedWaypoint.STREAM_CODEC`.
pub fn read_waypoint_packet(body: &[u8]) -> Result<(WaypointOp, TrackedWaypoint), ()> {
    let mut r = PacketReader::new(body);
    let op = WaypointOp::by_id(r.varint().map_err(|_| ())?);
    let waypoint = read_tracked_waypoint(&mut r)?;
    Ok((op, waypoint))
}

/// Decode and apply. Returns whether the body decoded.
pub fn apply(body: &[u8], store: &mut WaypointStore) -> bool {
    match read_waypoint_packet(body) {
        Ok((op, waypoint)) => {
            store.apply(op, waypoint);
            true
        }
        Err(()) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(v: i32, out: &mut Vec<u8>) {
        let mut v = v as u32;
        loop {
            if v & !0x7F == 0 {
                out.push(v as u8);
                return;
            }
            out.push((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
    }

    fn uuid_body(op: i32, uuid: u128, style: &str, color: Option<(u8, u8, u8)>) -> Vec<u8> {
        let mut b = Vec::new();
        varint(op, &mut b);
        b.push(1); // Either: true = left = UUID
        b.extend_from_slice(&uuid.to_be_bytes());
        varint(style.len() as i32, &mut b);
        b.extend_from_slice(style.as_bytes());
        match color {
            Some((r, g, bl)) => {
                b.push(1);
                b.extend_from_slice(&[r, g, bl]);
            }
            None => b.push(0),
        }
        b
    }

    fn vec3i(op: i32, uuid: u128, x: i32, y: i32, z: i32) -> Vec<u8> {
        let mut b = uuid_body(op, uuid, "minecraft:default", None);
        varint(1, &mut b);
        varint(x, &mut b);
        varint(y, &mut b);
        varint(z, &mut b);
        b
    }

    #[test]
    fn the_operation_wraps_and_never_rejects() {
        // `ByIdMap.continuous(…, WRAP)` = `sortedValues[Math.floorMod(id, 3)]`.
        assert_eq!(WaypointOp::by_id(0), WaypointOp::Track);
        assert_eq!(WaypointOp::by_id(1), WaypointOp::Untrack);
        assert_eq!(WaypointOp::by_id(2), WaypointOp::Update);
        assert_eq!(WaypointOp::by_id(3), WaypointOp::Track);
        assert_eq!(WaypointOp::by_id(4), WaypointOp::Untrack);
        // The half that a Rust `%` gets wrong: floorMod(-1, 3) == 2.
        assert_eq!(WaypointOp::by_id(-1), WaypointOp::Update);
        assert_eq!(WaypointOp::by_id(-3), WaypointOp::Track);
        // `Math.floorMod(Integer.MIN_VALUE, 3)` is 1, and `rem_euclid` agrees.
        // A `%` here would give -2 and index out of the array.
        assert_eq!(WaypointOp::by_id(i32::MIN), WaypointOp::Untrack);
    }

    #[test]
    fn the_identifier_flag_is_true_for_the_uuid() {
        let b = vec3i(0, 0x0123_4567_89AB_CDEF_0123_4567_89AB_CDEF, 1, 2, 3);
        let (op, w) = read_waypoint_packet(&b).unwrap();
        assert_eq!(op, WaypointOp::Track);
        assert_eq!(
            w.id,
            WaypointId::Uuid(0x0123_4567_89AB_CDEF_0123_4567_89AB_CDEF)
        );
        // And the string form, which is the same field with the flag cleared.
        let mut n = Vec::new();
        varint(0, &mut n);
        n.push(0);
        varint(5, &mut n);
        n.extend_from_slice(b"north");
        varint(17, &mut n);
        n.extend_from_slice(b"minecraft:default");
        n.push(0);
        varint(0, &mut n); // EMPTY
        let (_, w) = read_waypoint_packet(&n).unwrap();
        assert_eq!(w.id, WaypointId::Name("north".into()));
        assert_eq!(w.contents, WaypointContents::Empty);
    }

    #[test]
    fn the_colour_is_three_raw_bytes_and_opaque() {
        let mut b = uuid_body(0, 7, "minecraft:default", Some((0x12, 0x34, 0x56)));
        varint(0, &mut b);
        let (_, w) = read_waypoint_packet(&b).unwrap();
        assert_eq!(w.icon.color, Some(0xFF12_3456));
        // Absent is not black.
        let mut b = uuid_body(0, 7, "minecraft:default", None);
        varint(0, &mut b);
        assert_eq!(read_waypoint_packet(&b).unwrap().1.icon.color, None);
    }

    #[test]
    fn every_body_shape_decodes_and_an_unknown_tag_is_an_error() {
        let (_, w) = read_waypoint_packet(&vec3i(0, 1, -5, 70, 300)).unwrap();
        assert_eq!(
            w.contents,
            WaypointContents::Vec3i {
                x: -5,
                y: 70,
                z: 300
            }
        );

        let mut b = uuid_body(0, 1, "minecraft:default", None);
        varint(2, &mut b);
        varint(-3, &mut b);
        varint(9, &mut b);
        assert_eq!(
            read_waypoint_packet(&b).unwrap().1.contents,
            WaypointContents::Chunk { x: -3, z: 9 }
        );

        let mut b = uuid_body(0, 1, "minecraft:default", None);
        varint(3, &mut b);
        b.extend_from_slice(&1.25f32.to_be_bytes());
        assert_eq!(
            read_waypoint_packet(&b).unwrap().1.contents,
            WaypointContents::Azimuth { radians: 1.25 }
        );

        // `readEnum` indexes the constant array — 4 is out of bounds.
        let mut b = uuid_body(0, 1, "minecraft:default", None);
        varint(4, &mut b);
        assert!(read_waypoint_packet(&b).is_err());
    }

    #[test]
    fn update_writes_only_the_position() {
        let mut s = WaypointStore::default();
        let (_, first) = read_waypoint_packet(&vec3i(0, 1, 10, 64, 10)).unwrap();
        s.apply(WaypointOp::Track, first);
        assert_eq!(s.len(), 1);

        // A same-type UPDATE moves it…
        let mut b = uuid_body(2, 1, "minecraft:bowtie", Some((255, 0, 0)));
        varint(1, &mut b);
        varint(11, &mut b);
        varint(65, &mut b);
        varint(12, &mut b);
        let (op, upd) = read_waypoint_packet(&b).unwrap();
        assert_eq!(op, WaypointOp::Update);
        s.apply(op, upd);
        let w = s.get(&WaypointId::Uuid(1)).unwrap();
        assert_eq!(
            w.contents,
            WaypointContents::Vec3i {
                x: 11,
                y: 65,
                z: 12
            }
        );
        // …and does NOT restyle or recolour it, because both are `final`.
        assert_eq!(w.icon.style, "minecraft:default");
        assert_eq!(w.icon.color, None);

        // A cross-type UPDATE is refused whole: vanilla logs and keeps the old.
        let mut b = uuid_body(2, 1, "minecraft:default", None);
        varint(2, &mut b);
        varint(0, &mut b);
        varint(0, &mut b);
        let (op, upd) = read_waypoint_packet(&b).unwrap();
        s.apply(op, upd);
        assert_eq!(
            s.get(&WaypointId::Uuid(1)).unwrap().contents,
            WaypointContents::Vec3i {
                x: 11,
                y: 65,
                z: 12
            }
        );

        // An UPDATE for an unknown id inserts nothing (vanilla NPEs).
        let mut b = uuid_body(2, 99, "minecraft:default", None);
        varint(1, &mut b);
        varint(0, &mut b);
        varint(0, &mut b);
        varint(0, &mut b);
        let (op, upd) = read_waypoint_packet(&b).unwrap();
        s.apply(op, upd);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn untrack_reads_only_the_identifier() {
        let mut s = WaypointStore::default();
        let (op, w) = read_waypoint_packet(&vec3i(0, 42, 1, 2, 3)).unwrap();
        s.apply(op, w);
        // `removeWaypoint` builds an EMPTY waypoint with the NULL icon — the
        // body says nothing about what is being removed beyond the id.
        let mut b = uuid_body(1, 42, "minecraft:default", None);
        varint(0, &mut b);
        let (op, w) = read_waypoint_packet(&b).unwrap();
        assert_eq!(op, WaypointOp::Untrack);
        assert_eq!(w.contents, WaypointContents::Empty);
        s.apply(op, w);
        assert!(s.is_empty());
    }

    #[test]
    fn a_uuid_and_a_string_are_different_keys() {
        let mut s = WaypointStore::default();
        s.apply(
            WaypointOp::Track,
            TrackedWaypoint {
                id: WaypointId::Uuid(1),
                icon: WaypointIcon {
                    style: "minecraft:default".into(),
                    color: None,
                },
                contents: WaypointContents::Empty,
            },
        );
        s.apply(
            WaypointOp::Track,
            TrackedWaypoint {
                id: WaypointId::Name("1".into()),
                icon: WaypointIcon {
                    style: "minecraft:default".into(),
                    color: None,
                },
                contents: WaypointContents::Empty,
            },
        );
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn a_truncated_body_is_an_error_not_a_partial_apply() {
        let full = vec3i(0, 1, 10, 64, 10);
        for cut in 0..full.len() {
            let mut s = WaypointStore::default();
            assert!(!apply(&full[..cut], &mut s) || s.len() <= 1);
            if !apply(&full[..cut], &mut s) {
                assert!(s.is_empty(), "a failed decode wrote a waypoint");
            }
        }
        let mut s = WaypointStore::default();
        assert!(apply(&full, &mut s));
        assert_eq!(s.len(), 1);
    }
}
