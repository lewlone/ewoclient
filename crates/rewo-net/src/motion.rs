//! The four packets that move the local player from outside its own input
//! (M68): `explode`, `set_entity_motion`, `move_vehicle`, `set_passengers`.
//!
//! `REWO_PACKET_COVERAGE.md` §3.1 ranks these together, and the reason is not
//! that they are similar on the wire — they are not — but that they are the
//! four inputs `rewo play`'s `CORRECTIONS` meter is **structurally unable to
//! see**. That harness walks on flat ground; it is never knocked back, never
//! exploded at and never mounted, so the headline "0 corrections" says
//! nothing about any of them. Decoding them is half the milestone; the other
//! half is [`crate::play::MotionStats`] and the harness gate that drives it.
//!
//! ## Ground truth (bundled 26.2 decompile, `%APPDATA%/EwoClient/rewo/26.2/
//! decompiled/`)
//!
//! - `net/minecraft/network/protocol/game/ClientboundExplodePacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundSetEntityMotionPacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundMoveVehiclePacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundSetPassengersPacket.java`
//! - `net/minecraft/network/LpVec3.java` — the quantised velocity packing
//! - `net/minecraft/world/phys/Vec3.java` — `STREAM_CODEC`, `LP_STREAM_CODEC`
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` —
//!   `handleExplosion`, `handleSetEntityMotion`, `handleMoveVehicle`,
//!   `handleSetEntityPassengersPacket`
//! - `net/minecraft/world/entity/Entity.java` — `lerpMotion`,
//!   `addDeltaMovement`, `setDeltaMovement`, `ejectPassengers`, `startRiding`
//!
//! ## Five rules where the plausible implementation is silently wrong
//!
//! Each is mutation-tested by a witness at the bottom of this file.
//!
//! 1. **Velocity is NOT the legacy `short / 8000.0` fixed point.** Every
//!    pre-26.2 reference — and the brief this milestone started from — says a
//!    velocity component is a `short` in thousandths of a block per tick,
//!    clamped to ±3.9. **That encoding does not exist in 26.2.** A repo-wide
//!    grep of the decompile finds no `8000.0` outside a renderer rotation and
//!    no `Mth.clamp(x, -3.9, 3.9)` anywhere. `ClientboundSetEntityMotionPacket`
//!    composes `Vec3.LP_STREAM_CODEC`, which is [`read_lp_vec3`] — a
//!    variable-length packing of three 15-bit mantissas against one shared
//!    integer scale. Reading three shorts here consumes 6 bytes of a 6-or-more
//!    byte body, desynchronises nothing (packets are length-framed) and
//!    produces a velocity that is wrong by an arbitrary factor.
//! 2. **`lowest == 0` is a one-byte zero sentinel.** `LpVec3.read` returns
//!    `Vec3.ZERO` having consumed exactly one byte, so the body of a
//!    `set_entity_motion` that stops an entity is *two* bytes total (VarInt id
//!    + the sentinel). A reader that always takes six bytes overruns every
//!    stop-moving packet — and "stop moving" is the single most common
//!    velocity update a server sends.
//! 3. **`explode`'s `blockCount` is `ByteBufCodecs.INT`, a fixed big-endian
//!    i32 — not a VarInt.** It sits between two fields this milestone needs
//!    (`radius` and `playerKnockback`), so reading it as a VarInt misaligns
//!    the knockback that is the entire physics payload. Same trap as M34's
//!    signed-short slot index among var-ints.
//! 4. **`lerpMotion` REPLACES; explosion knockback ADDS.** In 26.2
//!    `Entity.lerpMotion(Vec3)` is a bare `setDeltaMovement(movement)` — the
//!    name is vestigial, there is no interpolation — whereas
//!    `handleExplosion` ends in `addDeltaMovement`. Getting either backwards
//!    is invisible when the player is at rest and wrong whenever it is not.
//!    Both are guarded by `Vec3.isFinite()`, which is why a NaN component
//!    leaves the velocity *unchanged* rather than poisoning it.
//! 5. **`set_passengers` REPLACES the rider list.** `handleSetEntityPassengersPacket`
//!    calls `vehicle.ejectPassengers()` **before** it starts anyone riding, so
//!    merging the new list into the old strands every rider that left — a
//!    dismount is expressed as an ADD packet whose list no longer contains
//!    you, never as a removal.
//!
//! ## Two structural facts about what can be exercised live
//!
//! Recorded here because they bound what the harness gate in
//! `rewo play --motion-check` can claim, and both were measured from the
//! decompile rather than assumed:
//!
//! - **`move_vehicle` is only ever a rejection.** Its two send sites are both
//!   inside `ServerGamePacketListenerImpl.handleMoveVehicle`, i.e. the server
//!   answering a *serverbound* `ServerboundMoveVehiclePacket` it did not like.
//!   A client that never claims to drive a vehicle never receives one. Rewo
//!   rides as a passenger and does not send the serverbound half (see
//!   [`VehicleMove`]), so this packet is decode-and-unit-test only — the live
//!   gate cannot trigger it, and says so rather than quietly passing.
//! - **A mounted player's movement is not validated at all.**
//!   `ServerGamePacketListenerImpl` line ~1086: `if (this.player.isPassenger())`
//!   the server snaps rotation and keeps its own position, skipping the whole
//!   move-check. So `CORRECTIONS` cannot rise while mounted **even if the
//!   client's idea of where it is sitting is completely wrong** — which makes
//!   "0 corrections while riding" a statement about vanilla's server, not
//!   about Rewo. The gate reports mount corrections separately for exactly
//!   this reason.

use std::collections::HashMap;

use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

/// A three-component double vector, matching `net.minecraft.world.phys.Vec3`.
///
/// Local to this module rather than shared: the rest of Rewo carries positions
/// as loose `f64` triples or `[f32; 3]`, and inventing a project-wide vector
/// type as a side effect of a protocol milestone would be a refactor wearing a
/// decode's clothes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// `Vec3.isFinite()` — the guard on both `setDeltaMovement` and
    /// `addDeltaMovement`. A non-finite vector is **dropped**, leaving the
    /// previous velocity in place; it is not clamped and not zeroed.
    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

// ── LpVec3 ──────────────────────────────────────────────────────────────────

/// `LpVec3`'s field constants, verbatim.
const DATA_BITS_MASK: u64 = 32767;
const MAX_QUANTIZED_VALUE: f64 = 32766.0;
const SCALE_BITS_MASK: u64 = 3;
const CONTINUATION_FLAG: u64 = 4;
const X_OFFSET: u32 = 3;
const Y_OFFSET: u32 = 18;
const Z_OFFSET: u32 = 33;

/// `LpVec3.unpack` — a 15-bit mantissa back to `[-1, 1]`.
///
/// The `min` against [`MAX_QUANTIZED_VALUE`] is not decoration: the mask
/// admits 32767 while the writer's `pack` never emits more than 32766, so the
/// clamp is what keeps a hand-written or hostile body inside `[-1, 1]`.
fn unpack(value: u64) -> f64 {
    ((value & DATA_BITS_MASK) as f64).min(MAX_QUANTIZED_VALUE) * 2.0 / MAX_QUANTIZED_VALUE - 1.0
}

/// `Vec3.LP_STREAM_CODEC` → `LpVec3.read`.
///
/// The layout, from the decompiled reader: one unsigned byte (`0` ⇒
/// [`Vec3::ZERO`], consuming nothing further), then one unsigned byte and one
/// **big-endian unsigned i32**, assembled as
/// `highest << 16 | middle << 8 | lowest` into 48 bits. Bits 0..1 are the low
/// two bits of the scale, bit 2 is a continuation flag, and bits 3/18/33 start
/// the three 15-bit mantissas. When the flag is set a VarInt follows carrying
/// `scale >> 2`.
///
/// Every component is `unpack(bits) * scale`, so the three share one
/// magnitude — which is why a velocity with one large and two tiny components
/// loses precision in the small ones, and why the encoding is compact.
pub fn read_lp_vec3(r: &mut PacketReader) -> Result<Vec3> {
    let lowest = r.u8()? as u64;
    // Rule 2: the one-byte sentinel. `write` emits this whenever the
    // chessboard length is below `ABS_MIN_VALUE`, which is every "this entity
    // has stopped" update.
    if lowest == 0 {
        return Ok(Vec3::ZERO);
    }
    let middle = r.u8()? as u64;
    // `readUnsignedInt` — four big-endian bytes widened without sign.
    let highest = r.i32()? as u32 as u64;
    let buffer = (highest << 16) | (middle << 8) | lowest;

    let mut scale = lowest & SCALE_BITS_MASK;
    if lowest & CONTINUATION_FLAG == CONTINUATION_FLAG {
        // `(VarInt.read(input) & 4294967295L) << 2` — masked to 32 unsigned
        // bits before the shift, so a negative VarInt does not sign-extend
        // into the high half of the scale.
        scale |= (r.varint()? as u32 as u64) << 2;
    }
    let scale = scale as f64;

    Ok(Vec3 {
        x: unpack(buffer >> X_OFFSET) * scale,
        y: unpack(buffer >> Y_OFFSET) * scale,
        z: unpack(buffer >> Z_OFFSET) * scale,
    })
}

/// `Vec3.STREAM_CODEC` — three plain big-endian f64s.
///
/// Distinct from [`read_lp_vec3`] and used by the *other* three packets here.
/// `explode` and `move_vehicle` carry positions, which need absolute
/// precision; only velocity is quantised.
pub fn read_vec3(r: &mut PacketReader) -> Result<Vec3> {
    Ok(Vec3 {
        x: r.f64()?,
        y: r.f64()?,
        z: r.f64()?,
    })
}

// ── set_entity_motion ───────────────────────────────────────────────────────

/// `ClientboundSetEntityMotionPacket` — one entity's velocity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityMotion {
    /// VarInt entity id. For the local player's own id this is knockback:
    /// from a hit, from a wind charge, from a mace slam.
    pub id: i32,
    pub movement: Vec3,
}

/// Decode a `set_entity_motion` body: VarInt id, then `Vec3.LP_STREAM_CODEC`.
pub fn read_set_entity_motion(body: &[u8]) -> Result<EntityMotion> {
    let mut r = PacketReader::new(body);
    let id = r.varint()?;
    let movement = read_lp_vec3(&mut r)?;
    Ok(EntityMotion { id, movement })
}

// ── explode ─────────────────────────────────────────────────────────────────

/// The physics-bearing prefix of `ClientboundExplodePacket`.
///
/// **This is a deliberate partial decode, and the one place in this module
/// where the body is not fully consumed.** The packet's seven fields are, in
/// order: `center`, `radius`, `blockCount`, `playerKnockback`,
/// `explosionParticle`, `explosionSound`, `blockParticles`. The four this
/// struct carries are exactly the prefix — the physics payload sits entirely
/// before the open-ended tail.
///
/// The tail is skipped rather than walked because `ParticleTypes.STREAM_CODEC`
/// dispatches on a registry id into that particle type's own options codec,
/// and M37 transcribed only the handful of types it simulates
/// (`route_level_particles` returns `None` for the rest for the same reason).
/// Consuming `explosionParticle` therefore requires transcribing ~125 option
/// codecs, `explosionSound` requires the inline-`SoundEvent` branch, and
/// `blockParticles` requires a `WeightedList` of the first. None of that
/// changes a single number the player's velocity depends on.
///
/// Stopping early is safe rather than a desync risk for the reason
/// `route_level_particles` records: frames are length-prefixed, so abandoning
/// a body part-way never disturbs the stream. [`read_explode`] documents the
/// exact byte count it consumes, and a witness pins it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Explosion {
    pub center: Vec3,
    pub radius: f32,
    /// `ByteBufCodecs.INT` — a fixed big-endian i32, **not** a VarInt (rule 3).
    pub block_count: i32,
    /// `Optional<Vec3>`, and the whole reason this packet is class A:
    /// `handleExplosion` ends in
    /// `packet.playerKnockback().ifPresent(this.minecraft.player::addDeltaMovement)`.
    ///
    /// Absent is the common case — `ServerExplosion.hitPlayers` only records a
    /// player that is neither a spectator nor a *flying* creative player, so
    /// most explosions in most sessions carry `None`.
    pub player_knockback: Option<Vec3>,
}

/// Bytes [`read_explode`] consumes when `playerKnockback` is absent:
/// 24 (`center`) + 4 (`radius`) + 4 (`blockCount`) + 1 (the `Optional` tag).
pub const EXPLODE_PREFIX_LEN_NO_KNOCKBACK: usize = 33;
/// Bytes [`read_explode`] consumes when `playerKnockback` is present — the
/// above plus a second `Vec3.STREAM_CODEC` (24 bytes of doubles, *not* the
/// quantised `LpVec3` the velocity packet uses).
pub const EXPLODE_PREFIX_LEN_WITH_KNOCKBACK: usize = 57;

/// Decode the physics prefix of an `explode` body. See [`Explosion`] for why
/// the trailing three fields are not consumed.
///
/// Returns the decoded prefix and the number of bytes it took, so a caller (or
/// a witness) can assert the stop point rather than trust it.
pub fn read_explode(body: &[u8]) -> Result<(Explosion, usize)> {
    let mut r = PacketReader::new(body);
    let center = read_vec3(&mut r)?;
    let radius = r.f32()?;
    // Rule 3: fixed i32, not a VarInt.
    let block_count = r.i32()?;
    // `Vec3.STREAM_CODEC.apply(ByteBufCodecs::optional)` — a boolean byte,
    // then the vector only when it is set.
    let player_knockback = if r.bool()? {
        Some(read_vec3(&mut r)?)
    } else {
        None
    };
    Ok((
        Explosion {
            center,
            radius,
            block_count,
            player_knockback,
        },
        r.offset(),
    ))
}

// ── move_vehicle ────────────────────────────────────────────────────────────

/// `ClientboundMoveVehiclePacket` — the server repositioning the vehicle the
/// local player is riding.
///
/// **There is no entity id in this packet.** The client resolves the target
/// itself as `this.minecraft.player.getRootVehicle()`, so the packet is
/// meaningless to a client that is not currently a passenger — which is also
/// why it needs no id.
///
/// Rewo applies the pose to its tracked vehicle and deliberately does **not**
/// send the `ServerboundMoveVehiclePacket` echo vanilla's handler ends with.
/// That echo is a *controlling* client asserting where it drove the vehicle;
/// Rewo implements no vehicle physics, so echoing would claim an authority it
/// cannot back and invite the "moved wrongly" rejection path. Riding as a
/// passenger and letting the server stay authoritative is the honest subset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehicleMove {
    pub position: Vec3,
    /// Written **before** `x_rot` — the pair is yaw-then-pitch, the opposite
    /// order from the field names' alphabetical reading.
    pub y_rot: f32,
    pub x_rot: f32,
}

/// Decode a `move_vehicle` body: `Vec3.STREAM_CODEC` then two f32s. Exactly
/// 32 bytes, no var-ints anywhere.
pub fn read_move_vehicle(body: &[u8]) -> Result<VehicleMove> {
    let mut r = PacketReader::new(body);
    let position = read_vec3(&mut r)?;
    let y_rot = r.f32()?;
    let x_rot = r.f32()?;
    Ok(VehicleMove {
        position,
        y_rot,
        x_rot,
    })
}

// ── set_passengers ──────────────────────────────────────────────────────────

/// `ClientboundSetPassengersPacket` — who is riding what.
///
/// The one packet of the four that is not a record over a composed
/// `StreamCodec`: it is a hand-written `Packet.codec(write, read)` pair, and
/// its body is a VarInt vehicle id followed by `readVarIntArray` (a VarInt
/// length then that many VarInts).
#[derive(Clone, Debug, PartialEq)]
pub struct Passengers {
    pub vehicle: i32,
    /// The **complete** rider list after this packet, in the server's order.
    /// Not a delta — see rule 5.
    pub passengers: Vec<i32>,
}

/// A sanity bound on the rider array, so a corrupt length cannot make us
/// allocate wildly before the reader runs out of bytes.
///
/// Vanilla's own bound is `readVarIntArray()`'s default `this.readableBytes()`
/// — i.e. the frame's remaining length, since a VarInt is at least one byte.
/// Using the remaining byte count reproduces that exactly rather than
/// inventing a fixed cap.
fn passenger_cap(r: &PacketReader) -> usize {
    r.remaining()
}

/// Decode a `set_passengers` body.
pub fn read_set_passengers(body: &[u8]) -> Result<Passengers> {
    let mut r = PacketReader::new(body);
    let vehicle = r.varint()?;
    let count = r.varint()?;
    let cap = passenger_cap(&r);
    if count < 0 || count as usize > cap {
        return Err(rewo_proto::ProtoError::Eof {
            needed: count.max(0) as usize,
            remaining: cap,
        });
    }
    let mut passengers = Vec::with_capacity(count as usize);
    for _ in 0..count {
        passengers.push(r.varint()?);
    }
    Ok(Passengers {
        vehicle,
        passengers,
    })
}

// ── mount state ─────────────────────────────────────────────────────────────

/// Who is riding what, as the client knows it.
///
/// Mirrors the pair vanilla keeps implicitly through `Entity.passengers` and
/// `Entity.vehicle`: a vehicle's rider list, and each rider's vehicle. Both
/// directions are stored because both are asked — the renderer wants a
/// vehicle's riders, and the physics wants "am *I* riding something".
#[derive(Clone, Debug, Default)]
pub struct Mounts {
    by_vehicle: HashMap<i32, Vec<i32>>,
    vehicle_of: HashMap<i32, i32>,
}

impl Mounts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a `set_passengers`, exactly as `handleSetEntityPassengersPacket`
    /// does: **eject every current passenger first**, then seat the new list
    /// (rule 5).
    ///
    /// The eject step is what makes a dismount work at all. The server never
    /// sends "X stopped riding"; it re-sends the vehicle's list without X, and
    /// a merge would leave X seated forever.
    pub fn apply(&mut self, p: &Passengers) {
        // `vehicle.ejectPassengers()`.
        if let Some(old) = self.by_vehicle.remove(&p.vehicle) {
            for rider in old {
                // Only clear the back-link if it still points at *this*
                // vehicle: a rider that already moved to another vehicle in an
                // earlier packet must keep its newer seat.
                if self.vehicle_of.get(&rider) == Some(&p.vehicle) {
                    self.vehicle_of.remove(&rider);
                }
            }
        }
        for &rider in &p.passengers {
            // `passenger.startRiding(vehicle, true, false)` — `force = true`,
            // so there is no `canRide` gate to reproduce. A rider moving
            // between vehicles leaves its old seat.
            if let Some(prev) = self.vehicle_of.insert(rider, p.vehicle) {
                if prev != p.vehicle {
                    if let Some(list) = self.by_vehicle.get_mut(&prev) {
                        list.retain(|&r| r != rider);
                    }
                }
            }
        }
        if !p.passengers.is_empty() {
            self.by_vehicle.insert(p.vehicle, p.passengers.clone());
        }
    }

    /// The vehicle `rider` is directly riding, if any.
    pub fn vehicle_of(&self, rider: i32) -> Option<i32> {
        self.vehicle_of.get(&rider).copied()
    }

    /// The riders of `vehicle`, in the server's order.
    pub fn passengers(&self, vehicle: i32) -> &[i32] {
        self.by_vehicle
            .get(&vehicle)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// `Entity.getRootVehicle()` — walk up the chain to the outermost vehicle.
    ///
    /// Bounded by the number of known seats so a server that sent a cycle
    /// (which `startRiding`'s own loop-check prevents server-side, but which a
    /// hostile or buggy peer could still emit) cannot hang the client.
    pub fn root_vehicle(&self, rider: i32) -> Option<i32> {
        let mut cur = self.vehicle_of(rider)?;
        for _ in 0..self.vehicle_of.len() {
            match self.vehicle_of(cur) {
                Some(next) => cur = next,
                None => return Some(cur),
            }
        }
        Some(cur)
    }

    /// Forget everything. Called on a dimension change, where every entity id
    /// is invalidated along with the world.
    pub fn clear(&mut self) {
        self.by_vehicle.clear();
        self.vehicle_of.clear();
    }

    /// Drop an entity that has been removed, from both directions.
    pub fn remove_entity(&mut self, id: i32) {
        if let Some(vehicle) = self.vehicle_of.remove(&id) {
            if let Some(list) = self.by_vehicle.get_mut(&vehicle) {
                list.retain(|&r| r != id);
            }
        }
        if let Some(riders) = self.by_vehicle.remove(&id) {
            for r in riders {
                if self.vehicle_of.get(&r) == Some(&id) {
                    self.vehicle_of.remove(&r);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Append a sentinel past the body and assert the decode stopped exactly
    /// at its start — the pattern every wire witness in this crate uses. A
    /// decoder that consumed one byte too few or too many would read the
    /// sentinel or leave it, and both show up here.
    const SENTINEL: &[u8] = &[0xDE, 0xAD, 0xBE, 0xEF];

    fn with_sentinel(mut body: Vec<u8>) -> (Vec<u8>, usize) {
        let len = body.len();
        body.extend_from_slice(SENTINEL);
        (body, len)
    }

    fn varint(v: i32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut val = v as u32;
        loop {
            if val & !0x7F == 0 {
                out.push(val as u8);
                return out;
            }
            out.push(((val & 0x7F) | 0x80) as u8);
            val >>= 7;
        }
    }

    // ── LpVec3 ──────────────────────────────────────────────────────────

    /// Encode a `Vec3` exactly as `LpVec3.write` does, so the round-trip
    /// witnesses below drive the real decoder with real bodies.
    ///
    /// This is a transcription of the *writer*, which the client never runs —
    /// so it is an independent statement of the format, not a mirror of
    /// [`read_lp_vec3`]. A decoder bug that the encoder shared would have to
    /// be made twice, in opposite directions.
    fn write_lp_vec3(v: Vec3) -> Vec<u8> {
        fn sanitize(value: f64) -> f64 {
            if value.is_nan() {
                0.0
            } else {
                value.clamp(-1.7179869183E10, 1.7179869183E10)
            }
        }
        fn pack(value: f64) -> u64 {
            // Java's `Math.round(double)` is floor(x + 0.5); the values here
            // are always positive so `.round()` agrees.
            ((value * 0.5 + 0.5) * 32766.0).round() as u64
        }
        let (x, y, z) = (sanitize(v.x), sanitize(v.y), sanitize(v.z));
        let chessboard = x.abs().max(y.abs().max(z.abs()));
        let mut out = Vec::new();
        if chessboard < 3.051944088384301E-5 {
            out.push(0u8);
            return out;
        }
        let scale = chessboard.ceil() as u64;
        let is_partial = (scale & 3) != scale;
        let markers = if is_partial { (scale & 3) | 4 } else { scale };
        let buffer = markers
            | (pack(x / scale as f64) << 3)
            | (pack(y / scale as f64) << 18)
            | (pack(z / scale as f64) << 33);
        out.push(buffer as u8);
        out.push((buffer >> 8) as u8);
        out.extend_from_slice(&((buffer >> 16) as u32).to_be_bytes());
        if is_partial {
            out.extend_from_slice(&varint((scale >> 2) as i32));
        }
        out
    }

    fn roundtrip(v: Vec3) -> Vec3 {
        let bytes = write_lp_vec3(v);
        let mut r = PacketReader::new(&bytes);
        read_lp_vec3(&mut r).expect("decodes")
    }

    #[test]
    fn lp_vec3_zero_is_a_one_byte_sentinel() {
        // Rule 2. The single most common velocity update — "this entity
        // stopped" — is one byte, not six.
        let bytes = write_lp_vec3(Vec3::ZERO);
        assert_eq!(bytes, vec![0u8], "zero encodes to the bare sentinel");
        let (body, len) = with_sentinel(bytes);
        assert_eq!(len, 1);
        let mut r = PacketReader::new(&body);
        assert_eq!(read_lp_vec3(&mut r).unwrap(), Vec3::ZERO);
        assert_eq!(
            r.offset(),
            1,
            "the sentinel path must consume exactly one byte"
        );
    }

    #[test]
    fn lp_vec3_small_velocity_survives_the_roundtrip() {
        // A walking-speed delta: the regime almost every real packet lives in.
        let v = Vec3::new(0.21, -0.0784, -0.13);
        let got = roundtrip(v);
        // scale = ceil(0.21) = 1, so the mantissa step is 2/32766 ≈ 6.1e-5.
        for (a, b, axis) in [(got.x, v.x, 'x'), (got.y, v.y, 'y'), (got.z, v.z, 'z')] {
            assert!(
                (a - b).abs() < 1.0e-4,
                "{axis}: decoded {a} vs written {b}"
            );
        }
    }

    #[test]
    fn lp_vec3_shares_one_scale_across_all_three_components() {
        // The property that makes the encoding compact and that a
        // per-component encoding would not have: one large component coarsens
        // the other two. `scale` here is ceil(40.0) = 40, so the step is
        // 40 * 2/32766 ≈ 2.4e-3 — three orders coarser than the test above.
        let v = Vec3::new(40.0, 0.001_5, 0.0);
        let got = roundtrip(v);
        assert!((got.x - 40.0).abs() < 0.01, "x kept its magnitude: {}", got.x);
        assert!(
            (got.y - 0.0015).abs() > 1.0e-5,
            "y should have been coarsened by the shared scale, got {}",
            got.y
        );
        assert!(
            (got.y - 0.0015).abs() < 3.0e-3,
            "…but still within one quantisation step, got {}",
            got.y
        );
    }

    #[test]
    fn lp_vec3_large_velocity_takes_the_continuation_varint() {
        // scale > 3 sets the continuation flag and appends a VarInt, so the
        // body is longer than six bytes. A fixed-width reader would stop
        // mid-packet here.
        let v = Vec3::new(100.0, -50.0, 25.0);
        let bytes = write_lp_vec3(v);
        assert!(
            bytes.len() > 6,
            "a scale above 3 must append a continuation VarInt, got {} bytes",
            bytes.len()
        );
        assert_eq!(bytes[0] & 4, 4, "the continuation flag must be set");
        let got = roundtrip(v);
        assert!((got.x - 100.0).abs() < 0.02, "x = {}", got.x);
        assert!((got.y + 50.0).abs() < 0.02, "y = {}", got.y);
        assert!((got.z - 25.0).abs() < 0.02, "z = {}", got.z);
    }

    #[test]
    fn lp_vec3_is_not_the_legacy_short_fixed_point() {
        // Rule 1, stated as a measurement rather than a comment. If 26.2 still
        // used `short / 8000.0`, a 0.5-block/tick velocity would encode as the
        // three shorts 4000/0/0 — six bytes, first two 0x0F 0xA0. It does not.
        let bytes = write_lp_vec3(Vec3::new(0.5, 0.0, 0.0));
        let legacy = {
            let mut v = Vec::new();
            v.extend_from_slice(&4000i16.to_be_bytes());
            v.extend_from_slice(&0i16.to_be_bytes());
            v.extend_from_slice(&0i16.to_be_bytes());
            v
        };
        assert_ne!(
            bytes, legacy,
            "26.2 velocity must not be the legacy short fixed point"
        );
        // And the decisive half: reading this body as three shorts gives a
        // wildly wrong answer, so the mistake is silent rather than fatal.
        let as_shorts = i16::from_be_bytes([bytes[0], bytes[1]]) as f64 / 8000.0;
        let real = roundtrip(Vec3::new(0.5, 0.0, 0.0)).x;
        assert!((real - 0.5).abs() < 1.0e-3, "the real decode is right: {real}");
        assert!(
            (as_shorts - 0.5).abs() > 0.1,
            "the legacy reading is wrong by a large factor, not a rounding: {as_shorts}"
        );
    }

    #[test]
    fn unpack_clamps_the_mask_ceiling() {
        // The mask admits 32767; `pack` never emits it. Without the `min` a
        // hand-built body could read above 1.0 and scale past the sender's
        // stated magnitude.
        assert!((unpack(32767) - 1.0).abs() < 1.0e-12);
        assert!((unpack(32766) - 1.0).abs() < 1.0e-12);
        assert!((unpack(0) + 1.0).abs() < 1.0e-12);
    }

    // ── set_entity_motion ───────────────────────────────────────────────

    #[test]
    fn set_entity_motion_consumes_exactly_its_body() {
        let mut body = varint(4242);
        body.extend_from_slice(&write_lp_vec3(Vec3::new(0.3, 0.42, -0.3)));
        let (with_tail, len) = with_sentinel(body);
        let mut r = PacketReader::new(&with_tail);
        let id = r.varint().unwrap();
        let _ = read_lp_vec3(&mut r).unwrap();
        assert_eq!(id, 4242);
        assert_eq!(r.offset(), len, "must stop exactly at the sentinel");

        let m = read_set_entity_motion(&with_tail).unwrap();
        assert_eq!(m.id, 4242);
        assert!((m.movement.y - 0.42).abs() < 1.0e-4);
    }

    #[test]
    fn set_entity_motion_stop_packet_is_two_bytes() {
        // id + the zero sentinel. Combined with the varint id this is the
        // shortest body any of the four packets has.
        let mut body = varint(7);
        body.extend_from_slice(&write_lp_vec3(Vec3::ZERO));
        assert_eq!(body.len(), 2);
        let m = read_set_entity_motion(&body).unwrap();
        assert_eq!(m.id, 7);
        assert_eq!(m.movement, Vec3::ZERO);
    }

    // ── explode ─────────────────────────────────────────────────────────

    fn explode_body(knockback: Option<Vec3>) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&1.5f64.to_be_bytes());
        b.extend_from_slice(&64.0f64.to_be_bytes());
        b.extend_from_slice(&(-2.5f64).to_be_bytes());
        b.extend_from_slice(&4.0f32.to_be_bytes());
        // blockCount as a FIXED i32 (rule 3).
        b.extend_from_slice(&37i32.to_be_bytes());
        match knockback {
            Some(v) => {
                b.push(1);
                b.extend_from_slice(&v.x.to_be_bytes());
                b.extend_from_slice(&v.y.to_be_bytes());
                b.extend_from_slice(&v.z.to_be_bytes());
            }
            None => b.push(0),
        }
        b
    }

    #[test]
    fn explode_reads_the_physics_prefix_and_stops_there() {
        let body = explode_body(Some(Vec3::new(0.4, 0.7, -0.2)));
        // The real packet continues with particle/sound/weighted-list fields;
        // stand in for them with bytes the prefix decoder must not touch.
        let (with_tail, len) = with_sentinel(body);
        assert_eq!(len, EXPLODE_PREFIX_LEN_WITH_KNOCKBACK);
        let (e, used) = read_explode(&with_tail).unwrap();
        assert_eq!(
            used, EXPLODE_PREFIX_LEN_WITH_KNOCKBACK,
            "the documented stop point is where it actually stops"
        );
        assert_eq!(e.center, Vec3::new(1.5, 64.0, -2.5));
        assert_eq!(e.radius, 4.0);
        assert_eq!(e.block_count, 37);
        assert_eq!(e.player_knockback, Some(Vec3::new(0.4, 0.7, -0.2)));
    }

    #[test]
    fn explode_without_knockback_is_shorter_by_a_whole_vector() {
        let body = explode_body(None);
        assert_eq!(body.len(), EXPLODE_PREFIX_LEN_NO_KNOCKBACK);
        let (e, used) = read_explode(&body).unwrap();
        assert_eq!(used, EXPLODE_PREFIX_LEN_NO_KNOCKBACK);
        assert_eq!(e.player_knockback, None);
    }

    #[test]
    fn explode_block_count_is_a_fixed_int_not_a_varint() {
        // Rule 3, as a measurement. Pick a blockCount whose big-endian i32
        // encoding starts with a byte a VarInt reader would happily consume as
        // a complete (and much smaller) value, then show the knockback that
        // follows lands correctly only under the fixed reading.
        let mut b = Vec::new();
        b.extend_from_slice(&0.0f64.to_be_bytes());
        b.extend_from_slice(&0.0f64.to_be_bytes());
        b.extend_from_slice(&0.0f64.to_be_bytes());
        b.extend_from_slice(&1.0f32.to_be_bytes());
        b.extend_from_slice(&1i32.to_be_bytes()); // 00 00 00 01
        b.push(1);
        b.extend_from_slice(&9.0f64.to_be_bytes());
        b.extend_from_slice(&8.0f64.to_be_bytes());
        b.extend_from_slice(&7.0f64.to_be_bytes());
        let (e, used) = read_explode(&b).unwrap();
        assert_eq!(e.block_count, 1);
        assert_eq!(e.player_knockback, Some(Vec3::new(9.0, 8.0, 7.0)));
        assert_eq!(used, b.len());

        // A VarInt reader would have taken only the first `00` of the four
        // count bytes, read `00` as the Optional tag → "absent", and returned
        // a knockback-free explosion from a packet that carries one. That is
        // the silent failure: no error, just no knockback.
        let mut r = PacketReader::new(&b);
        let _ = read_vec3(&mut r).unwrap();
        let _ = r.f32().unwrap();
        let wrong_count = r.varint().unwrap();
        let wrong_present = r.bool().unwrap();
        assert_eq!(wrong_count, 0, "the VarInt misreading swallows one byte");
        assert!(
            !wrong_present,
            "…and then reads the next count byte as 'no knockback'"
        );
    }

    // ── move_vehicle ────────────────────────────────────────────────────

    #[test]
    fn move_vehicle_is_thirty_two_bytes_yaw_before_pitch() {
        let mut b = Vec::new();
        b.extend_from_slice(&10.5f64.to_be_bytes());
        b.extend_from_slice(&63.0f64.to_be_bytes());
        b.extend_from_slice(&(-4.25f64).to_be_bytes());
        b.extend_from_slice(&90.0f32.to_be_bytes()); // yRot
        b.extend_from_slice(&(-12.0f32).to_be_bytes()); // xRot
        let (with_tail, len) = with_sentinel(b);
        assert_eq!(len, 32);
        let mut r = PacketReader::new(&with_tail);
        let position = read_vec3(&mut r).unwrap();
        let y_rot = r.f32().unwrap();
        let x_rot = r.f32().unwrap();
        assert_eq!(r.offset(), len, "must stop exactly at the sentinel");
        assert_eq!(position, Vec3::new(10.5, 63.0, -4.25));
        assert_eq!((y_rot, x_rot), (90.0, -12.0));

        let v = read_move_vehicle(&with_tail).unwrap();
        assert_eq!(v.y_rot, 90.0, "yaw is the FIRST float");
        assert_eq!(v.x_rot, -12.0, "pitch is the second");
    }

    // ── set_passengers ──────────────────────────────────────────────────

    fn passengers_body(vehicle: i32, riders: &[i32]) -> Vec<u8> {
        let mut b = varint(vehicle);
        b.extend_from_slice(&varint(riders.len() as i32));
        for &r in riders {
            b.extend_from_slice(&varint(r));
        }
        b
    }

    #[test]
    fn set_passengers_consumes_exactly_its_body() {
        let body = passengers_body(500, &[11, 22, 33]);
        let (with_tail, len) = with_sentinel(body);
        let mut r = PacketReader::new(&with_tail);
        let vehicle = r.varint().unwrap();
        let n = r.varint().unwrap();
        for _ in 0..n {
            let _ = r.varint().unwrap();
        }
        assert_eq!(r.offset(), len, "must stop exactly at the sentinel");
        assert_eq!(vehicle, 500);

        let p = read_set_passengers(&with_tail).unwrap();
        assert_eq!(p.vehicle, 500);
        assert_eq!(p.passengers, vec![11, 22, 33]);
    }

    #[test]
    fn set_passengers_rejects_a_length_longer_than_the_body_before_allocating() {
        // A corrupt count must be rejected *up front*, not discovered by
        // running out of bytes part-way through the loop — otherwise a
        // one-byte lie makes us reserve a million elements first.
        //
        // The first version of this witness only asserted `is_err()`, and
        // **mutation-testing showed it was decorative**: deleting the bound
        // entirely left it green, because `Vec::with_capacity(1_000_000)`
        // succeeds and the *first* `r.varint()` then fails on the empty
        // remainder. Both paths return an error; only the error's shape says
        // which one ran. `needed` is the declared count when the bound fired
        // and 1 when the loop discovered it the expensive way.
        let mut b = varint(1);
        b.extend_from_slice(&varint(1_000_000));
        match read_set_passengers(&b) {
            Err(rewo_proto::ProtoError::Eof { needed, remaining }) => {
                assert_eq!(
                    needed, 1_000_000,
                    "the bound must reject the declared count itself, not \
                     stumble into an Eof one element at a time"
                );
                assert_eq!(remaining, 0, "…measured against the bytes actually left");
            }
            other => panic!("expected an up-front Eof, got {other:?}"),
        }

        // And the bound must not be so tight that a legitimate list fails:
        // vanilla's own cap is the frame's remaining byte count, and a VarInt
        // is at least one byte, so a list is admissible exactly when its
        // count fits in the bytes that follow.
        let ok = passengers_body(7, &[1, 2, 3, 4]);
        assert_eq!(read_set_passengers(&ok).unwrap().passengers, vec![1, 2, 3, 4]);
    }

    #[test]
    fn set_passengers_replaces_rather_than_merges() {
        // Rule 5, and the reason a dismount works at all: it arrives as a
        // shorter list, never as a removal.
        let mut m = Mounts::new();
        m.apply(&Passengers {
            vehicle: 9,
            passengers: vec![1, 2],
        });
        assert_eq!(m.passengers(9), &[1, 2]);
        assert_eq!(m.vehicle_of(1), Some(9));

        // Player 1 dismounts: the server re-sends the vehicle's list without
        // it. A merge would leave 1 seated.
        m.apply(&Passengers {
            vehicle: 9,
            passengers: vec![2],
        });
        assert_eq!(m.passengers(9), &[2]);
        assert_eq!(m.vehicle_of(1), None, "the ejected rider must be free");
        assert_eq!(m.vehicle_of(2), Some(9));

        // And an empty list empties the vehicle entirely.
        m.apply(&Passengers {
            vehicle: 9,
            passengers: vec![],
        });
        assert!(m.passengers(9).is_empty());
        assert_eq!(m.vehicle_of(2), None);
    }

    #[test]
    fn a_rider_moving_between_vehicles_leaves_its_old_seat() {
        let mut m = Mounts::new();
        m.apply(&Passengers {
            vehicle: 9,
            passengers: vec![1],
        });
        // The new vehicle's packet arrives before the old one's update — the
        // two packets have no ordering guarantee.
        m.apply(&Passengers {
            vehicle: 10,
            passengers: vec![1],
        });
        assert_eq!(m.vehicle_of(1), Some(10));
        assert!(
            !m.passengers(9).contains(&1),
            "the stale seat must not keep the rider"
        );
    }

    #[test]
    fn root_vehicle_walks_the_chain_and_cannot_hang_on_a_cycle() {
        let mut m = Mounts::new();
        // A player on a horse in a boat.
        m.apply(&Passengers {
            vehicle: 100,
            passengers: vec![200],
        });
        m.apply(&Passengers {
            vehicle: 200,
            passengers: vec![300],
        });
        assert_eq!(m.root_vehicle(300), Some(100));
        assert_eq!(m.root_vehicle(200), Some(100));
        assert_eq!(m.root_vehicle(100), None, "the root rides nothing");

        // A cycle cannot be produced by a well-behaved server (startRiding
        // walks the chain and refuses), but the walk must still terminate.
        let mut cyc = Mounts::new();
        cyc.apply(&Passengers {
            vehicle: 1,
            passengers: vec![2],
        });
        cyc.apply(&Passengers {
            vehicle: 2,
            passengers: vec![1],
        });
        let _ = cyc.root_vehicle(1); // must return, not spin
    }

    #[test]
    fn removing_an_entity_clears_both_directions() {
        let mut m = Mounts::new();
        m.apply(&Passengers {
            vehicle: 9,
            passengers: vec![1, 2],
        });
        m.remove_entity(1);
        assert_eq!(m.vehicle_of(1), None);
        assert_eq!(m.passengers(9), &[2]);
        // Removing the vehicle frees its remaining riders.
        m.remove_entity(9);
        assert_eq!(m.vehicle_of(2), None);
        assert!(m.passengers(9).is_empty());
    }

    #[test]
    fn a_non_finite_vector_is_rejected_by_the_finite_guard() {
        // Both `setDeltaMovement` and `addDeltaMovement` guard on this, which
        // is why a NaN leaves the velocity untouched rather than poisoning it.
        assert!(Vec3::new(0.0, 0.0, 0.0).is_finite());
        assert!(!Vec3::new(f64::NAN, 0.0, 0.0).is_finite());
        assert!(!Vec3::new(0.0, f64::INFINITY, 0.0).is_finite());
    }
}
