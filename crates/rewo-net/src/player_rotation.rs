//! The two packets that turn the local player's head (M76):
//! `player_rotation` (73) and `player_look_at` (71).
//!
//! The decode half. The arithmetic they drive — `calculateAbsolute`'s rotation
//! clause, `Entity.lookAt`, `Mth.atan2` and the two rotation setters — lives in
//! [`rewo_world::rotation`], the same decode/state split M75 used for the
//! abilities. Both are `REWO_PACKET_COVERAGE.md` class **A**: they write two
//! floats onto the local player and a witness can prove it without drawing
//! anything.
//!
//! `player_position` (72) has been handled since M3 and these two never were,
//! which is the asymmetry §3 of that document ranks second: a server that moves
//! your body works and one that turns your head does nothing at all, so the
//! natural diagnosis is "teleports work, so it isn't the teleport path".
//!
//! # `player_rotation`'s flags are two booleans, not a bitfield
//!
//! `ClientboundPlayerRotationPacket` is a four-field
//! `StreamCodec.composite` — `FLOAT yRot, BOOL relativeY, FLOAT xRot, BOOL
//! relativeX` — ten fixed bytes with each flag *after* the float it qualifies.
//! It does **not** carry `Relative.SET_STREAM_CODEC`'s packed int, which is
//! what its positional twin uses and what a reader written from the twin would
//! expect. That reader would consume the yaw's four bytes as the mask.
//!
//! The `Set<Relative>` does exist, one layer up: `handleRotatePlayer` builds it
//! with `Relative.rotation(relativeY, relativeX)` before calling
//! `calculateAbsolute`. So the two packets share their *semantics* and not
//! their *layout*.
//!
//! # `player_look_at`'s trailing fields are conditional
//!
//! ```java
//! this.fromAnchor = input.readEnum(EntityAnchorArgument.Anchor.class);
//! this.x = input.readDouble(); this.y = …; this.z = …;
//! this.atEntity = input.readBoolean();
//! if (this.atEntity) { this.entity = input.readVarInt();
//!                      this.toAnchor = input.readEnum(…); }
//! ```
//!
//! so the body is either 26 bytes (a one-byte anchor VarInt, three doubles, the
//! flag) or 26 plus a VarInt entity and a VarInt anchor. Reading the trailing
//! pair unconditionally desyncs the stream on the common
//! `/teleport … facing <x y z>` form, which is the one that carries no entity.
//!
//! # An unknown target entity is not a no-op
//!
//! `getPosition` falls back to the packet's **own** `x/y/z` when
//! `level.getEntity(entity)` is null. Those coordinates are not filler: the
//! sending constructor sets them to `toAnchor.apply(entity)` at send time, so
//! the fallback is the correct anchored point, stale only by however far the
//! entity has moved since. Treating an unresolvable entity as "do nothing"
//! would drop a rotation vanilla performs.
//!
//! That is also why [`PlayerLookAt::position`]'s resolver may decline: see
//! [`PlayerLookAt::position`] for what Rewo can and cannot anchor.
//!
//! # Only one of the two answers the server
//!
//! `handleRotatePlayer` ends with
//! `send(new ServerboundMovePlayerPacket.Rot(getYRot(), getXRot(), false, false))`
//! — unconditionally, before any tick. `handleLookAt` sends nothing at all and
//! lets the next tick's ordinary movement report carry the new angles. Hence
//! [`RotationRoute`], which reports *which* packet matched so the session can
//! reproduce that asymmetry.
//!
//! ## Ground truth (bundled 26.2 decompile, `%APPDATA%/EwoClient/rewo/26.2/
//! decompiled/`)
//!
//! - `net/minecraft/network/protocol/game/ClientboundPlayerRotationPacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundPlayerLookAtPacket.java`
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` —
//!   `handleRotatePlayer`, `handleLookAt`
//! - `net/minecraft/commands/arguments/EntityAnchorArgument.java` — `Anchor`
//! - `net/minecraft/network/FriendlyByteBuf.java` — `readEnum` is
//!   `values()[readVarInt()]`

use rewo_proto::reader::PacketReader;
use rewo_proto::Result;
use rewo_world::rotation;

/// `EntityAnchorArgument.Anchor`, re-exported so a caller wiring the
/// [`PlayerLookAt::position`] resolver does not need a second import path for
/// the one type that resolver is keyed on.
pub use rewo_world::rotation::Anchor;

/// `ClientboundPlayerRotationPacket` — ten fixed bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerRotation {
    /// The yaw, or the yaw *delta* when [`relative_y`](Self::relative_y).
    pub y_rot: f32,
    /// Whether `y_rot` composes with the player's current yaw.
    pub relative_y: bool,
    /// The pitch, or the pitch delta when [`relative_x`](Self::relative_x).
    pub x_rot: f32,
    pub relative_x: bool,
}

impl PlayerRotation {
    pub fn parse(body: &[u8]) -> Result<PlayerRotation> {
        let mut r = PacketReader::new(body);
        // The interleaving is the point: flag, then float, then flag. Reading
        // the two floats first and the two bools after decodes every packet
        // without error and gets the relativity backwards whenever the flags
        // differ.
        let y_rot = r.f32()?;
        let relative_y = r.bool()?;
        let x_rot = r.f32()?;
        let relative_x = r.bool()?;
        Ok(PlayerRotation {
            y_rot,
            relative_y,
            x_rot,
            relative_x,
        })
    }

    /// `handleRotatePlayer`'s body. Returns `(wrote_yaw, wrote_pitch)`; either
    /// is false when the packet carried a non-finite value, which vanilla
    /// discards rather than clamping.
    pub fn apply_to(&self, yaw: &mut f32, pitch: &mut f32) -> (bool, bool) {
        rotation::apply_relative_rotation(
            yaw,
            pitch,
            self.y_rot,
            self.relative_y,
            self.x_rot,
            self.relative_x,
        )
    }
}

/// `ClientboundPlayerLookAtPacket`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerLookAt {
    /// Which of the **player's** anchors the ray leaves from.
    pub from_anchor: Anchor,
    /// The target point. When [`at_entity`](Self::at_entity) this is the
    /// sender's snapshot of `to_anchor.apply(entity)`, not a placeholder.
    pub pos: [f64; 3],
    pub at_entity: bool,
    /// Meaningful only when `at_entity`; vanilla stores `0` otherwise.
    pub entity: i32,
    /// Which of the *target's* anchors to aim at. `None` unless `at_entity`.
    pub to_anchor: Option<Anchor>,
}

impl PlayerLookAt {
    pub fn parse(body: &[u8]) -> Result<PlayerLookAt> {
        let mut r = PacketReader::new(body);
        let from_anchor = read_anchor(&mut r)?;
        let x = r.f64()?;
        let y = r.f64()?;
        let z = r.f64()?;
        let at_entity = r.bool()?;
        let (entity, to_anchor) = if at_entity {
            let e = r.varint()?;
            (e, Some(read_anchor(&mut r)?))
        } else {
            // Vanilla's own `else` arm writes exactly these.
            (0, None)
        };
        Ok(PlayerLookAt {
            from_anchor,
            pos: [x, y, z],
            at_entity,
            entity,
            to_anchor,
        })
    }

    /// `ClientboundPlayerLookAtPacket.getPosition(level)`.
    ///
    /// `resolve(entity_id, to_anchor)` stands in for
    /// `to_anchor.apply(level.getEntity(id))`. It returns `None` for an entity
    /// the client cannot anchor, which takes vanilla's null branch: the
    /// packet's own carried coordinates.
    ///
    /// **What Rewo can resolve, and the one thing it cannot.** A `FEET` anchor
    /// is the entity's position, which [`rewo_world::entities::EntityTable`]
    /// has exactly. An `EYES` anchor needs `Entity.getEyeHeight()`, which is
    /// `EntityDimensions.eyeHeight` — a per-type field Rewo does not model
    /// (`entity_pick::DimensionInputs` carries width and height, and eye height
    /// is neither `height` nor a fixed fraction of it for every type). Rather
    /// than invent one, the production resolver declines, which lands on the
    /// carried coordinates — and those *are* the anchored eye point, computed
    /// by the server from that entity. The approximation is therefore purely
    /// one of **staleness**, bounded by how far the target moved between the
    /// server building the packet and the client applying it, and it is the
    /// same branch vanilla takes for an entity it has not been told about.
    ///
    /// Vanilla's signature is `@Nullable` but neither branch can return null in
    /// 26.2; `handleLookAt`'s `if (pos != null)` guard is vestigial.
    pub fn position(
        &self,
        resolve: impl FnOnce(i32, Anchor) -> Option<[f64; 3]>,
    ) -> [f64; 3] {
        // `this.toAnchor` is non-null exactly when `atEntity` — [`Self::parse`]
        // sets them together, as vanilla's two constructors do — so the
        // `at_entity` test and the `and_then` are **redundant with each other**
        // for any value the wire can produce. Kept because the `if
        // (this.atEntity)` is what `getPosition` is, and because the fields are
        // public: a hand-built value with `at_entity: false, to_anchor:
        // Some(..)` separates them, and vanilla's answer for that state is the
        // carried coordinates. The mutation battery found the redundancy by
        // deleting this test and observing nothing change; the invariant is
        // witnessed in the gate instead.
        if self.at_entity {
            self.to_anchor
                .and_then(|anchor| resolve(self.entity, anchor))
                .unwrap_or(self.pos)
        } else {
            self.pos
        }
    }

    /// `handleLookAt`: `minecraft.player.lookAt(fromAnchor, getPosition(level))`.
    ///
    /// `player_pos` is the player's feet and `player_eye_height` its
    /// `getEyeHeight()`; the `from` anchor is applied here because vanilla
    /// applies it inside `Entity.lookAt`, against the *viewer*.
    pub fn apply_to(
        &self,
        yaw: &mut f32,
        pitch: &mut f32,
        player_pos: [f64; 3],
        player_eye_height: f64,
        resolve: impl FnOnce(i32, Anchor) -> Option<[f64; 3]>,
    ) -> bool {
        let from = self.from_anchor.apply(player_pos, player_eye_height);
        rotation::apply_look_at(yaw, pitch, from, self.position(resolve))
    }
}

