//! `world/attribute/AmbientSounds` — the biome's ambient audio, and the
//! resolution that turns "the player is at P" into one of these records.
//!
//! It lives in `rewo-world` rather than beside the rest of the sound model in
//! `rewo-net` for the same reason `chat::ChatSpan` does (M126): [`BiomeDef`]
//! has to name it, and the dependency runs net -> world.
//!
//! # How a client obtains one
//!
//! `BiomeAmbientSoundsHandler.tick` asks
//! `level.environmentAttributes().getValue(EnvironmentAttributes.AMBIENT_SOUNDS,
//! player.position())` (`BiomeAmbientSoundsHandler.java:46`). Unpacking that:
//!
//! * The attribute is registered at `"audio/ambient_sounds"` with
//!   `defaultValue(AmbientSounds.EMPTY)` and `.syncable()`
//!   (`EnvironmentAttributes.java:100-102`) — so its value reaches the client
//!   over the wire, inside the registry entries themselves.
//! * The **dimension type** supplies the base:
//!   `DimensionTypes.java:43` sets `AMBIENT_SOUNDS` to
//!   `AmbientSounds.LEGACY_CAVE_SETTINGS` for the Overworld. **That, and not
//!   any biome, is why an Overworld cave makes cave sounds** — no vanilla
//!   Overworld biome declares a mood.
//! * The **biome** layers over it
//!   (`EnvironmentAttributeSystem.addBiomeLayerForAttribute`, `:85-96`).
//!
//! # Two things about that layer invert
//!
//! 1. **The biome is sampled at the RAW QUART, with no fiddle.** The layer
//!    calls `biomeManager.getNoiseBiomeAtPosition(pos.x, pos.y, pos.z)`
//!    (`:93`), which is `QuartPos.fromBlock(Mth.floor(c))` per axis straight
//!    into `getNoiseBiomeAtQuart` (`BiomeManager.java:67-72`) — *not* the
//!    fiddled `BiomeManager.getBiome(BlockPos)` at `:31` that M14's colour path
//!    uses. Reaching for the colour path's resolver here is the natural move
//!    and gives a different biome within 2 blocks of every boundary.
//! 2. **There is no interpolation.** `AttributeTypes.AMBIENT_SOUNDS` is
//!    `AttributeType.ofNotInterpolated(AmbientSounds.CODEC)`
//!    (`AttributeTypes.java:42`), and the layer's interpolated branch is
//!    `biomeWeights != null && attribute.isSpatiallyInterpolated()` (`:89`) —
//!    both halves fail here, since `getValue(attr, pos)` passes a null
//!    interpolator (`EnvironmentAttributeReader.java:28-30`). So crossing a
//!    biome boundary is a **hard switch**, where M14's colours blend.
//!
//! The consequence of 1 + 2 together is that this resolution is a plain
//! lookup, which is why [`AmbientSounds::resolve`] takes a biome id rather
//! than a sampler.

use crate::biome::BiomeDef;

/// `world/attribute/AmbientMoodSettings`.
#[derive(Clone, Debug, PartialEq)]
pub struct AmbientMood {
    /// `soundEvent` — the mood sound's identifier.
    pub sound: String,
    /// `tickDelay`. **Not a delay in the usual sense**: it is the divisor of
    /// the per-tick moodiness step, so a larger value makes the mood take
    /// *longer* to build. 6000 in the legacy cave settings.
    pub tick_delay: i32,
    /// `blockSearchExtent` — the half-width of the cube the mood samples in.
    /// The span is `2 * extent + 1` (`BiomeAmbientSoundsHandler.java:71`).
    pub block_search_extent: i32,
    /// `soundPositionOffset` — how far **past** the sampled block the sound is
    /// placed, along the same ray. 2.0 in the legacy cave settings.
    pub sound_position_offset: f64,
}

impl AmbientMood {
    /// `AmbientMoodSettings.LEGACY_CAVE_SETTINGS`
    /// (`AmbientMoodSettings.java:20`) — the Overworld's, via its
    /// **dimension type**, not via any biome.
    pub fn legacy_cave() -> Self {
        Self {
            sound: "minecraft:ambient.cave".into(),
            tick_delay: 6000,
            block_search_extent: 8,
            sound_position_offset: 2.0,
        }
    }
}

