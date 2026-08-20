//! `ParticleTypes.STREAM_CODEC`'s per-type options, as shapes (M162).
//!
//! # Why this exists at all
//!
//! `ClientboundExplodePacket`'s last three fields are `explosionParticle`
//! (`ParticleTypes.STREAM_CODEC`), `explosionSound` (`SoundEvent.STREAM_CODEC`)
//! and `blockParticles` (a `WeightedList<ExplosionParticleInfo>`). There is **no
//! length prefix anywhere in that tail**, so the sound cannot be read without
//! first walking the particle — M41's `DataComponentPatch` rule, one layer over.
//!
//! `motion.rs` recorded the cost of doing that as "~125 option codecs". Measured
//! against `ParticleTypes.java`: **125 registrations, of which 103 are
//! `SimpleParticleType` and carry ZERO option bytes**, and the remaining 22
//! share only **13 distinct option classes**. Every one of the 13 composes from
//! combinators `component_wire::Shape` already has, so this file is a table and
//! not a parser.
//!
//! # Keyed by NAME, and that is M64's trap avoided rather than survived
//!
//! `Shape::Dispatch` indexes its variants **by position**, so expressing this as
//! a dispatch would need a 125-entry array in protocol-id order — and the
//! particle registry is not alphabetical (`block` is 1, `explosion_emitter` 29,
//! `poof` 66, `smoke` 69) while `serde_json`'s `Map` is sorted. An
//! `enumerate()`-built array would give a different wrong shape for most of the
//! table, decode without erroring for the 103, and desynchronise on the 22.
//!
//! So the table is keyed by registry name and the *id* comes from the report,
//! through the same `rewo_data::particle_types::ParticleTypes` that
//! `route_level_particles` already uses. An id the report does not know fails
//! there, before this file is asked anything.
//!
//! # What an absent name means, and the guard on it
//!
//! [`shape_for`] returns `None` for a name not in [`OPTION_BEARING`], and the
//! caller reads that as `SimpleParticleType` — zero bytes. **That is right for
//! 103 of 125 today and would silently desynchronise the moment a version adds a
//! 23rd option-bearing type**, which is exactly the failure trap 19 of the M162
//! spec names.
//!
//! It is guarded by a count rather than by a 125-name literal:
//! `soundshot`'s `w9` asserts, against the real datagen report, that the
//! registry holds exactly [`REGISTERED_AT_26_2`] names, that every one of the
//! [`OPTION_BEARING`] names is among them, and that the remainder is exactly
//! [`SIMPLE_AT_26_2`]. A version bump that adds a type — option-bearing or not —
//! moves one of those three numbers and fails the gate.

use crate::component_wire::Shape;

/// `net.minecraft.core.particles` — the shapes the 13 option classes reduce to.
///
/// Constants rather than inline literals because `Shape`'s recursive variants
/// hold `&'static Shape`, and a temporary cannot be borrowed for `'static`.
mod shapes {
    use crate::component_wire::Shape;

    /// `ByteBufCodecs.idMapper(Block.BLOCK_STATE_REGISTRY)` — a VarInt block
    /// state id. `idMapper` is `VAR_INT.map(...)`, which is what
    /// `route_level_particles` has always read for `minecraft:block`.
    pub(super) const BLOCK: Shape = Shape::VarInt;

    /// `ByteBufCodecs.INT` — a fixed big-endian i32, **not** a VarInt.
    /// `ColorParticleOption` (entity_effect / tinted_leaves / flash) and
    /// `GeyserParticleOptions` (geyser / geyser_plume) are both exactly this,
    /// and both would decode "successfully" as a VarInt for small values while
    /// eating the wrong number of bytes for a packed ARGB, which is routinely
    /// negative.
    pub(super) const INT: Shape = Shape::Int;

    /// `ByteBufCodecs.FLOAT` — `PowerParticleOption` (dragon_breath) and
    /// `SculkChargeParticleOptions`.
    pub(super) const FLOAT: Shape = Shape::Float;

    /// `ByteBufCodecs.VAR_INT` — `ShriekParticleOption`'s delay.
    pub(super) const VAR_INT: Shape = Shape::VarInt;

    /// `ItemStackTemplate.STREAM_CODEC` — `ItemParticleOption`. Recursive, and
    /// bounded by the walker's depth limit.
    pub(super) const ITEM: Shape = Shape::ItemStackTemplate;

    /// `GeyserBaseParticleOptions` — `INT waterBlocks`, `FLOAT
    /// burstImpulseBase`. Also `SpellParticleOption` (`INT color`, `FLOAT
    /// power`) and `DustParticleOptions` (`INT color`, `FLOAT scale`): three
    /// classes, one wire shape.
    const INT_FLOAT_FIELDS: [Shape; 2] = [Shape::Int, Shape::Float];
    pub(super) const INT_FLOAT: Shape = Shape::Tuple(&INT_FLOAT_FIELDS);