/// `FriendlyByteBuf.readEnum` = `values()[readVarInt()]`.
///
/// An out-of-range ordinal is an `ArrayIndexOutOfBoundsException` in vanilla,
/// so it is a decode error here — not `Anchor::Feet`. The distinction is real:
/// `ByIdMap.continuous(…, ZERO)` (M65) *would* give the zero value and
/// `…, WRAP)` (M74) would give `values()[floorMod(id, 2)]`, so an id of 2 has
/// three different plausible readings and only this one is correct for a
/// `readEnum` field.
fn read_anchor(r: &mut PacketReader) -> Result<Anchor> {
    let ordinal = r.varint()?;
    // The same shape M65's `RenderType` uses for its own `readEnum` field.
    Anchor::from_ordinal(ordinal).ok_or(rewo_proto::ProtoError::LengthOutOfRange {
        what: "look-at entity anchor ordinal",
        len: ordinal as i64,
        max: 1,
    })
}

/// Which of the two rotation packets an id is — and, because the two differ in
/// whether they answer the server, what the session owes afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationRoute {
    /// Not one of ours; the caller keeps looking.
    NoMatch,
    /// `player_rotation`. `handleRotatePlayer` answers it immediately with a
    /// `ServerboundMovePlayerPacket.Rot`, **unconditionally** — including when
    /// the packet was a no-op, and without waiting for the next tick.
    Rotation,
    /// `player_look_at`. `handleLookAt` sends nothing; the next ordinary
    /// movement report carries the new angles.
    LookAt,
}

