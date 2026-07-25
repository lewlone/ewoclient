//! Entity metadata (`set_entity_data`) — the `SynchedEntityData` delta
//! stream. Entries are `u8 index` (0xFF terminator) + `VarInt serializer
//! type` + a value whose wire format depends on the type
//! (`EntityDataSerializers` registration order, read from the decompile).
//!
//! We extract the reliably-indexed **Entity base** fields — shared flags
//! (index 0) and custom name (index 2) — skipping the rest by type. The
//! skip table covers the simple serializers; a complex one (item stack,
//! particle, …) after the fields we want just stops the parse (we've
//! already got what we need — those never precede index 2). Slime size /
//! baby flags live at entity-specific indices (fragile) — deferred.

use rewo_proto::reader::PacketReader;

#[derive(Default)]
pub struct EntityMeta {
    /// Custom name (index 2), flattened from its text component.
    pub custom_name: Option<String>,
    /// Shared flags byte (index 0): 0x01 on-fire, 0x02 crouching, 0x08
    /// sprinting, 0x10 swimming, 0x20 invisible, 0x40 glowing, 0x80 elytra.
    pub flags: Option<u8>,
    /// Entity pose ordinal (index 6, POSE serializer): STANDING=0,
    /// FALL_FLYING, SLEEPING, SWIMMING, SPIN_ATTACK, CROUCHING,
    /// LONG_JUMPING, DYING, CROAKING, USING_TONGUE, SITTING, ROARING,
    /// SNIFFING, EMERGING, DIGGING, SLIDING, SHOOTING, INHALING.
    pub pose: Option<u8>,
    /// Mob gesture state (index 17 on sniffer/armadillo/copper golem —
    /// their SNIFFER_STATE/ARMADILLO_STATE/… enum ordinal). Which enum it
    /// is depends on the entity type; the caller knows the kind.
    pub gesture_state: Option<u8>,
    /// Slime / magma-cube size (index 16, INT — `AbstractCubeMob.ID_SIZE`;
    /// the model + bbox scale linearly by it). The index is exact: Entity
    /// defines 0..7, LivingEntity 8..14, Mob 15, AbstractCubeMob 16 (and
    /// DATA_POSE=6 cross-checks the count). Only meaningful on cube mobs.
    pub size: Option<i32>,
    /// Raw index-16 BOOLEAN value — **polymorphic by entity kind**, which the
    /// byte parser cannot know. The client models two uses of this slot: the
    /// baby path (`AgeableMob`/`Zombie.DATA_BABY_ID` → baby) and
    /// `Allay.DATA_DANCING` (→ dancing); both are BOOLEAN (serializer id 8). The
    /// serializer type separates INT-size from BOOLEAN, but not baby from
    /// dancing — that needs the kind, so the caller
    /// ([`crate::route_set_entity_data`]) routes this raw bit to `set_baby` or
    /// `set_dancing`. This is not a claim that slot 16 is baby-or-dancing for
    /// every entity — only for the kinds the client renders.
    pub bool16: Option<bool>,
    /// `Avatar.DATA_PLAYER_MAIN_HAND` — index 15, **HUMANOID_ARM** serializer
    /// (id 42), value `0 = LEFT`, `1 = RIGHT` (M19 combat swings).
    ///
    /// Index 15 is the first slot past `LivingEntity` (Entity 0..7,
    /// LivingEntity 8..14), so several classes claim it — `Mob` puts
    /// `DATA_MOB_FLAGS_ID` there, `ArmorStand` `DATA_CLIENT_FLAGS`, both BYTE.
    /// Unlike M18's index-16 BOOLEAN, though, the **serializer disambiguates**:
    /// only `Avatar` uses HUMANOID_ARM, so this needs no kind gate. Stored as
    /// the raw wire id; `HumanoidArm.STREAM_CODEC` is
    /// `idMapper(..., OutOfBoundsStrategy.ZERO)`, so anything but 1 is LEFT.
    pub main_arm: Option<u8>,
}