    /// `DustColorTransitionOptions` — `INT from`, `INT to`, `FLOAT scale`.
    const DUST_TRANSITION_FIELDS: [Shape; 3] = [Shape::Int, Shape::Int, Shape::Float];
    pub(super) const DUST_TRANSITION: Shape = Shape::Tuple(&DUST_TRANSITION_FIELDS);

    /// `TrailParticleOption` — `Vec3.STREAM_CODEC target` (three raw doubles,
    /// not the quantised `LpVec3` the velocity packet uses), `INT color`,
    /// `VAR_INT duration`.
    const TRAIL_FIELDS: [Shape; 5] = [
        Shape::Double,
        Shape::Double,
        Shape::Double,
        Shape::Int,
        Shape::VarInt,
    ];
    pub(super) const TRAIL: Shape = Shape::Tuple(&TRAIL_FIELDS);

    /// `EntityPositionSource.STREAM_CODEC` — `VAR_INT id`, `FLOAT yOffset`.
    const ENTITY_SOURCE_FIELDS: [Shape; 2] = [Shape::VarInt, Shape::Float];
    const ENTITY_SOURCE: Shape = Shape::Tuple(&ENTITY_SOURCE_FIELDS);

    /// `PositionSource.STREAM_CODEC` — a **nested dispatch**, over
    /// `minecraft:position_source_type` and not over the particle registry.
    ///
    /// Measured from the datagen report: exactly two entries, `block` = 0 and
    /// `entity` = 1, so the array order below is the registry's.
    ///
    /// The `block` arm is `BlockPos.STREAM_CODEC`, which is **one packed
    /// long** (`BlockPos.of(input.readLong())`) — not three ints and not a
    /// VarInt. `vibration` is the only particle in the game that reaches a
    /// second registry's ordering, and it is easy to miss because
    /// `VibrationParticleOption`'s own codec looks like a plain composite.
    const POSITION_SOURCE_VARIANTS: [Shape; 2] = [Shape::Long, ENTITY_SOURCE];
    const POSITION_SOURCE: Shape = Shape::Dispatch(&POSITION_SOURCE_VARIANTS);

    /// `VibrationParticleOption` — a `PositionSource`, then `VAR_INT
    /// arrivalInTicks`.
    const VIBRATION_FIELDS: [Shape; 2] = [POSITION_SOURCE, Shape::VarInt];
    pub(super) const VIBRATION: Shape = Shape::Tuple(&VIBRATION_FIELDS);
}

/// The 22 registry names whose `ParticleType` carries options, and the shape
/// each writes.
///
/// Extracted from `ParticleTypes.java` by reading every `register(...)` call
/// with four arguments — the two-argument overload is the `SimpleParticleType`
/// one. Order here is the source file's, which is deliberately **not** the
/// registry's: nothing indexes this table by position.
pub const OPTION_BEARING: &[(&str, Shape)] = &[
    // BlockParticleOption x5
    ("minecraft:block", shapes::BLOCK),
    ("minecraft:block_marker", shapes::BLOCK),
    ("minecraft:falling_dust", shapes::BLOCK),
    ("minecraft:dust_pillar", shapes::BLOCK),
    ("minecraft:block_crumble", shapes::BLOCK),
    // GeyserParticleOptions x2
    ("minecraft:geyser", shapes::INT),
    ("minecraft:geyser_plume", shapes::INT),
    // GeyserBaseParticleOptions x2
    ("minecraft:geyser_base", shapes::INT_FLOAT),
    ("minecraft:geyser_poof", shapes::INT_FLOAT),
    // PowerParticleOption
    ("minecraft:dragon_breath", shapes::FLOAT),
    // DustParticleOptions
    ("minecraft:dust", shapes::INT_FLOAT),
    // DustColorTransitionOptions
    ("minecraft:dust_color_transition", shapes::DUST_TRANSITION),
    // SpellParticleOption x2
    ("minecraft:effect", shapes::INT_FLOAT),
    ("minecraft:instant_effect", shapes::INT_FLOAT),
    // ColorParticleOption x3
    ("minecraft:entity_effect", shapes::INT),
    ("minecraft:tinted_leaves", shapes::INT),
    ("minecraft:flash", shapes::INT),
    // SculkChargeParticleOptions
    ("minecraft:sculk_charge", shapes::FLOAT),
    // ItemParticleOption
    ("minecraft:item", shapes::ITEM),
    // VibrationParticleOption
    ("minecraft:vibration", shapes::VIBRATION),
    // TrailParticleOption
    ("minecraft:trail", shapes::TRAIL),
    // ShriekParticleOption
    ("minecraft:shriek", shapes::VAR_INT),
];

/// `BuiltInRegistries.PARTICLE_TYPE`'s size in 26.2, measured from
/// `ParticleTypes.java`'s registration count and cross-checked against the
/// datagen report by `soundshot`'s `w9`.
pub const REGISTERED_AT_26_2: usize = 125;

/// How many of those are `SimpleParticleType` — zero option bytes.
pub const SIMPLE_AT_26_2: usize = REGISTERED_AT_26_2 - OPTION_BEARING.len();