/// Everything [`route_player_rotation`] needs about the local player.
///
/// A struct rather than five positional arguments because two of them are
/// `f64` triples and floats, and a transposed pair would be a silent
/// mis-aim rather than a type error.
pub struct LocalRotation<'a> {
    /// The player's feet — `Entity.position()`.
    pub pos: [f64; 3],
    /// `Entity.getEyeHeight()`; [`rewo_world::physics::EYE_HEIGHT`] for a
    /// standing player.
    pub eye_height: f64,
    pub yaw: &'a mut f32,
    pub pitch: &'a mut f32,
}

/// The clientbound-play dispatch seam for the two rotation packets.
///
/// Returns which packet matched — **not** whether the body decoded. A body
/// that fails to parse still reports its kind, exactly as the other seams
/// return `true` on an undecodable body, because the id did match and the
/// caller must not go on to test it against anything else.
pub fn route_player_rotation(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    local: LocalRotation<'_>,
    resolve: impl FnOnce(i32, Anchor) -> Option<[f64; 3]>,
) -> RotationRoute {
    if id == ids.cb_play_player_rotation {
        match PlayerRotation::parse(body) {
            Ok(p) => {
                p.apply_to(local.yaw, local.pitch);
            }
            Err(err) => log::debug!("net: player_rotation decode: {err}"),
        }
        RotationRoute::Rotation
    } else if id == ids.cb_play_player_look_at {
        match PlayerLookAt::parse(body) {
            Ok(p) => {
                p.apply_to(local.yaw, local.pitch, local.pos, local.eye_height, resolve);
            }
            Err(err) => log::debug!("net: player_look_at decode: {err}"),
        }
        RotationRoute::LookAt
    } else {
        RotationRoute::NoMatch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rotation_body(y_rot: f32, rel_y: bool, x_rot: f32, rel_x: bool) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&y_rot.to_be_bytes());
        b.push(rel_y as u8);
        b.extend_from_slice(&x_rot.to_be_bytes());
        b.push(rel_x as u8);
        b
    }

    fn look_at_body(
        from: i32,
        pos: [f64; 3],
        at_entity: Option<(i32, i32)>,
    ) -> Vec<u8> {
        let mut b = Vec::new();
        b.push(from as u8); // a VarInt of 0 or 1 is one byte
        for v in pos {
            b.extend_from_slice(&v.to_be_bytes());
        }
        b.push(at_entity.is_some() as u8);
        if let Some((entity, to)) = at_entity {
            rewo_proto::varint::write_varint(&mut b, entity);
            b.push(to as u8);
        }
        b
    }

    /// The layout is interleaved, and the body is exactly ten bytes.
    ///
    /// MUTATION: reading `f32, f32, bool, bool` — the "it's like the positional
    /// twin" shape. The flags below differ, so the mutation reports
    /// `relative_y = false, relative_x = ?` from bytes that are float payload.
    #[test]
    fn rotation_layout_is_float_bool_float_bool() {
        let body = rotation_body(90.0, true, -30.0, false);
        assert_eq!(body.len(), 10);
        let p = PlayerRotation::parse(&body).unwrap();
        assert_eq!(p.y_rot, 90.0);
        assert!(p.relative_y);
        assert_eq!(p.x_rot, -30.0);
        assert!(!p.relative_x);

        // And the other way round, so a reader that hard-codes either flag
        // fails one of the two.
        let p = PlayerRotation::parse(&rotation_body(1.0, false, 2.0, true)).unwrap();
        assert!(!p.relative_y);
        assert!(p.relative_x);
    }

    /// MUTATION: reading the relative bits out of an `i32` mask
    /// (`Relative.SET_STREAM_CODEC`). A nine-byte body is one byte short of a
    /// mask-shaped read and must fail rather than reading past the end.
    #[test]
    fn a_truncated_rotation_body_is_an_error() {
        let mut body = rotation_body(0.0, false, 0.0, false);
        body.pop();
        assert!(PlayerRotation::parse(&body).is_err());
    }

    /// The trailing entity pair is present only when the flag is set — the
    /// difference between the two `/teleport … facing` forms.
    ///
    /// MUTATION: reading `entity` and `to_anchor` unconditionally. The
    /// point-form body below is exactly 26 bytes with nothing after it, so the
    /// mutation errors on the packet that is by far the more common of the two.
    ///
    /// (The length assertion earned its place immediately: the module docs
    /// first said 25, and this is what corrected them. 1 + 3×8 + 1 = 26.)
    #[test]
    fn look_at_trailing_fields_are_conditional() {
        let body = look_at_body(1, [1.0, 2.0, 3.0], None);
        assert_eq!(body.len(), 26);
        let p = PlayerLookAt::parse(&body).unwrap();
        assert_eq!(p.from_anchor, Anchor::Eyes);
        assert_eq!(p.pos, [1.0, 2.0, 3.0]);
        assert!(!p.at_entity);
        assert_eq!(p.entity, 0, "vanilla's else arm writes 0");
        assert_eq!(p.to_anchor, None);

        let p = PlayerLookAt::parse(&look_at_body(0, [4.0, 5.0, 6.0], Some((77, 1)))).unwrap();
        assert_eq!(p.from_anchor, Anchor::Feet);
        assert!(p.at_entity);
        assert_eq!(p.entity, 77);
        assert_eq!(p.to_anchor, Some(Anchor::Eyes));
    }

    /// MUTATION: giving `read_anchor` a `ByIdMap`-style default. `readEnum` is
    /// an array index and ordinal 2 is an `ArrayIndexOutOfBoundsException`.
    #[test]
    fn an_out_of_range_anchor_is_an_error_not_a_default() {
        let mut body = look_at_body(0, [0.0, 0.0, 0.0], None);
        body[0] = 2;
        assert!(PlayerLookAt::parse(&body).is_err());
        // The *trailing* anchor is read by the same function, so it errors too.
        let mut body = look_at_body(0, [0.0, 0.0, 0.0], Some((1, 0)));
        let last = body.len() - 1;
        body[last] = 7;
        assert!(PlayerLookAt::parse(&body).is_err());
    }

    /// An unresolvable entity falls back to the packet's carried coordinates,
    /// which are the server's snapshot of the anchored point — not to "do
    /// nothing", and not to the origin.
    ///
    /// MUTATION: returning `None`/skipping the apply when the entity is
    /// unknown. The first assertion below then reads the origin or leaves the
    /// rotation untouched.
    #[test]
    fn an_unknown_entity_uses_the_carried_coordinates() {
        let p = PlayerLookAt::parse(&look_at_body(0, [9.0, 8.0, 7.0], Some((5, 0)))).unwrap();
        assert_eq!(p.position(|_, _| None), [9.0, 8.0, 7.0]);
        // A resolvable one overrides them, so the fallback is not simply
        // ignoring the resolver.
        assert_eq!(p.position(|_, _| Some([1.0, 1.0, 1.0])), [1.0, 1.0, 1.0]);
    }

    /// The resolver is consulted **only** for the entity form; the point form
    /// must not reach it even if a caller would answer.
    ///
    /// MUTATION: dropping the `at_entity` test in `position`. The point form
    /// would then take the resolver's answer and aim somewhere else entirely.
    #[test]
    fn the_point_form_never_consults_the_resolver() {
        let p = PlayerLookAt::parse(&look_at_body(0, [3.0, 3.0, 3.0], None)).unwrap();
        assert_eq!(p.position(|_, _| Some([0.0, 0.0, 0.0])), [3.0, 3.0, 3.0]);
        // And it is handed the right anchor when it *is* consulted.
        let p = PlayerLookAt::parse(&look_at_body(0, [0.0, 0.0, 0.0], Some((5, 1)))).unwrap();
        let mut seen = None;
        let _ = p.position(|id, anchor| {
            seen = Some((id, anchor));
            None
        });
        assert_eq!(seen, Some((5, Anchor::Eyes)));
    }

    /// The `from` anchor is the **player's**, so the same packet aimed at the
    /// same point gives a different pitch from the feet and from the eyes.
    ///
    /// MUTATION: applying `from_anchor` to the target instead of the viewer, or
    /// ignoring it.
    #[test]
    fn apply_uses_the_players_own_anchor() {
        let feet_pkt = PlayerLookAt::parse(&look_at_body(0, [0.0, 10.0, 10.0], None)).unwrap();
        let eyes_pkt = PlayerLookAt::parse(&look_at_body(1, [0.0, 10.0, 10.0], None)).unwrap();
        let (mut y1, mut p1) = (0.0f32, 0.0f32);
        let (mut y2, mut p2) = (0.0f32, 0.0f32);
        assert!(feet_pkt.apply_to(&mut y1, &mut p1, [0.0, 0.0, 0.0], 1.62, |_, _| None));
        assert!(eyes_pkt.apply_to(&mut y2, &mut p2, [0.0, 0.0, 0.0], 1.62, |_, _| None));
        assert_eq!(y1, y2, "the yaw does not depend on the vertical anchor");
        assert!(
            p2 > p1,
            "from the eyes the target is less far above: feet {p1}, eyes {p2}"
        );
    }
}
