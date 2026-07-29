//! The server's view area (M67): `set_chunk_cache_center`,
//! `set_chunk_cache_radius` and `set_simulation_distance`.
//!
//! **Decode and state only.** Nothing here unloads a column, throttles a mesh
//! or gates an entity tick — those are policy, and policy needs a renderer and
//! a tuning pass to grade. What this produces is the three numbers vanilla's
//! `ClientPacketListener` keeps (`serverChunkRadius`,
//! `serverSimulationDistance`) plus the pair `ClientChunkCache` keeps
//! (`viewCenterX` / `viewCenterZ`), so a later policy has something true to
//! read instead of a guess.
//!
//! Chosen together because they are one thing: the volume of world the server
//! considers this client's. Splitting them across three modules would put
//! [`ViewArea::storage_radius`] and the centre it is measured from in
//! different files.
//!
//! ## Ground truth (bundled 26.2 decompile, `%APPDATA%/EwoClient/rewo/26.2/
//! decompiled/`)
//!
//! - `net/minecraft/network/protocol/game/ClientboundSetChunkCacheCenterPacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundSetChunkCacheRadiusPacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundSetSimulationDistancePacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundLoginPacket.java` — the
//!   initial `chunkRadius` / `simulationDistance` pair
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` —
//!   `handleSetChunkCacheRadius`, `handleSetChunkCacheCenter`,
//!   `handleSetSimulationDistance`, `handleLogin`, `handleRespawn`
//! - `net/minecraft/client/multiplayer/ClientChunkCache.java` —
//!   `calculateStorageRange`, `Storage.inRange`
//! - `net/minecraft/client/Options.java` — `getEffectiveRenderDistance`
//! - `net/minecraft/client/multiplayer/ClientLevel.java` — `shouldTickDeath`
//!
//! ## Four rules where the plausible implementation is silently wrong
//!
//! Each is mutation-tested by a witness below.
//!
//! 1. **The retention radius is not the radius on the wire.**
//!    `calculateStorageRange(viewRange)` is `max(2, viewRange) + 3`, so a
//!    server radius of 2 still stores 5. Retaining columns at the packet's own
//!    number evicts chunks the server still considers loaded — and only at
//!    small render distances, so it is invisible at 12 and wrong at 2.
//! 2. **A server radius of `0` means "no cap", not "render nothing".**
//!    `Options.getEffectiveRenderDistance` is
//!    `serverRenderDistance > 0 ? min(local, server) : local`. Clamping
//!    unconditionally renders an empty world, and `0` is exactly the value the
//!    field holds before any server has spoken.
//! 3. **`inRange` is Chebyshev** — `abs(dx) <= r && abs(dz) <= r`, a square.
//!    A Euclidean test drops the corner columns the server is still streaming.
//! 4. **A respawn does not reset any of this.** `handleRespawn` rebuilds the
//!    `ClientLevel` from the *existing* `serverChunkRadius` /
//!    `serverSimulationDistance` fields; only `handleLogin` and the two
//!    packets write them. Clearing the view area on a dimension change (which
//!    is what M16's `WorldTransition` does to everything else) would silently
//!    drop back to the defaults for the rest of the session.
//!
//! ## Why the two defaults differ, and why neither is an `Option`
//!
//! Vanilla holds this state in two places with two different pre-login
//! defaults, and both are deliberate:
//!
//! - `ClientPacketListener.serverChunkRadius = 3` — a *storage* radius, and
//!   there has to be some ring buffer before the first login.
//! - `Options.serverRenderDistance = 0` — a *cap*, where `0` is the sentinel
//!   for "uncapped" that rule 2 depends on.
//!
//! So `0` is a meaningful value rather than an absence, and wrapping either in
//! an `Option` would invent a distinction vanilla does not make — the
//! opposite call from `PlaySession::gamemodes`, where "the server has not
//! said" and "the server said survival" really are different answers.

use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

/// `ClientPacketListener.serverChunkRadius`'s field initialiser.
pub const DEFAULT_CHUNK_RADIUS: i32 = 3;
/// `Options.serverRenderDistance`'s field initialiser — the "uncapped"
/// sentinel, not a radius of zero. See rule 2.
pub const NO_RENDER_DISTANCE_CAP: i32 = 0;
/// `ClientChunkCache.calculateStorageRange`'s floor.
pub const MIN_STORAGE_VIEW_RANGE: i32 = 2;
/// `ClientChunkCache.calculateStorageRange`'s margin.
pub const STORAGE_RANGE_MARGIN: i32 = 3;

/// The three numbers the server uses to describe how much world is ours.
///
/// Constructed at [`ViewArea::default`] with vanilla's pre-login values, then
/// seeded by the login packet and updated by the three packets this module
/// decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewArea {
    /// `ClientChunkCache.Storage.viewCenterX` — a **chunk** coordinate.
    pub center_x: i32,
    /// `ClientChunkCache.Storage.viewCenterZ`.
    pub center_z: i32,
    /// `ClientPacketListener.serverChunkRadius`, in chunks. Also what
    /// `Options.setServerRenderDistance` is fed, which is why rule 2's `0`
    /// sentinel applies to this same field.
    pub chunk_radius: i32,
    /// `ClientLevel.serverSimulationDistance`, in chunks. Distinct from
    /// `chunk_radius`: it bounds which entities *tick*
    /// (`ClientLevel.shouldTickDeath` compares the chessboard distance
    /// against it), not which chunks are stored. A server routinely sets it
    /// lower than the render distance, so conflating the two is not caught by
    /// the common case.
    pub simulation_distance: i32,
}

impl Default for ViewArea {
    fn default() -> Self {
        Self {
            center_x: 0,
            center_z: 0,
            chunk_radius: DEFAULT_CHUNK_RADIUS,
            // Vanilla has no pre-login simulation distance to speak of; the
            // field is a plain `int`, so 0. It is not the rule-2 sentinel —
            // nothing tests this one against zero.
            simulation_distance: 0,
        }
    }
}

impl ViewArea {
    /// `ClientChunkCache.calculateStorageRange` — `max(2, viewRange) + 3`.
    ///
    /// The radius a column must be within to be *kept*, which is strictly
    /// larger than the radius on the wire. See rule 1.
    pub fn storage_radius(self) -> i32 {
        self.chunk_radius.max(MIN_STORAGE_VIEW_RANGE) + STORAGE_RANGE_MARGIN
    }

    /// `ClientChunkCache.Storage.inRange` — a **square**, in chunk
    /// coordinates, centred on the last `set_chunk_cache_center`. See rule 3.
    ///
    /// Vanilla logs `Ignoring chunk since it's not in the view range` and
    /// drops a `level_chunk_with_light` that fails this, so it is the test for
    /// "would the real client even have kept this column".
    pub fn in_range(self, chunk_x: i32, chunk_z: i32) -> bool {
        let r = self.storage_radius();
        (chunk_x - self.center_x).abs() <= r && (chunk_z - self.center_z).abs() <= r
    }

    /// `Options.getEffectiveRenderDistance` —
    /// `server > 0 ? min(local, server) : local`. See rule 2.
    pub fn effective_render_distance(self, local: i32) -> i32 {
        if self.chunk_radius > NO_RENDER_DISTANCE_CAP {
            local.min(self.chunk_radius)
        } else {
            local
        }
    }

    /// `ClientLevel.shouldTickDeath` — `ChunkPos.getChessboardDistance`
    /// against the simulation distance. Chebyshev again, but measured from
    /// the *player's* chunk rather than the view centre, so the caller passes
    /// both.
    pub fn within_simulation_distance(
        self,
        player_chunk: (i32, i32),
        entity_chunk: (i32, i32),
    ) -> bool {
        let dx = (entity_chunk.0 - player_chunk.0).abs();
        let dz = (entity_chunk.1 - player_chunk.1).abs();
        dx.max(dz) <= self.simulation_distance
    }

    /// Apply the pair `ClientboundLoginPacket` carries. Both fields sit in the
    /// login prefix, ahead of the embedded `CommonPlayerSpawnInfo`, and
    /// `handleLogin` assigns both plus `options.setServerRenderDistance`.
    ///
    /// The centre is deliberately untouched: login does not carry one, and
    /// the server sends a `set_chunk_cache_center` before the first chunk.
    pub fn apply_login(&mut self, chunk_radius: i32, simulation_distance: i32) {
        self.chunk_radius = chunk_radius;
        self.simulation_distance = simulation_distance;
    }
}

