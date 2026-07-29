//! Entity metadata (`set_entity_data`) — the `SynchedEntityData` delta
//! stream. Entries are `u8 index` (0xFF terminator) + `VarInt serializer
//! type` + a value whose wire format depends on the type
//! (`EntityDataSerializers` registration order, read from the decompile).
//!
//! Fields are read where their index is pinned by counting `defineId` up the
//! hierarchy, and everything else is skipped by serializer type. Several
//! indices are **polymorphic** — slot 8 is `LivingEntity`'s flags byte *and*
//! `ItemEntity`'s stack, slot 15 is a main arm *and* a mob's flags, slot 16 is
//! baby *and* dancing *and* celebrating. Where the serializer separates them it
//! does; where it cannot (slot 16's three BOOLEAN claimants) the caller routes
//! on the entity kind.
//!
//! The skip table covers every serializer whose length is self-describing.
//! ITEM_STACK is the exception: skipping one means walking its
//! `DataComponentPatch`, which needs the component registry ids, so
//! [`parse`] takes them and reports the entry unwalkable without them —
//! ending the parse rather than desynchronising the stream.

use rewo_data::components::DataComponentIds;
use rewo_proto::reader::PacketReader;

#[derive(Default)]
pub struct EntityMeta {
    /// Custom name (index 2), flattened from its text component.
    pub custom_name: Option<String>,
    /// Shared flags byte (index 0): 0x01 on-fire, 0x02 crouching, 0x08
    /// sprinting, 0x10 swimming, 0x20 invisible, 0x40 glowing, 0x80 elytra.
    pub flags: Option<u8>,
    /// `Entity.DATA_CUSTOM_NAME_VISIBLE` — index **3**, BOOLEAN (M70).
    ///
    /// This is `Entity.shouldShowName()` for everything except a player
    /// (`Player` overrides it to a literal `true`), so it is the whole of
    /// whether a named mob wears its nametag without being looked at.
    ///
    /// Index 3 is pinned by counting `Entity`'s `defineId` calls in
    /// declaration order: 0 shared flags, 1 air supply, 2 custom name, 3 this,
    /// 4 silent, 5 no-gravity, 6 pose, 7 ticks frozen. The same argument pins
    /// `DATA_POSE` to 6, which the working pose decode confirms independently.
    /// No kind gate is possible or needed — `Entity` owns 0..7.
    pub custom_name_visible: Option<bool>,
    /// Entity pose ordinal (index 6, POSE serializer): STANDING=0,
    /// FALL_FLYING, SLEEPING, SWIMMING, SPIN_ATTACK, CROUCHING,
    /// LONG_JUMPING, DYING, CROAKING, USING_TONGUE, SITTING, ROARING,
    /// SNIFFING, EMERGING, DIGGING, SLIDING, SHOOTING, INHALING.
    pub pose: Option<u8>,
    /// `LivingEntity.DATA_LIVING_ENTITY_FLAGS` — index 8, **BYTE** (M23
    /// item-use state). Bit `1` is `isUsingItem()`, bit `2` selects the hand
    /// (`getUsedItemHand()` → set means OFF_HAND), bit `4` is auto-spin-attack.
    ///
    /// Index 8 is exact and needs no kind gate: `Entity` defines 0..7 and this
    /// is the *first* `LivingEntity` slot (`LivingEntity:179`), so nothing else
    /// can claim it — the same counting argument that pins `DATA_POSE` to 6.
    ///
    /// **This one bit is the whole M23 unlock.** `useItemRemainingTicks` is
    /// never sent; `onSyncedDataUpdated` reconstructs it the moment this bit
    /// flips on, from the held item's `getUseDuration`.
    pub living_flags: Option<u8>,
    /// `LivingEntity.DATA_HEALTH_ID` — index 9, **FLOAT** (M24 death).
    ///
    /// The slot right after the flags byte, and pinned by the same counting
    /// argument: `Entity` owns 0..7, `LivingEntity` declares the flags at 8 and
    /// health at 9. `isDeadOrDying()` is `getHealth() <= 0 || dead`, so this is
    /// what starts the death clock for an entity the client watches die rather
    /// than being told about.
    pub health: Option<f32>,
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
    /// Raw index-16 BYTE value — `Avatar.DATA_PLAYER_MODE_CUSTOMISATION` on
    /// a player (M60), the skin-part toggle mask whose **bit 0 is
    /// `PlayerModelPart.CAPE`**.
    ///
    /// The index is a 26.2 fact and differs from 1.21's: `Avatar` was
    /// inserted between `LivingEntity` and `Player`, and it defines
    /// `DATA_PLAYER_MAIN_HAND` then `DATA_PLAYER_MODE_CUSTOMISATION` — so
    /// they are 15 and 16, where 1.21's `Player extends LivingEntity`
    /// defined absorption and score first and put customisation at 17.
    ///
    /// Like [`Self::bool16`] this is raw: the BYTE separates it from the
    /// INT-size and BOOLEAN-baby readings of the same slot, but not from
    /// any other class that might claim a BYTE there, so the caller gates
    /// on the entity kind.
    pub byte16: Option<u8>,
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
    /// `Mob.DATA_MOB_FLAGS_ID` — index 15, **BYTE**. Bit `1` is `isNoAi`, bit
    /// `2` is `isLeftHanded` (which *is* `Mob.getMainArm()`), bit `4` is
    /// `isAggressive` (M20 mob combat rigs).
    ///
    /// Index 15 is shared with [`Self::main_arm`], but the serializer
    /// separates them: only `Avatar` uses HUMANOID_ARM (42) there, and only a
    /// `Mob` puts a BYTE there. `ArmorStand.DATA_CLIENT_FLAGS` is also a BYTE
    /// at 15, so the caller gates this on the entity being a mob rather than
    /// trusting the slot alone.
    pub mob_flags: Option<u8>,
    /// Raw index-17 BYTE — `SpellcasterIllager.DATA_SPELL_CASTING_ID`
    /// (`isCastingSpell()` is `> 0`). Index 17 is the first slot past `Raider`
    /// (Entity 0..7, LivingEntity 8..14, Mob 15, PathfinderMob/Monster/
    /// PatrollingMonster none, Raider 16). Polymorphic with
    /// [`Self::bool17`] only by serializer, and with the gesture-state enums by
    /// serializer too — the caller gates on kind.
    pub byte17: Option<u8>,
    /// Raw index-17 BOOLEAN — `Pillager.IS_CHARGING_CROSSBOW`.
    pub bool17: Option<bool>,
    /// Index 17 **INT** — `TropicalFish.DATA_ID_TYPE_VARIANT` (M68), the
    /// packed `(shape, pattern, body colour, pattern colour)`.
    ///
    /// `TropicalFish extends AbstractSchoolingFish extends AbstractFish
    /// extends WaterAnimal extends PathfinderMob`: Entity 0..7, LivingEntity
    /// 8..14, Mob 15, PathfinderMob and WaterAnimal none, `AbstractFish`
    /// `FROM_BUCKET` 16, `AbstractSchoolingFish` none — so the fish's own is
    /// 17. A third reading of index 17, separated from the BYTE and BOOLEAN
    /// above by serializer; the caller still gates on kind (the M18 rule).
    pub int17: Option<i32>,
    /// Index 18 BYTE. `Sheep.DATA_WOOL_ID` (M52) — `AgeableMob` claims 16
    /// (`DATA_BABY_ID`) *and* 17 (`AGE_LOCKED`), so a `Sheep`'s own first
    /// accessor is 18. The kind gate lives in `apply_set_entity_data`;
    /// `Creaking.IS_TEARING_DOWN` shares the index with a different serializer.
    pub byte18: Option<u8>,
    /// Index 18 **INT** — `Axolotl.DATA_VARIANT` (M64). `Axolotl extends
    /// Animal`, so its first own accessor is 18: Entity 0..7, LivingEntity
    /// 8..14, Mob 15, PathfinderMob none, AgeableMob 16 + 17, Animal none.
    /// Kind-gated by the caller; the BYTE and the FROG_VARIANT at the same
    /// index are separated here by serializer.
    pub int18: Option<i32>,
    /// Index 19 **INT** — `Horse.DATA_ID_TYPE_VARIANT` (M64). `Horse extends
    /// AbstractHorse extends Animal`, and `AbstractHorse` declares exactly one
    /// accessor (`DATA_ID_FLAGS`, 18), so the horse's own is 19. The low byte
    /// is the coat and the high byte the markings.
    pub int19: Option<i32>,
    /// Index 21 **INT** — `Llama.DATA_VARIANT_ID` (M64). `Llama extends
    /// AbstractChestedHorse extends AbstractHorse`: 18 flags, 19 chest, 20
    /// `DATA_STRENGTH_ID`, 21 the variant.
    pub int21: Option<i32>,
    /// Index 20, **CAT_VARIANT** serializer (id 21) — `Cat.DATA_VARIANT_ID`
    /// (M64), a raw `minecraft:cat_variant` registry id.
    ///
    /// `Cat extends TamableAnimal`, which adds two accessors of its own
    /// (`DATA_FLAGS_ID` 18, `DATA_OWNERUUID_ID` 19) on top of `Animal`'s 18,
    /// so the cat's first is 20. **The serializer alone would have been
    /// enough** — `EntityDataSerializers.CAT_VARIANT` has exactly one
    /// claimant in the whole game — but it is keyed on `(index, serializer)`
    /// like every other field here, so a slot that moves fails loudly.
    pub cat_variant: Option<i32>,
    /// Index 23, **WOLF_VARIANT** serializer (id 25) — `Wolf.DATA_VARIANT_ID`
    /// (M64). `Wolf` declares `DATA_INTERESTED_ID` 20, `DATA_COLLAR_COLOR` 21
    /// and `DATA_ANGER_END_TIME` 22 first, so the variant is 23.
    pub wolf_variant: Option<i32>,
    /// Index 18, **FROG_VARIANT** serializer (id 27) — `Frog.DATA_VARIANT_ID`
    /// (M64). `Frog extends Animal`, so 18 — the same slot the sheep's wool
    /// byte and the axolotl's int variant use, and here too it is the
    /// serializer that separates them.
    pub frog_variant: Option<i32>,
    /// `ItemEntity.DATA_ITEM` — index 8, **ITEM_STACK** serializer (id 7).
    ///
    /// Index 8 is shared with `LivingEntity.DATA_LIVING_ENTITY_FLAGS`, which is
    /// a BYTE: `ItemEntity` extends `Entity` directly, so 8 is its first own
    /// slot exactly as it is `LivingEntity`'s. The **serializer** separates
    /// them with no kind gate needed, the same way HUMANOID_ARM separates
    /// `DATA_PLAYER_MAIN_HAND` from `DATA_MOB_FLAGS_ID` at index 15.
    ///
    /// `Some(None)` is an explicitly *empty* stack — a distinction worth
    /// keeping, because an item entity whose stack was cleared should stop
    /// rendering rather than keep its last one. `Some(Some((item, count)))`
    /// carries the count too: `getRenderedAmount` buckets it into how many
    /// copies the dropped stack draws.
    /// `(item id, count, hasFoil)` — the foil rides along because it
    /// exists only in the patch this decode already walked (M45).
    pub item_stack: Option<Option<(i32, i32, bool)>>,
}