/// `world/attribute/AmbientAdditionsSettings`.
#[derive(Clone, Debug, PartialEq)]
pub struct AmbientAddition {
    pub sound: String,
    /// `tickChance` — compared against one `nextDouble()` **per addition, per
    /// tick** (`BiomeAmbientSoundsHandler.java:64-66`).
    pub tick_chance: f64,
}

/// `world/attribute/AmbientSounds` — a record of three independent optional
/// features. All three are absent in `AmbientSounds.EMPTY`, which is the
/// attribute's declared default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AmbientSounds {
    /// `loop` — the looping bed, faded in and out by
    /// `BiomeAmbientSoundsHandler.LoopSoundInstance`.
    pub loop_sound: Option<String>,
    pub mood: Option<AmbientMood>,
    /// `additions`. A **list** since 26.x, so a biome may declare several.
    pub additions: Vec<AmbientAddition>,
}

impl AmbientSounds {
    /// `AmbientSounds.EMPTY` (`AmbientSounds.java:11`).
    pub const fn empty() -> Self {
        Self {
            loop_sound: None,
            mood: None,
            additions: Vec::new(),
        }
    }

    /// `AmbientSounds.LEGACY_CAVE_SETTINGS` (`AmbientSounds.java:12-14`) — a
    /// mood and **nothing else**. No loop, no additions.
    pub fn legacy_cave() -> Self {
        Self {
            loop_sound: None,
            mood: Some(AmbientMood::legacy_cave()),
            additions: Vec::new(),
        }
    }

    /// Whether this record asks for anything at all. `EMPTY` is the default
    /// value, so a dimension and biome that both decline leave the handler
    /// with nothing to do.
    pub fn is_empty(&self) -> bool {
        self.loop_sound.is_none() && self.mood.is_none() && self.additions.is_empty()
    }

    /// The value at a position: the dimension's base, **replaced** by the
    /// biome's if that biome declares one.
    ///
    /// The replacement is total rather than field-wise, because the biome's
    /// entry is the bare-value (override) form of an attribute modifier —
    /// `EnvironmentAttributeMap.applyModifier` runs the entry's modifier over
    /// the base (`EnvironmentAttributeMap.java:52-55`), and for every vanilla
    /// biome that modifier is a set. **This is why a Nether biome has no cave
    /// mood**: its own record declares `mood`, so the Overworld dimension's
    /// legacy cave settings are not merged in — and a field-wise merge would
    /// give every Nether biome a cave sound it does not have.
    pub fn resolve(dimension_base: &AmbientSounds, biome: Option<&BiomeDef>) -> AmbientSounds {
        match biome.and_then(|b| b.ambient_sounds.as_ref()) {
            Some(over) => over.clone(),
            None => dimension_base.clone(),
        }
    }
}