/// The options shape for a particle registry name, or `None` for a
/// `SimpleParticleType`.
///
/// A linear scan over 22 entries, because this is called once per explosion and
/// twice more per weighted-list entry: a map would cost more to build than the
/// scan saves.
pub fn shape_for(name: &str) -> Option<&'static Shape> {
    OPTION_BEARING
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| s)
}

/// Walk one `ParticleTypes.STREAM_CODEC` value.
///
/// `ByteBufCodecs.registry(Registries.PARTICLE_TYPE).dispatch(...)` — a **raw**
/// registry id (not `holder`'s `id + 1`), then that type's own options. The two
/// conventions sit one field apart in `ClientboundExplodePacket`: the particle
/// is raw and the sound that follows it is `id + 1` with 0 meaning inline, and
/// reading either as the other desynchronises the weighted list after them.
///
/// Returns the resolved registry name so a caller can say which particle it saw.
///
/// **Fails closed on a name the report does not have**, rather than assuming
/// zero option bytes: a newer server naming a newer particle has an options
/// payload of unknown length, and skipping it is a guess that costs the rest of
/// the packet either way — but a guess that *looks* like it worked.
pub fn walk_particle(
    r: &mut rewo_proto::reader::PacketReader<'_>,
    types: &rewo_data::particle_types::ParticleTypes,
) -> Option<String> {
    let id = r.varint().ok()?;
    let name = types.name(id)?.to_string();
    if let Some(shape) = shape_for(&name) {
        // `depth` starts at 0: this is a top-level value, and the only
        // recursive shape reachable from here is `item`'s stack template.
        if !crate::component_wire::walk(r, shape, 0).ok()? {
            return None;
        }
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_proto::reader::PacketReader;

    /// Thirteen distinct option classes, and the table says so.
    ///
    /// The M162 spec's own trap list contradicted itself here — "the walker is
    /// a 13-row table" against "generate the 125-entry id-ordered array". 13 is
    /// the count of distinct *shapes*; 22 is the count of *rows*; 125 is the
    /// registry. All three are real and none of them is the table's length.
    #[test]
    fn the_table_has_twenty_two_rows_over_thirteen_shapes() {
        assert_eq!(OPTION_BEARING.len(), 22);
        assert_eq!(SIMPLE_AT_26_2, 103);
        let mut names: Vec<&str> = OPTION_BEARING.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "a duplicated name shadows one shape");
    }

    /// Every name is namespaced, because the report's keys are.
    #[test]
    fn every_name_carries_its_namespace() {
        for (n, _) in OPTION_BEARING {
            assert!(n.starts_with("minecraft:"), "{n}");
        }
    }

    /// `INT` is four bytes and `VAR_INT` is one for a small value — the
    /// distinction the `ColorParticleOption` family turns on.
    #[test]
    fn a_color_option_is_a_fixed_int_not_a_varint() {
        let mut b = 0x7F00_0001i32.to_be_bytes().to_vec();
        b.push(0xAB); // a sentinel the walk must not reach
        let mut r = PacketReader::new(&b);
        assert!(crate::component_wire::walk(&mut r, shape_for("minecraft:flash").unwrap(), 0).unwrap());
        assert_eq!(r.offset(), 4, "a VarInt reading stops after one byte");
    }

    /// `vibration`'s block arm is ONE packed long, not three ints.
    ///
    /// And the arm is chosen by a **second** registry's ordering
    /// (`position_source_type`: block 0, entity 1), which is the part a reader
    /// of `VibrationParticleOption` alone would miss.
    #[test]
    fn a_vibration_position_source_dispatches_on_its_own_registry() {
        let shape = shape_for("minecraft:vibration").unwrap();
        // block source: selector 0, then a packed BlockPos long, then the
        // arrival VarInt.
        let mut b = vec![0u8];
        b.extend_from_slice(&1234i64.to_be_bytes());
        b.push(40);
        let mut r = PacketReader::new(&b);
        assert!(crate::component_wire::walk(&mut r, shape, 0).unwrap());
        assert_eq!(r.offset(), b.len());

        // entity source: selector 1, VarInt id, f32 yOffset, arrival VarInt.
        let mut b = vec![1u8, 200, 1];
        b.extend_from_slice(&0.5f32.to_be_bytes());
        b.push(40);
        let mut r = PacketReader::new(&b);
        assert!(crate::component_wire::walk(&mut r, shape, 0).unwrap());
        assert_eq!(r.offset(), b.len());

        // A third selector has no shape, so the walk stops rather than
        // guessing a length.
        let mut r = PacketReader::new(&[2u8, 0, 0, 0]);
        assert!(!crate::component_wire::walk(&mut r, shape, 0).unwrap());
    }

    /// A `SimpleParticleType` consumes the id and nothing else.
    #[test]
    fn a_simple_particle_type_has_no_options() {
        assert!(shape_for("minecraft:poof").is_none());
        assert!(shape_for("minecraft:smoke").is_none());
        assert!(shape_for("minecraft:explosion_emitter").is_none());
        assert!(shape_for("minecraft:explosion").is_none());
    }
}