/// Parse a metadata stream (reader positioned at the first entry index).
pub fn parse(r: &mut PacketReader) -> EntityMeta {
    let mut meta = EntityMeta::default();
    loop {
        let index = match r.u8() {
            Ok(0xFF) | Err(_) => break,
            Ok(i) => i,
        };
        let ty = match r.varint() {
            Ok(t) => t,
            Err(_) => break,
        };
        match (index, ty) {
            (0, 0) => meta.flags = r.u8().ok(), // shared flags (BYTE)
            (2, 6) => {
                // custom name (OPTIONAL_COMPONENT): bool + text component.
                if r.bool().unwrap_or(false) {
                    if let Ok(nbt) = r.nbt() {
                        let s = nbt.to_plain_text();
                        if !s.is_empty() {
                            meta.custom_name = Some(s);
                        }
                    }
                }
            }
            (6, 20) => meta.pose = r.varint().ok().map(|v| v as u8), // POSE
            // HUMANOID_ARM at 15 = `Avatar.DATA_PLAYER_MAIN_HAND`; the BYTE at
            // the same index is somebody else's flags byte and still skips.
            (15, 42) => meta.main_arm = r.varint().ok().map(|v| v as u8),
            (16, 1) => meta.size = r.varint().ok(), // AbstractCubeMob.ID_SIZE (INT)
            (16, 8) => meta.bool16 = r.u8().ok().map(|b| b != 0), // baby (ageable/zombie) or dancing (allay)
            // SNIFFER_STATE(35) / ARMADILLO_STATE(36) / COPPER_GOLEM(37)
            // at their shared first-own-field index.
            (17, 35..=37) => meta.gesture_state = r.varint().ok().map(|v| v as u8),
            _ => {
                if !skip_value(r, ty) {
                    break; // unknown/complex type — stop (already have ours)
                }
            }
        }
    }
    meta
}

