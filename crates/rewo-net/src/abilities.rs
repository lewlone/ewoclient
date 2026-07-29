//! M75 — the two `player_abilities` packets.
//!
//! Ground truth (bundled 26.2 decompile):
//!
//! - `network/protocol/game/ClientboundPlayerAbilitiesPacket.java`
//! - `network/protocol/game/ServerboundPlayerAbilitiesPacket.java`
//! - `client/multiplayer/ClientPacketListener.java` (`handlePlayerAbilities`)
//! - `server/network/ServerGamePacketListenerImpl.java` (its serverbound twin)
//!
//! The *state* these drive lives in [`rewo_world::abilities`], next to the
//! movement it changes; this module is only the wire, mirroring the way
//! `rewo-net::attributes` sits beside `rewo-world::attributes`.
//!
//! # The body
//!
//! One flags byte then **two floats, flying speed first**:
//!
//! ```text
//! bit 1  invulnerable
//! bit 2  flying
//! bit 4  canFly      (`Abilities.mayfly`)
//! bit 8  instabuild
//! f32    flyingSpeed
//! f32    walkingSpeed
//! ```
//!
//! Fixed nine bytes — nothing here is a var-int, and there is no length prefix.
//!
//! **`mayBuild` is not on the wire.** `Abilities` has five booleans and the
//! packet carries four; `mayBuild` reaches the client only through
//! `GameType.updatePlayerAbilities`. So a decode must not clear it, which is
//! why [`PlayerAbilities::apply_to`] assigns exactly the four it was told.
//!
//! # The serverbound twin is not the same packet with a direction flipped
//!
//! It is **one byte**, and it declares only `FLAG_FLYING = 2` — the client's
//! sole ability claim is "I am flying now". Everything else is the server's to
//! decide, and it does not take our word for even that one: `handlePlayerAbilities`
//! server-side is `flying = packet.isFlying() && player.getAbilities().mayfly`,
//! so a client claiming flight it has not been granted is **ignored, not
//! kicked**. Writing the full nine-byte clientbound body here would desync the
//! stream by eight bytes.

use rewo_proto::reader::PacketReader;
use rewo_proto::writer::PacketWriter;
use rewo_proto::Result;
use rewo_world::abilities::Abilities;

/// `ClientboundPlayerAbilitiesPacket.FLAG_INVULNERABLE`.
pub const FLAG_INVULNERABLE: u8 = 1;
/// `ClientboundPlayerAbilitiesPacket.FLAG_FLYING` — and the *only* bit the
/// serverbound packet declares.
pub const FLAG_FLYING: u8 = 2;
/// `ClientboundPlayerAbilitiesPacket.FLAG_CAN_FLY` — `Abilities.mayfly`. Note
/// the packet's name and the field's name differ; they are the same thing.
pub const FLAG_CAN_FLY: u8 = 4;
/// `ClientboundPlayerAbilitiesPacket.FLAG_INSTABUILD`.
pub const FLAG_INSTABUILD: u8 = 8;

/// A decoded `ClientboundPlayerAbilitiesPacket`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerAbilities {
    pub invulnerable: bool,
    pub flying: bool,
    pub can_fly: bool,
    pub instabuild: bool,
    pub flying_speed: f32,
    pub walking_speed: f32,
}

impl PlayerAbilities {
    /// Decode the nine-byte body.
    ///
    /// Unused flag bits are *ignored*, exactly as vanilla ignores them — it
    /// tests four masks and never looks at the rest of the byte. A future
    /// protocol addition is therefore not a decode failure.
    pub fn parse(body: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(body);
        let bits = r.u8()?;
        Ok(Self {
            invulnerable: bits & FLAG_INVULNERABLE != 0,
            flying: bits & FLAG_FLYING != 0,
            can_fly: bits & FLAG_CAN_FLY != 0,
            instabuild: bits & FLAG_INSTABUILD != 0,
            flying_speed: r.f32()?,
            walking_speed: r.f32()?,
        })
    }

    /// `ClientPacketListener.handlePlayerAbilities`, verbatim: six assignments
    /// into the player's live `Abilities`, and nothing else.
    ///
    /// In particular it does **not** touch `may_build` (absent from the wire),
    /// and it does **not** write `walking_speed` anywhere near the movement
    /// speed — the client's walk speed is `Attributes.MOVEMENT_SPEED`, synced by
    /// its own packet. See [`rewo_world::abilities`] for why that matters.
    pub fn apply_to(&self, a: &mut Abilities) {
        a.flying = self.flying;
        a.instabuild = self.instabuild;
        a.invulnerable = self.invulnerable;
        a.mayfly = self.can_fly;
        a.set_flying_speed(self.flying_speed);
        a.set_walking_speed(self.walking_speed);
    }

    /// The clientbound encoding, for round-trip tests and the gate. Not used on
    /// the wire by a client — only a server writes this direction.
    pub fn encode_body(&self) -> Vec<u8> {
        let mut bits = 0u8;
        if self.invulnerable {
            bits |= FLAG_INVULNERABLE;
        }
        if self.flying {
            bits |= FLAG_FLYING;
        }
        if self.can_fly {
            bits |= FLAG_CAN_FLY;
        }
        if self.instabuild {
            bits |= FLAG_INSTABUILD;
        }
        let mut b = vec![bits];
        b.extend_from_slice(&self.flying_speed.to_be_bytes());
        b.extend_from_slice(&self.walking_speed.to_be_bytes());
        b
    }
}