/// Parse a metadata stream (reader positioned at the first entry index).
///
/// `components` supplies the data-component registry ids the ITEM_STACK
/// serializer needs to walk a stack's `DataComponentPatch`. `None` — the
/// headless protocol harnesses — leaves ITEM_STACK unwalkable, which ends the
/// parse at that entry exactly as it did before M24b; that is the honest
/// answer, since a stack cannot be skipped without knowing its components.
pub fn parse(r: &mut PacketReader, components: Option<DataComponentIds>) -> EntityMeta {
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
            // BOOLEAN at 3 = `Entity.DATA_CUSTOM_NAME_VISIBLE` (M70). Serializer
            // 8 is BOOLEAN, the same id the index-16 dancing/baby flag uses.
            (3, 8) => meta.custom_name_visible = r.bool().ok(),
            (6, 20) => meta.pose = r.varint().ok().map(|v| v as u8), // POSE
            // BYTE at 8 = `LivingEntity.DATA_LIVING_ENTITY_FLAGS`, the first
            // LivingEntity-owned slot. Unambiguous: Entity owns 0..7.
            (8, 0) => meta.living_flags = r.u8().ok(),
            // FLOAT at 9 = `LivingEntity.DATA_HEALTH_ID`. Same counting
            // argument as index 8, and the serializer narrows it further —
            // a direct `Entity` subclass would have to claim slot 9 with a
            // FLOAT to collide, which the caller's living gate rules out.
            (9, 3) => meta.health = r.f32().ok(),
            // ITEM_STACK at 8 = `ItemEntity.DATA_ITEM`. Needs the component
            // ids to walk the stack; without them the entry is unwalkable and
            // the parse must stop rather than desynchronise.
            (8, 7) => match components.and_then(|c| crate::item_stack::read_optional(r, c).ok()) {
                Some(crate::item_stack::WireSlot::Empty) => meta.item_stack = Some(None),
                Some(crate::item_stack::WireSlot::Stack(s)) if s.aligned_stack() => {
                    meta.item_stack = Some(Some((s.item_id, s.count, s.components.has_foil())))
                }
                _ => break,
            },
            // HUMANOID_ARM at 15 = `Avatar.DATA_PLAYER_MAIN_HAND`; the BYTE at
            // the same index is somebody else's flags byte and still skips.
            (15, 42) => meta.main_arm = r.varint().ok().map(|v| v as u8),
            // BYTE at 15 = `Mob.DATA_MOB_FLAGS_ID` (or `ArmorStand`'s client
            // flags — the caller's kind gate decides). Still skips correctly
            // for anything else because the value width is the same.
            (15, 0) => meta.mob_flags = r.u8().ok(),
            (16, 1) => meta.size = r.varint().ok(), // AbstractCubeMob.ID_SIZE (INT)
            (16, 8) => meta.bool16 = r.u8().ok().map(|b| b != 0), // baby (ageable/zombie) or dancing (allay)
            // BYTE at 16 = `Avatar.DATA_PLAYER_MODE_CUSTOMISATION` on a
            // player (M60). A third serializer in the same slot, so the
            // parser can keep them apart; which *kind* owns the BYTE is
            // still the caller's call.
            (16, 0) => meta.byte16 = r.u8().ok(),
            // SNIFFER_STATE(35) / ARMADILLO_STATE(36) / COPPER_GOLEM(37)
            // at their shared first-own-field index.
            (17, 35..=37) => meta.gesture_state = r.varint().ok().map(|v| v as u8),
            // `SpellcasterIllager.DATA_SPELL_CASTING_ID` (BYTE) and
            // `Pillager.IS_CHARGING_CROSSBOW` (BOOLEAN) both sit at 17.
            (17, 0) => meta.byte17 = r.u8().ok(),
            (17, 8) => meta.bool17 = r.u8().ok().map(|b| b != 0),
            // M68: `TropicalFish.DATA_ID_TYPE_VARIANT` — the third serializer
            // at index 17.
            (17, 1) => meta.int17 = r.varint().ok(),
            (18, 0) => meta.byte18 = r.u8().ok(),
            // M64's variant slots. Index 18 now has three readings — BYTE
            // (sheep wool / tamable flags), INT (axolotl) and FROG_VARIANT —
            // and the serializer separates all three; only the two BYTE
            // claimants need the caller's kind gate.
            (18, 1) => meta.int18 = r.varint().ok(),
            (18, 27) => meta.frog_variant = r.varint().ok(),
            (19, 1) => meta.int19 = r.varint().ok(),
            (20, 21) => meta.cat_variant = r.varint().ok(),
            (21, 1) => meta.int21 = r.varint().ok(),
            (23, 25) => meta.wolf_variant = r.varint().ok(),
            _ => {
                if !skip_value_with(r, ty, components) {
                    break; // unknown/complex type — stop (already have ours)
                }
            }
        }
    }
    meta
}