/// Advance past one value of serializer `ty`. Returns false for complex
/// types we can't size (item stack, particle, variants, profile …).
fn skip_value(r: &mut PacketReader, ty: i32) -> bool {
    match ty {
        // BYTE. Only index 0 (shared flags) is *read* above, but the serializer
        // is reused all over — `Mob.DATA_MOB_FLAGS_ID` and `ArmorStand`'s client
        // flags both sit at index 15 — and until M19 it was missing here, so a
        // single BYTE past index 0 ended the parse and hid every later field
        // (baby / dancing / size) in the same delta packet.
        0 => r.u8().is_ok(),
        // varint-shaped: INT, DIRECTION, BLOCK_STATE, OPTIONAL_BLOCK_STATE,
        // OPTIONAL_UNSIGNED_INT, POSE, CAT/COW/WOLF/… variant enums,
        // painting variant, mob state enums, humanoid arm.
        1 | 12 | 14 | 15 | 19 | 20 | 21..=32 | 34 | 35..=38 | 42 => r.varint().is_ok(),
        // OPTIONAL_GLOBAL_POS: bool + dimension identifier + block pos.
        33 => match r.bool() {
            Ok(true) => r.string(32767).is_ok() && r.take(8).is_ok(),
            Ok(false) => true,
            Err(_) => false,
        },
        39 => r.take(12).is_ok(), // VECTOR3 (3× f32)
        40 => r.take(16).is_ok(), // QUATERNION (4× f32)
        2 => r.varlong().is_ok(),               // LONG
        3 => r.take(4).is_ok(),                 // FLOAT
        4 => r.string(32767).is_ok(),           // STRING
        5 => r.nbt().is_ok(),                   // COMPONENT
        6 => match r.bool() {
            Ok(true) => r.nbt().is_ok(),        // OPTIONAL_COMPONENT
            Ok(false) => true,
            Err(_) => false,
        },
        8 => r.u8().is_ok(),                    // BOOLEAN
        9 => r.take(12).is_ok(),               // ROTATIONS (3× f32)
        10 => r.take(8).is_ok(),               // BLOCK_POS
        11 => match r.bool() {
            Ok(true) => r.take(8).is_ok(),      // OPTIONAL_BLOCK_POS
            Ok(false) => true,
            Err(_) => false,
        },
        13 => match r.bool() {
            Ok(true) => r.varint().is_ok(),     // OPTIONAL_LIVING_ENTITY_REF
            Ok(false) => true,
            Err(_) => false,
        },
        18 => r.varint().is_ok() && r.varint().is_ok() && r.varint().is_ok(), // VILLAGER_DATA
        _ => false, // ITEM_STACK(7), PARTICLE(16), PARTICLES(17), … — bail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A text component sent as a plain-string NBT: tag 8, u16 len, bytes.
    fn nbt_string(s: &str) -> Vec<u8> {
        let mut v = vec![0x08, 0x00, s.len() as u8];
        v.extend_from_slice(s.as_bytes());
        v
    }

    #[test]
    fn reads_custom_name_after_skipping_earlier_fields() {
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(&[0x00, 0x00, 0x40]); // idx0 BYTE flags=glowing
        b.extend_from_slice(&[0x01, 0x01, 0xAC, 0x02]); // idx1 INT air=300
        b.extend_from_slice(&[0x02, 0x06, 0x01]); // idx2 OPTIONAL_COMPONENT present
        b.extend_from_slice(&nbt_string("Bessie"));
        b.push(0xFF); // terminator
        let mut r = PacketReader::new(&b);
        let m = parse(&mut r);
        assert_eq!(m.flags, Some(0x40));
        assert_eq!(m.custom_name.as_deref(), Some("Bessie"));
    }

    #[test]
    fn stops_cleanly_on_a_complex_type() {
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(&[0x02, 0x06, 0x01]); // name first
        b.extend_from_slice(&nbt_string("Named"));
        b.extend_from_slice(&[0x07, 0x07]); // ITEM_STACK — unskippable
        let mut r = PacketReader::new(&b);
        let m = parse(&mut r);
        assert_eq!(m.custom_name.as_deref(), Some("Named"));
    }

    #[test]
    fn empty_stream_yields_nothing() {
        let mut r = PacketReader::new(&[0xFF]);
        let m = parse(&mut r);
        assert!(m.custom_name.is_none() && m.flags.is_none());
    }

    #[test]
    fn reads_cube_mob_size_at_index_16() {
        // A slime size update: index 16, INT serializer (type 1), value 4.
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(&[0x00, 0x00, 0x00]); // idx0 BYTE flags=0
        b.extend_from_slice(&[0x10, 0x01, 0x04]); // idx16 INT size=4
        b.push(0xFF);
        let mut r = PacketReader::new(&b);
        let m = parse(&mut r);
        assert_eq!(m.size, Some(4));
        assert_eq!(m.bool16, None, "INT at 16 is size, not a BOOLEAN");
    }

    #[test]
    fn reads_the_main_hand_at_index_15_only_for_the_humanoid_arm_serializer() {
        // A player's `DATA_PLAYER_MAIN_HAND`: index 15, HUMANOID_ARM (42),
        // value 0 = LEFT.
        let mut b: Vec<u8> = vec![0x0F, 42, 0x00, 0xFF];
        let m = parse(&mut PacketReader::new(&b));
        assert_eq!(m.main_arm, Some(0));
        // RIGHT.
        b = vec![0x0F, 42, 0x01, 0xFF];
        assert_eq!(parse(&mut PacketReader::new(&b)).main_arm, Some(1));
        // The BYTE at the same index is `Mob.DATA_MOB_FLAGS_ID` (or an armor
        // stand's client flags) — not a main hand. It must skip cleanly and
        // leave a later field readable.
        b = vec![0x0F, 0x00, 0x02, 0x10, 0x08, 0x01, 0xFF];
        let m = parse(&mut PacketReader::new(&b));
        assert_eq!(m.main_arm, None, "BYTE at 15 is a flags byte, not the arm");
        assert_eq!(m.bool16, Some(true), "…and the stream stayed in sync");
    }

    #[test]
    fn reads_index16_boolean_raw_without_disambiguating() {
        // Index 16, BOOLEAN serializer (type 8), true. The parser exposes the
        // raw bit; whether it means baby (ageable/zombie) or dancing (allay) is
        // the kind-aware caller's decision, not the byte parser's.
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(&[0x10, 0x08, 0x01]); // idx16 BOOLEAN = true
        b.push(0xFF);
        let mut r = PacketReader::new(&b);
        let m = parse(&mut r);
        assert_eq!(m.bool16, Some(true));
        assert_eq!(m.size, None, "BOOLEAN at 16 is not an INT size");
    }
}