/// `ClientboundSetChunkCacheCenterPacket` — two VarInts, chunk coordinates.
///
/// **They are two's-complement VarInts, not zig-zag**, so a negative chunk
/// coordinate is five bytes. That is the whole encoding, but it is the field
/// most likely to be reached for with a zig-zag reader by anyone who has
/// written a protobuf decoder: zig-zag would read `-1` as `2147483647` and
/// put the view centre 33 million blocks away.
pub fn read_center(r: &mut PacketReader<'_>) -> Result<(i32, i32)> {
    let x = r.varint()?;
    let z = r.varint()?;
    Ok((x, z))
}

/// `ClientboundSetChunkCacheRadiusPacket` — one VarInt.
pub fn read_chunk_radius(r: &mut PacketReader<'_>) -> Result<i32> {
    r.varint()
}

/// `ClientboundSetSimulationDistancePacket` — one VarInt.
///
/// Byte-identical to [`read_chunk_radius`] and a *different quantity*: only
/// the packet id distinguishes them, which is why the routing layer decides
/// and neither reader guesses from the body. The same shape is why M63 takes
/// the sound packet's kind from its id rather than sniffing the body.
pub fn read_simulation_distance(r: &mut PacketReader<'_>) -> Result<i32> {
    r.varint()
}

/// Which of the three view-area packets a body is — decided by the caller
/// from the resolved packet id, never from the bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewAreaPacket {
    /// `ClientboundSetChunkCacheCenterPacket`.
    Center,
    /// `ClientboundSetChunkCacheRadiusPacket`.
    ChunkRadius,
    /// `ClientboundSetSimulationDistancePacket`.
    SimulationDistance,
}