/// Advance past one value of serializer `ty`. Returns false for complex
/// types we can't size (item stack, particle, variants, profile …).
fn skip_value_with(
    r: &mut PacketReader,
    ty: i32,
    components: Option<DataComponentIds>,
) -> bool {
    // ITEM_STACK is the one serializer whose *skip* needs external data: its
    // component patch is self-describing only if the reader knows the codecs.
    if ty == 7 {
        return match components.and_then(|c| crate::item_stack::read_optional(r, c).ok()) {
            Some(slot) => slot.aligned(),
            None => false,
        };
    }
    skip_value(r, ty)
}

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
        _ => false, // PARTICLE(16), PARTICLES(17), … — bail
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
        let m = parse(&mut r, None);
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
        let m = parse(&mut r, None);
        assert_eq!(m.custom_name.as_deref(), Some("Named"));
    }

    #[test]
    fn empty_stream_yields_nothing() {
        let mut r = PacketReader::new(&[0xFF]);
        let m = parse(&mut r, None);
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
        let m = parse(&mut r, None);
        assert_eq!(m.size, Some(4));
        assert_eq!(m.bool16, None, "INT at 16 is size, not a BOOLEAN");
    }

    #[test]
    fn reads_the_main_hand_at_index_15_only_for_the_humanoid_arm_serializer() {
        // A player's `DATA_PLAYER_MAIN_HAND`: index 15, HUMANOID_ARM (42),
        // value 0 = LEFT.
        let mut b: Vec<u8> = vec![0x0F, 42, 0x00, 0xFF];
        let m = parse(&mut PacketReader::new(&b), None);
        assert_eq!(m.main_arm, Some(0));
        // RIGHT.
        b = vec![0x0F, 42, 0x01, 0xFF];
        assert_eq!(parse(&mut PacketReader::new(&b), None).main_arm, Some(1));
        // The BYTE at the same index is `Mob.DATA_MOB_FLAGS_ID` (or an armor
        // stand's client flags) — not a main hand. It must skip cleanly and
        // leave a later field readable.
        b = vec![0x0F, 0x00, 0x02, 0x10, 0x08, 0x01, 0xFF];
        let m = parse(&mut PacketReader::new(&b), None);
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
        let m = parse(&mut r, None);
        assert_eq!(m.bool16, Some(true));
        assert_eq!(m.size, None, "BOOLEAN at 16 is not an INT size");
    }
}