/// Build the `ServerboundPlayerAbilitiesPacket` the client owes the server
/// after it changes `flying` itself — vanilla's `onUpdateAbilities()`.
///
/// One byte, carrying only [`FLAG_FLYING`].
pub fn serverbound(packet_id: i32, flying: bool) -> PacketWriter {
    let mut p = PacketWriter::packet(packet_id);
    p.u8(if flying { FLAG_FLYING } else { 0 });
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(bits: u8, fly: f32, walk: f32) -> Vec<u8> {
        let mut b = vec![bits];
        b.extend_from_slice(&fly.to_be_bytes());
        b.extend_from_slice(&walk.to_be_bytes());
        b
    }

    /// Each bit in isolation, so a swapped pair cannot pass. The two most
    /// confusable are `flying` (2) and `canFly` (4): a client that read them
    /// the other way round would let a survival player fly and refuse a
    /// creative one.
    #[test]
    fn each_flag_bit_is_isolated() {
        let one = |bits: u8| PlayerAbilities::parse(&body(bits, 0.05, 0.1)).unwrap();
        let a = one(FLAG_INVULNERABLE);
        assert_eq!((a.invulnerable, a.flying, a.can_fly, a.instabuild), (true, false, false, false));
        let a = one(FLAG_FLYING);
        assert_eq!((a.invulnerable, a.flying, a.can_fly, a.instabuild), (false, true, false, false));
        let a = one(FLAG_CAN_FLY);
        assert_eq!((a.invulnerable, a.flying, a.can_fly, a.instabuild), (false, false, true, false));
        let a = one(FLAG_INSTABUILD);
        assert_eq!((a.invulnerable, a.flying, a.can_fly, a.instabuild), (false, false, false, true));
        assert_eq!((FLAG_INVULNERABLE, FLAG_FLYING, FLAG_CAN_FLY, FLAG_INSTABUILD), (1, 2, 4, 8));
    }

    /// Flying speed comes **first**. Swapping the two floats is invisible at the
    /// defaults' *shape* but not at their values, so the fixture uses two that
    /// cannot be confused.
    #[test]
    fn the_floats_are_flying_speed_then_walking_speed() {
        let a = PlayerAbilities::parse(&body(0, 0.25, 0.7)).unwrap();
        assert_eq!(a.flying_speed, 0.25);
        assert_eq!(a.walking_speed, 0.7);
    }

    #[test]
    fn round_trips_through_its_own_encoder() {
        let a = PlayerAbilities {
            invulnerable: true,
            flying: false,
            can_fly: true,
            instabuild: false,
            flying_speed: 0.123,
            walking_speed: 0.456,
        };
        assert_eq!(PlayerAbilities::parse(&a.encode_body()).unwrap(), a);
        assert_eq!(a.encode_body().len(), 9, "one byte plus two f32s");
    }

    /// Unknown high bits are ignored, not rejected.
    #[test]
    fn unused_bits_are_ignored() {
        let a = PlayerAbilities::parse(&body(0xF0 | FLAG_FLYING, 0.05, 0.1)).unwrap();
        assert!(a.flying && !a.invulnerable && !a.can_fly && !a.instabuild);
    }

    #[test]
    fn a_short_body_is_an_error() {
        assert!(PlayerAbilities::parse(&[]).is_err());
        assert!(PlayerAbilities::parse(&[0, 0, 0, 0]).is_err(), "one float short");
        assert!(PlayerAbilities::parse(&body(0, 0.05, 0.1)[..8]).is_err());
    }

    /// `apply_to` writes six fields and must leave `may_build` alone — it is
    /// not on the wire, and clearing it would silently take away block placing.
    #[test]
    fn apply_to_does_not_touch_may_build() {
        let mut a = Abilities::default();
        a.may_build = false;
        PlayerAbilities::parse(&body(0xF, 0.2, 0.3))
            .unwrap()
            .apply_to(&mut a);
        assert!(!a.may_build, "left as it was");
        assert!(a.flying && a.mayfly && a.instabuild && a.invulnerable);
        assert_eq!(a.flying_speed(), 0.2);
        assert_eq!(a.walking_speed(), 0.3);

        // And the other direction: a packet with no bits clears all four.
        PlayerAbilities::parse(&body(0, 0.05, 0.1))
            .unwrap()
            .apply_to(&mut a);
        assert!(!a.flying && !a.mayfly && !a.instabuild && !a.invulnerable);
    }

    /// The serverbound packet is one byte and carries only bit 2.
    #[test]
    fn serverbound_is_one_byte_of_flying_only() {
        // id 0x25 encodes as a single var-int byte, so body = bytes[1..].
        let on = serverbound(0x25, true).into_bytes();
        let off = serverbound(0x25, false).into_bytes();
        assert_eq!(on, vec![0x25, FLAG_FLYING]);
        assert_eq!(off, vec![0x25, 0]);
        assert_eq!(on.len(), 2, "id + exactly one payload byte");
        // The clientbound body for the same state is eight bytes longer —
        // writing that here would desync the stream.
        assert_eq!(
            PlayerAbilities {
                invulnerable: false,
                flying: true,
                can_fly: false,
                instabuild: false,
                flying_speed: 0.05,
                walking_speed: 0.1,
            }
            .encode_body()
            .len(),
            9
        );
    }
}