/// `QuartPos.fromBlock(Mth.floor(c))` for one axis — the conversion
/// `getNoiseBiomeAtPosition(double, double, double)` applies
/// (`BiomeManager.java:67-71`). `Mth.floor` then `>> 2`, which is **not** the
/// same as truncating toward zero: at x = -0.5 this is quart -1, where a cast
/// would give 0.
pub fn quart_from_block_coord(c: f64) -> i32 {
    (c.floor() as i32) >> 2
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The conversion floors before shifting. A truncating `as i32` cast is
    /// the natural mistake, and the point of this witness is **how narrow its
    /// disagreement is**: `>>` is an arithmetic shift, so `-3 >> 2` and
    /// `-4 >> 2` are both -1 and the two readings agree across most of the
    /// negative axis. They differ only where truncation crosses a
    /// multiple-of-4 boundary — the 1-block band immediately below each one,
    /// a quarter of the negative axis.
    ///
    /// The first draft of this test asserted they differ at `c = -1.0` and
    /// `c = -3.9`, where they do not. A fixture that cannot tell the two
    /// readings apart is worth nothing, so the disagreeing band is named
    /// explicitly and the agreeing one is pinned as well.
    #[test]
    fn the_quart_conversion_floors_rather_than_truncating() {
        assert_eq!(quart_from_block_coord(0.0), 0);
        assert_eq!(quart_from_block_coord(3.9), 0);
        assert_eq!(quart_from_block_coord(4.0), 1);
        assert_eq!(quart_from_block_coord(-0.5), -1);
        assert_eq!(quart_from_block_coord(-4.0), -1);
        assert_eq!(quart_from_block_coord(-4.1), -2);

        // Where a truncating implementation is WRONG: non-integral, and its
        // truncation lands on a multiple of 4.
        for c in [-0.5f64, -0.001, -4.5, -8.25] {
            assert_ne!(
                quart_from_block_coord(c),
                (c as i32) >> 2,
                "truncation should disagree at c = {c}"
            );
        }
        // Where it is accidentally RIGHT — most of the axis, which is why the
        // bug would be easy to ship.
        for c in [-1.0f64, -1.5, -2.75, -3.9, -5.5, -7.0] {
            assert_eq!(
                quart_from_block_coord(c),
                (c as i32) >> 2,
                "truncation happens to agree at c = {c}"
            );
        }
    }

    /// `LEGACY_CAVE_SETTINGS` is a mood alone — pinned because the natural
    /// reading of "the Overworld's ambient sounds" is a loop.
    #[test]
    fn the_legacy_cave_settings_are_a_mood_and_nothing_else() {
        let s = AmbientSounds::legacy_cave();
        assert!(s.loop_sound.is_none());
        assert!(s.additions.is_empty());
        assert!(!s.is_empty());
        assert!(AmbientSounds::empty().is_empty());
        let mood = s.mood.clone().expect("legacy cave settings declare a mood");
        assert_eq!(mood.sound, "minecraft:ambient.cave");
        assert_eq!(mood.tick_delay, 6000);
        assert_eq!(mood.block_search_extent, 8);
        assert_eq!(mood.sound_position_offset, 2.0);
    }

    /// **The biome REPLACES the dimension base; it does not merge with it.**
    ///
    /// `ofNotInterpolated`'s one-arg overload supplies an empty modifier
    /// library, so OVERRIDE is the only legal modifier and a biome's entry
    /// substitutes the whole record. The observable consequence is the Nether:
    /// every Nether biome declares its own mood, and a field-wise merge would
    /// leave the Overworld dimension's cave mood showing through wherever a
    /// biome happened not to set one.
    #[test]
    fn a_biome_replaces_the_whole_record_rather_than_merging() {
        let base = AmbientSounds::legacy_cave();
        assert!(base.mood.is_some() && base.loop_sound.is_none());

        // A biome declaring ONLY a loop.
        let mut biome = biome_def();
        biome.ambient_sounds = Some(AmbientSounds {
            loop_sound: Some("minecraft:ambient.nether_wastes.loop".into()),
            mood: None,
            additions: Vec::new(),
        });
        let r = AmbientSounds::resolve(&base, Some(&biome));
        assert_eq!(r.loop_sound.as_deref(), Some("minecraft:ambient.nether_wastes.loop"));
        assert!(
            r.mood.is_none(),
            "a merge would leak the dimension's cave mood into a biome that declares none"
        );

        // A biome declaring nothing inherits the base whole.
        let mut plain = biome_def();
        plain.ambient_sounds = None;
        assert_eq!(AmbientSounds::resolve(&base, Some(&plain)), base);
        assert_eq!(AmbientSounds::resolve(&base, None), base);

        // …and an EXPLICITLY empty record is silence, not inheritance — the
        // distinction the decoder's modifier-form guard exists to preserve.
        let mut silent = biome_def();
        silent.ambient_sounds = Some(AmbientSounds::empty());
        assert!(AmbientSounds::resolve(&base, Some(&silent)).is_empty());
    }

    fn biome_def() -> BiomeDef {
        BiomeDef {
            name: "test:biome".into(),
            temperature: 0.8,
            downfall: 0.4,
            water_color: 0,
            grass_override: None,
            foliage_override: None,
            dry_foliage_override: None,
            grass_modifier: crate::biome::GrassModifier::None,
            sky_color: None,
            fog_color: None,
            has_precipitation: true,
            temperature_modifier: crate::weather::TemperatureModifier::None,
            ambient_sounds: None,
            background_music: None,
        }
    }
}