#[cfg(test)]
mod m20_mob_metadata_tests {
    use crate::metadata;
    use crate::PacketReader;

    /// [`super::parse`] with no component ids — every test below drives
    /// serializers that need none.
    fn parse_nc(r: &mut PacketReader) -> metadata::EntityMeta {
        metadata::parse(r, None)
    }

    /// One `(index, serializer, value)` entry plus the 0xFF terminator.
    fn one(index: u8, ser: i32, value: &[u8]) -> Vec<u8> {
        let mut b = vec![index];
        // serializer ids are all < 128 here, so a single VarInt byte.
        assert!(ser < 128);
        b.push(ser as u8);
        b.extend_from_slice(value);
        b.push(0xFF);
        b
    }

    #[test]
    fn the_index_15_byte_is_the_mob_flags_and_the_arm_is_the_humanoid_arm() {
        // BYTE (0) at 15 → `Mob.DATA_MOB_FLAGS_ID`.
        let body = one(15, 0, &[0b0000_0110]);
        let m = parse_nc(&mut PacketReader::new(&body));
        assert_eq!(m.mob_flags, Some(0b0000_0110));
        assert_eq!(m.main_arm, None, "a flags byte is not a main arm");

        // HUMANOID_ARM (42) at 15 → `Avatar.DATA_PLAYER_MAIN_HAND`.
        let body = one(15, 42, &[1]);
        let m = parse_nc(&mut PacketReader::new(&body));
        assert_eq!(m.main_arm, Some(1));
        assert_eq!(m.mob_flags, None, "a main arm is not a flags byte");
    }