/// The three resolved packet ids, in the one order that maps them to kinds.
///
/// Split out of `crate::route_view_area` so the mapping can be witnessed
/// without an `Ids`, which only exists once the datagen report is loaded. It
/// is the *same* decision, parameterised — not a second copy of it: the router
/// calls this and does nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewAreaIds {
    pub center: i32,
    pub chunk_radius: i32,
    pub simulation_distance: i32,
}

/// Which view-area packet an id names, or `None` for anything else.
///
/// The radius and the simulation distance are byte-identical on the wire, so
/// this is the **only** place the two are told apart. Swapping the two arms
/// produces a client that decodes every body successfully and stores each
/// number in the other's field.
pub fn kind_for_id(id: i32, ids: ViewAreaIds) -> Option<ViewAreaPacket> {
    if id == ids.center {
        Some(ViewAreaPacket::Center)
    } else if id == ids.chunk_radius {
        Some(ViewAreaPacket::ChunkRadius)
    } else if id == ids.simulation_distance {
        Some(ViewAreaPacket::SimulationDistance)
    } else {
        None
    }
}

/// Decode one view-area packet into `area`.
///
/// Returns whether the body decoded. A short body leaves `area` **completely
/// untouched** rather than half-applied — the same stance `set_player_team`
/// and the M65 arms take, and it matters more here than it looks: the centre's
/// two VarInts are positional, so a body that runs out after `x` would
/// otherwise move the view area to a centre the server never sent.
pub fn apply(kind: ViewAreaPacket, body: &[u8], area: &mut ViewArea) -> bool {
    let mut r = PacketReader::new(body);
    match kind {
        ViewAreaPacket::Center => match read_center(&mut r) {
            Ok((x, z)) => {
                area.center_x = x;
                area.center_z = z;
                true
            }
            Err(_) => false,
        },
        ViewAreaPacket::ChunkRadius => match read_chunk_radius(&mut r) {
            Ok(v) => {
                area.chunk_radius = v;
                true
            }
            Err(_) => false,
        },
        ViewAreaPacket::SimulationDistance => match read_simulation_distance(&mut r) {
            Ok(v) => {
                area.simulation_distance = v;
                true
            }
            Err(_) => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_proto::writer::PacketWriter;

    /// A byte no reader in this module can consume: every `read_*` here ends
    /// on a known field, so if a body is followed by this and the reader is
    /// not sitting exactly on it, the decode read the wrong number of bytes.
    const SENTINEL: u8 = 0xA7;

    fn with_sentinel(body: &[u8]) -> Vec<u8> {
        let mut v = body.to_vec();
        v.push(SENTINEL);
        v
    }

    fn assert_consumed_exactly(r: &mut PacketReader<'_>) {
        assert_eq!(r.remaining(), 1, "decoder must stop on the sentinel byte");
        assert_eq!(r.u8().unwrap(), SENTINEL, "trailing byte is the sentinel");
        assert_eq!(r.remaining(), 0);
    }

    fn body(f: impl FnOnce(&mut PacketWriter)) -> Vec<u8> {
        let mut w = PacketWriter::default();
        f(&mut w);
        w.into_bytes()
    }

    #[test]
    fn the_centre_is_two_var_ints_and_consumes_exactly_them() {
        let bytes = with_sentinel(&body(|w| {
            w.varint(25565);
            w.varint(7);
        }));
        let mut r = PacketReader::new(&bytes);
        assert_eq!(read_center(&mut r).unwrap(), (25565, 7));
        assert_consumed_exactly(&mut r);
    }

    #[test]
    fn a_negative_chunk_centre_is_a_five_byte_twos_complement_var_int_not_zig_zag() {
        // Zig-zag would encode -1 as a single 0x01 byte and decode 0x01 as -1;
        // Minecraft writes -1 as ff ff ff ff 0f. A zig-zag reader applied to
        // these bytes yields 2147483647 — a view centre 33 million blocks out,
        // which reads as "no chunk is ever in range" rather than as an error.
        let bytes = with_sentinel(&body(|w| {
            w.varint(-1);
            w.varint(-2048);
        }));
        assert_eq!(
            &bytes[..5],
            &[0xff, 0xff, 0xff, 0xff, 0x0f],
            "-1 must be the five-byte two's-complement form"
        );
        let mut r = PacketReader::new(&bytes);
        assert_eq!(read_center(&mut r).unwrap(), (-1, -2048));
        assert_consumed_exactly(&mut r);
    }

    #[test]
    fn the_radius_and_the_simulation_distance_are_one_var_int_each() {
        for radius in [0i32, 2, 12, 32, -1] {
            let bytes = with_sentinel(&body(|w| {
                w.varint(radius);
            }));
            let mut r = PacketReader::new(&bytes);
            assert_eq!(read_chunk_radius(&mut r).unwrap(), radius);
            assert_consumed_exactly(&mut r);

            let mut r = PacketReader::new(&bytes);
            assert_eq!(read_simulation_distance(&mut r).unwrap(), radius);
            assert_consumed_exactly(&mut r);
        }
    }

    /// Found by mutation: replacing `r.varint()` with `r.varlong()? as i32`
    /// **survived** every other witness in this module. It is very nearly an
    /// equivalent mutant — for any `i32` the server actually writes, the
    /// five-byte two's-complement form ends on a byte with no continuation
    /// bit, so a VarLong reader consumes the same bytes and its low 32 bits
    /// are the same number. The one place the two differ is a *malformed*
    /// body: a VarInt reader must reject a sixth continuation byte, and a
    /// VarLong reader happily keeps going for four more. That is the whole
    /// remaining difference, so it is the whole remaining witness.
    #[test]
    fn an_overlong_var_int_is_rejected_rather_than_read_as_a_var_long() {
        // rewo-proto's own canonical overlong vector: six bytes, five of them
        // carrying the continuation bit.
        let overlong = [0xffu8, 0xff, 0xff, 0xff, 0xff, 0x01];
        let mut r = PacketReader::new(&overlong);
        assert!(read_chunk_radius(&mut r).is_err());
        let mut r = PacketReader::new(&overlong);
        assert!(read_simulation_distance(&mut r).is_err());
        let mut r = PacketReader::new(&overlong);
        assert!(read_center(&mut r).is_err());

        let mut area = ViewArea::default();
        let before = area;
        assert!(!apply(ViewAreaPacket::ChunkRadius, &overlong, &mut area));
        assert_eq!(area, before);
    }

    #[test]
    fn the_storage_radius_is_max_two_plus_three_not_the_radius_on_the_wire() {
        // ClientChunkCache.calculateStorageRange. The floor is what a naive
        // `radius + 3` gets wrong, and only below 2 — so the failure hides at
        // every normal render distance.
        let cases = [(-5, 5), (0, 5), (1, 5), (2, 5), (3, 6), (12, 15), (32, 35)];
        for (wire, expected) in cases {
            let area = ViewArea {
                chunk_radius: wire,
                ..ViewArea::default()
            };
            assert_eq!(area.storage_radius(), expected, "radius {wire}");
            assert_ne!(
                area.storage_radius(),
                wire,
                "the storage radius is never the wire radius"
            );
        }
    }

    #[test]
    fn a_server_radius_of_zero_means_uncapped_not_a_blank_world() {
        // Options.getEffectiveRenderDistance. Zero is the pre-login value, so
        // clamping unconditionally renders nothing until the first radius
        // packet arrives — which some servers never send, because login
        // already carried it.
        let uncapped = ViewArea {
            chunk_radius: 0,
            ..ViewArea::default()
        };
        assert_eq!(uncapped.effective_render_distance(16), 16);
        let negative = ViewArea {
            chunk_radius: -3,
            ..ViewArea::default()
        };
        assert_eq!(negative.effective_render_distance(16), 16);

        let capped = ViewArea {
            chunk_radius: 8,
            ..ViewArea::default()
        };
        assert_eq!(capped.effective_render_distance(16), 8);
        assert_eq!(
            capped.effective_render_distance(4),
            4,
            "the cap is a minimum, so a smaller local setting wins"
        );
    }

    #[test]
    fn in_range_is_a_chebyshev_square_around_the_centre() {
        let area = ViewArea {
            center_x: 10,
            center_z: -4,
            chunk_radius: 2, // storage radius 5
            ..ViewArea::default()
        };
        assert_eq!(area.storage_radius(), 5);
        // The corner is the whole point: it is inside the square and outside
        // the circle of the same radius, so a Euclidean test drops it.
        assert!(area.in_range(15, 1), "the (+5, +5) corner is in range");
        assert!(
            ((15 - 10) as f64).hypot((1 - -4) as f64) > 5.0,
            "…and is outside a circle of the same radius, which is what makes \
             this witness distinguish the two tests"
        );
        assert!(area.in_range(5, -9), "the (-5, -5) corner is in range");
        assert!(!area.in_range(16, -4), "one past the edge on x");
        assert!(!area.in_range(10, 2), "one past the edge on z");
    }

    #[test]
    fn the_simulation_distance_bounds_ticking_not_storage() {
        let area = ViewArea {
            chunk_radius: 12,
            simulation_distance: 4,
            ..ViewArea::default()
        };
        // A chunk well inside the storage radius but outside the simulation
        // distance: stored, not ticked. Reading one field as the other makes
        // this case vanish — with radius 12 the storage radius is 15, so
        // (8, 0) is comfortably stored while sitting twice the simulation
        // distance away.
        assert!(area.in_range(8, 0), "stored");
        assert!(
            !area.within_simulation_distance((0, 0), (8, 0)),
            "…and not ticked"
        );
        assert!(area.within_simulation_distance((0, 0), (4, -4)));
        assert!(!area.within_simulation_distance((0, 0), (5, 0)));
        assert!(!area.within_simulation_distance((0, 0), (0, -5)));
    }

    #[test]
    fn the_pre_login_default_radius_is_three_and_still_yields_a_ring_buffer() {
        let area = ViewArea::default();
        assert_eq!(area.chunk_radius, 3, "ClientPacketListener's initialiser");
        assert_eq!(
            area.storage_radius(),
            6,
            "so a client that never logged in still has a ring buffer"
        );
        // …and the same field read as a cap is `> 0`, so it *does* cap at 3
        // once seeded. The `0` sentinel only applies before login in vanilla
        // because `Options.serverRenderDistance` is a separate field; Rewo
        // folds them, so the honest statement is that 0 is uncapped and 3 is
        // a cap of 3.
        assert_eq!(area.effective_render_distance(16), 3);
    }

    #[test]
    fn a_short_body_leaves_the_view_area_completely_untouched() {
        let before = ViewArea {
            center_x: 100,
            center_z: 200,
            chunk_radius: 12,
            simulation_distance: 10,
        };

        // A centre body that runs out after `x`: half-applying it would move
        // the view centre to a place the server never named.
        let mut area = before;
        let truncated = body(|w| {
            w.varint(7);
        });
        assert!(!apply(ViewAreaPacket::Center, &truncated[..0], &mut area));
        assert_eq!(area, before);
        assert!(!apply(ViewAreaPacket::Center, &truncated, &mut area));
        assert_eq!(area, before, "x read, z missing — nothing applied");

        for kind in [ViewAreaPacket::ChunkRadius, ViewAreaPacket::SimulationDistance] {
            let mut area = before;
            assert!(!apply(kind, &[], &mut area));
            assert_eq!(area, before);
        }
    }

    #[test]
    fn each_packet_writes_only_its_own_field() {
        let mut area = ViewArea::default();

        assert!(apply(
            ViewAreaPacket::Center,
            &body(|w| {
                w.varint(-3);
                w.varint(9);
            }),
            &mut area
        ));
        assert_eq!((area.center_x, area.center_z), (-3, 9));
        assert_eq!(area.chunk_radius, DEFAULT_CHUNK_RADIUS, "radius untouched");
        assert_eq!(area.simulation_distance, 0, "sim distance untouched");

        assert!(apply(
            ViewAreaPacket::ChunkRadius,
            &body(|w| {
                w.varint(16);
            }),
            &mut area
        ));
        assert_eq!(area.chunk_radius, 16);
        assert_eq!(area.simulation_distance, 0, "sim distance still untouched");
        assert_eq!((area.center_x, area.center_z), (-3, 9), "centre kept");

        assert!(apply(
            ViewAreaPacket::SimulationDistance,
            &body(|w| {
                w.varint(6);
            }),
            &mut area
        ));
        assert_eq!(area.simulation_distance, 6);
        assert_eq!(area.chunk_radius, 16, "radius kept — they are two fields");
    }

    #[test]
    fn the_radius_and_the_simulation_distance_are_told_apart_only_by_their_ids() {
        // Their bodies are the same single VarInt, so nothing downstream can
        // notice a swap here: both decode, both store, and the client then
        // renders at the simulation distance and ticks at the render
        // distance. This mapping is the only guard that exists.
        let ids = ViewAreaIds {
            center: 94,
            chunk_radius: 95,
            simulation_distance: 111,
        };
        assert_eq!(kind_for_id(94, ids), Some(ViewAreaPacket::Center));
        assert_eq!(kind_for_id(95, ids), Some(ViewAreaPacket::ChunkRadius));
        assert_eq!(
            kind_for_id(111, ids),
            Some(ViewAreaPacket::SimulationDistance)
        );
        assert_eq!(kind_for_id(0, ids), None, "an unrelated id routes nowhere");
        assert_eq!(kind_for_id(96, ids), None);

        // Drive the ids end-to-end so a swapped arm lands in the wrong field
        // rather than merely returning the wrong enum.
        let mut area = ViewArea::default();
        let one = |v: i32| body(|w| { w.varint(v); });
        apply(kind_for_id(95, ids).unwrap(), &one(16), &mut area);
        apply(kind_for_id(111, ids).unwrap(), &one(6), &mut area);
        assert_eq!(area.chunk_radius, 16, "id 95 is the render distance");
        assert_eq!(area.simulation_distance, 6, "id 111 is the tick distance");
    }

    #[test]
    fn login_seeds_both_distances_and_leaves_the_centre_alone() {
        let mut area = ViewArea {
            center_x: 42,
            center_z: -42,
            ..ViewArea::default()
        };
        area.apply_login(10, 8);
        assert_eq!(area.chunk_radius, 10);
        assert_eq!(area.simulation_distance, 8);
        assert_eq!(
            (area.center_x, area.center_z),
            (42, -42),
            "login carries no centre; a set_chunk_cache_center precedes the \
             first chunk"
        );
    }
}