    #[test]
    fn index_17_separates_the_spell_byte_from_the_crossbow_boolean() {
        let m = parse_nc(&mut PacketReader::new(&one(17, 0, &[3])));
        assert_eq!(m.byte17, Some(3));
        assert_eq!(m.bool17, None);

        let m = parse_nc(&mut PacketReader::new(&one(17, 8, &[1])));
        assert_eq!(m.bool17, Some(true));
        assert_eq!(m.byte17, None);

        // The gesture-state enums share the index but not the serializer, and
        // must keep decoding as gesture state (35..=37).
        let m = parse_nc(&mut PacketReader::new(&one(17, 36, &[2])));
        assert_eq!(m.gesture_state, Some(2));
        assert_eq!(m.byte17, None);
    }

    #[test]
    fn a_flags_byte_does_not_end_the_delta_stream() {
        // The mob-flags byte followed by a baby boolean: reading the first must
        // not consume the second's bytes or stop the walk.
        let mut body = vec![15u8, 0, 0b0000_0100];
        body.extend_from_slice(&[16u8, 8, 1, 0xFF]);
        let m = parse_nc(&mut PacketReader::new(&body));
        assert_eq!(m.mob_flags, Some(0b0000_0100));
        assert_eq!(m.bool16, Some(true), "the entry after the flags byte was lost");
    }
}
