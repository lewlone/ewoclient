//! The `minecraft:dimension_type` registry, in registry order.
//!
//! Every entry the server syncs during Configuration becomes one
//! [`DimensionTypeDef`]. The Play `login` / `respawn` packets reference an entry
//! by its **raw registry id** (`DimensionType.STREAM_CODEC =
//! ByteBufCodecs.holderRegistry` = an `idMapper`), so the vector's index *is*
//! the wire id — no holder id is ever assumed by name.
//!
//! Ground truth (bundled 26.2 decompile,
//! `%APPDATA%/EwoClient/rewo/26.2/decompiled/`):
//!
//! - `net/minecraft/world/level/dimension/DimensionType.java` — the record and
//!   its `createDirectCodec`, which is where every default below comes from.
//! - `net/minecraft/world/level/CardinalLighting.java` — the two directional
//!   shade tables ([`CardinalLighting::DEFAULT`] / [`CardinalLighting::NETHER`]).
//! - `net/minecraft/world/attribute/EnvironmentAttributes.java` — the
//!   `visual/*` attribute defaults ([`DimensionTypeDef`]'s colour fields).
//! - `data/minecraft/dimension_type/{overworld,overworld_caves,the_nether,
//!   the_end}.json` — the built-in entries the parser fixtures mirror.

/// Vertical extent of a dimension, in blocks/sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DimensionShape {
    pub min_y: i32,
    pub height: i32,
}

impl DimensionShape {
    /// Vanilla overworld (-64..320). The safe default when the dimension
    /// registry hasn't been parsed yet — the M1 flat-world test lives here.
    pub const OVERWORLD: DimensionShape = DimensionShape {
        min_y: -64,
        height: 384,
    };

    /// Vanilla nether (0..256) — the shape a stale Overworld column count gets
    /// wrong, which is what made every Nether chunk fail to decode in M15.
    pub const NETHER: DimensionShape = DimensionShape {
        min_y: 0,
        height: 256,
    };

    /// `DimensionType.MIN_Y` / `MAX_Y` / `Y_SIZE`, evaluated from the same
    /// chain the decompile uses:
    /// `BlockPos.PACKED_HORIZONTAL_LENGTH = 1 + log2(2^25) = 26`, so
    /// `BITS_FOR_Y = PACKED_Y_LENGTH = 64 - 2*26 = 12`,
    /// `Y_SIZE = (1 << 12) - 32 = 4064`, `MAX_Y = (4064 >> 1) - 1 = 2031`,
    /// `MIN_Y = 2031 - 4064 + 1 = -2032`.
    pub const Y_SIZE: i32 = 4064;
    pub const MAX_Y: i32 = 2031;
    pub const MIN_Y: i32 = -2032;

    /// The constraints `DimensionType`'s compact constructor and codec impose
    /// on `min_y` / `height`, verbatim:
    ///
    /// - `Codec.intRange(MIN_Y, MAX_Y).fieldOf("min_y")`
    /// - `Codec.intRange(16, Y_SIZE).fieldOf("height")`
    /// - `height >= 16`, `height % 16 == 0`, `min_y % 16 == 0`,
    ///   `min_y + height <= MAX_Y + 1`
    ///
    /// Every one of these is load-bearing for us, not just for the server:
    /// `section_count` / `section_index` divide by 16 and index a `Vec`, so a
    /// shape that violates them would silently mis-address chunk sections.
    pub fn is_valid(&self) -> bool {
        self.min_y >= Self::MIN_Y
            && self.min_y <= Self::MAX_Y
            && self.height >= 16
            && self.height <= Self::Y_SIZE
            && self.height % 16 == 0
            && self.min_y % 16 == 0
            && self.min_y + self.height <= Self::MAX_Y + 1
    }

    pub fn section_count(&self) -> usize {
        (self.height / 16) as usize
    }

    pub fn min_section(&self) -> i32 {
        self.min_y >> 4
    }

    /// Section index (0-based from the bottom) for a world y, if in range.
    pub fn section_index(&self, y: i32) -> Option<usize> {
        if y < self.min_y || y >= self.min_y + self.height {
            return None;
        }
        Some(((y >> 4) - self.min_section()) as usize)
    }
}

/// `DimensionType.Skybox` — which sky the renderer draws.
///
/// Codec: `StringRepresentable.fromEnum`, `optionalFieldOf("skybox",
/// Skybox.OVERWORLD)` — so an entry that omits the field is `Overworld`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skybox {
    None,
    Overworld,
    End,
}

impl Skybox {
    /// The codec default for a missing `skybox` field.
    pub const DEFAULT: Skybox = Skybox::Overworld;

    /// `getSerializedName` → value. Unknown strings are `None` (the caller
    /// falls back to the codec default rather than guessing).
    pub fn parse(name: &str) -> Option<Skybox> {
        match name {
            "none" => Some(Skybox::None),
            "overworld" => Some(Skybox::Overworld),
            "end" => Some(Skybox::End),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Skybox::None => "none",
            Skybox::Overworld => "overworld",
            Skybox::End => "end",
        }
    }

    /// `DimensionType.hasEndFlashes()` — `skybox == END`.
    pub fn has_end_flashes(self) -> bool {
        self == Skybox::End
    }
}

/// `CardinalLighting` — the six directional shade factors a dimension applies
/// to block faces. Transcribed verbatim from the decompiled record's two
/// constants; these are the *only* two tables vanilla ships.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardinalLighting {
    pub down: f32,
    pub up: f32,
    pub north: f32,
    pub south: f32,
    pub west: f32,
    pub east: f32,
}

impl CardinalLighting {
    /// `CardinalLighting.DEFAULT = new CardinalLighting(0.5F, 1.0F, 0.8F, 0.8F, 0.6F, 0.6F)`.
    pub const DEFAULT: CardinalLighting = CardinalLighting {
        down: 0.5,
        up: 1.0,
        north: 0.8,
        south: 0.8,
        west: 0.6,
        east: 0.6,
    };

    /// `CardinalLighting.NETHER = new CardinalLighting(0.9F, 0.9F, 0.8F, 0.8F, 0.6F, 0.6F)`.
    /// The ceilinged Nether lights floors and ceilings almost equally.
    pub const NETHER: CardinalLighting = CardinalLighting {
        down: 0.9,
        up: 0.9,
        north: 0.8,
        south: 0.8,
        west: 0.6,
        east: 0.6,
    };

    /// `byFace`, in the mesher's face order `[up, down, north, south, west,
    /// east]` (`rewo_mesh::FACE_OFFSETS` / `rewo_data::assets::FACE_DIRS`) —
    /// **not** the Java `Direction` ordinal order.
    pub fn by_mesh_face(&self, face: usize) -> f32 {
        match face {
            0 => self.up,
            1 => self.down,
            2 => self.north,
            3 => self.south,
            4 => self.west,
            _ => self.east,
        }
    }
}

/// `CardinalLighting.Type` — the codec-visible selector.
///
/// Codec: `optionalFieldOf("cardinal_light", CardinalLighting.Type.DEFAULT)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardinalLightType {
    Default,
    Nether,
}

impl CardinalLightType {
    /// The codec default for a missing `cardinal_light` field.
    pub const DEFAULT: CardinalLightType = CardinalLightType::Default;

    pub fn parse(name: &str) -> Option<CardinalLightType> {
        match name {
            "default" => Some(CardinalLightType::Default),
            "nether" => Some(CardinalLightType::Nether),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CardinalLightType::Default => "default",
            CardinalLightType::Nether => "nether",
        }
    }

    /// `Type.get()` — the table this selector names.
    pub fn get(self) -> CardinalLighting {
        match self {
            CardinalLightType::Default => CardinalLighting::DEFAULT,
            CardinalLightType::Nether => CardinalLighting::NETHER,
        }
    }
}

// -- EnvironmentAttributes defaults (exact) ---------------------------------

/// `EnvironmentAttributes.AMBIENT_LIGHT_COLOR` default `-16777216`
/// (`0xFF000000`) — no ambient floor.
pub const DEFAULT_AMBIENT_LIGHT_COLOR: i32 = -16777216;

/// `EnvironmentAttributes.SKY_LIGHT_COLOR` default `-1` (`0xFFFFFFFF`, white).
pub const DEFAULT_SKY_LIGHT_COLOR: i32 = -1;

/// `EnvironmentAttributes.SKY_LIGHT_FACTOR` default `1.0`
/// (`valueRange(UNIT_FLOAT)`).
pub const DEFAULT_SKY_LIGHT_FACTOR: f32 = 1.0;

/// `EnvironmentAttributes.CLOUD_COLOR` default **`0`** — fully transparent.
///
/// This is not a placeholder: `LevelRenderer` skips the entire cloud pass when
/// `ARGB.alpha(cloudColor) == 0`, so a dimension that sets no cloud colour has
/// no clouds. That is how the Nether and the End get none — by omission, not by
/// a dimension check. The Overworld sets `#ccffffff`.
pub const DEFAULT_CLOUD_COLOR: i32 = 0;

/// `EnvironmentAttributes.CLOUD_HEIGHT` default `192.33`.
pub const DEFAULT_CLOUD_HEIGHT: f32 = 192.33;

/// One `minecraft:dimension_type` registry entry.
///
/// This replaces the M14-era parallel `dim_shapes` / `dim_attrs` vectors: one
/// registry-ordered vector of these carries everything the client needs, so a
/// holder id can never index two arrays that have drifted apart.
///
/// Fields correspond 1:1 to the `DimensionType` record members the client
/// actually consumes. Codec defaults are reproduced exactly:
///
/// | field | codec | default |
/// |---|---|---|
/// | `has_fixed_time` | `optionalFieldOf("has_fixed_time", false)` | `false` |
/// | `skybox` | `optionalFieldOf("skybox", OVERWORLD)` | [`Skybox::Overworld`] |
/// | `cardinal_light_type` | `optionalFieldOf("cardinal_light", DEFAULT)` | [`CardinalLightType::Default`] |
/// | `attributes` | `optionalFieldOf("attributes", EMPTY)` | every `visual/*` default below |
/// | `timelines` | `optionalFieldOf("timelines", HolderSet.empty())` | no day timeline |
/// | `ambient_light_color` | `AMBIENT_LIGHT_COLOR` | [`DEFAULT_AMBIENT_LIGHT_COLOR`] |
/// | `sky_light_color` | `SKY_LIGHT_COLOR` | [`DEFAULT_SKY_LIGHT_COLOR`] |
/// | `sky_light_factor` | `SKY_LIGHT_FACTOR` | [`DEFAULT_SKY_LIGHT_FACTOR`] |
///
/// `has_skylight`, `min_y`, `height` and `ambient_light` are **required**
/// fields with no codec default. A value of this type therefore only ever
/// exists because every one of them decoded — `rewo_net::dimension_parse`
/// returns `Result`, and a missing required field or an unknown `skybox` /
/// `cardinal_light` string is a connection error, never a silent default.
/// The one construction that is *not* a parsed registry entry is
/// [`DimensionTypeDef::unresolved_holder`], and it names itself as such.
///
/// `sky_color` / `fog_color` stay `Option` rather than taking the attribute's
/// literal `0` default: `None` means "this dimension does not override the
/// biome layer's colour", which is what the M14 positional colour stack wants.
/// Vanilla's Nether entry sets neither, and its base colour must not read as
/// black — so absence is modelled as absence, not as the codec's `0`.
#[derive(Clone, Debug, PartialEq)]
pub struct DimensionTypeDef {
    /// Registry name (`minecraft:the_nether`), kept for diagnostics — never
    /// used to *select* an entry. Selection is always by raw registry id.
    pub name: String,
    pub shape: DimensionShape,
    pub has_fixed_time: bool,
    /// Whether the synced `timelines` holder set contains `minecraft:day`.
    /// Decoded from the holder set itself, never inferred from fixed time,
    /// skybox, or the dimension name.
    pub has_day_timeline: bool,
    pub has_sky_light: bool,
    pub skybox: Skybox,
    /// `ambient_light` — the scalar floor folded into block light. Required
    /// field; `0.0` in the Overworld, `0.1` in the Nether, `0.25` in the End.
    pub ambient_light: f32,
    pub cardinal_light_type: CardinalLightType,
    /// `cardinal_light_type.get()`, resolved once at parse time.
    pub cardinal_light: CardinalLighting,
    /// `minecraft:visual/sky_color` override, if the dimension sets one.
    pub sky_color: Option<i32>,
    /// `minecraft:visual/fog_color` override, if the dimension sets one.
    pub fog_color: Option<i32>,
    /// `minecraft:visual/ambient_light_color` (opaque ARGB).
    pub ambient_light_color: i32,
    /// `minecraft:visual/sky_light_color` (opaque ARGB).
    pub sky_light_color: i32,
    /// `minecraft:visual/sky_light_factor`, `[0, 1]`.
    pub sky_light_factor: f32,
    /// `minecraft:visual/cloud_color` — an **ARGB** attribute whose alpha is
    /// meaningful (see [`DEFAULT_CLOUD_COLOR`]). Unlike `sky_color` this needs
    /// no `Option`: the attribute's own default of 0 *is* "no clouds", so
    /// absence and transparency mean the same thing here.
    pub cloud_color: i32,
    /// `minecraft:visual/cloud_height`, in blocks.
    pub cloud_height: f32,
    /// `minecraft:audio/ambient_sounds` — the **base** a biome may replace.
    ///
    /// This layer is the whole reason an Overworld cave makes cave sounds:
    /// `DimensionTypes.java:43` sets `AmbientSounds.LEGACY_CAVE_SETTINGS` here
    /// for the Overworld (and for `overworld_caves` and `the_end`), while **no
    /// vanilla Overworld or End biome sets the attribute at all**. Drop this
    /// layer and those two dimensions go silent; hard-code it as a universal
    /// default instead and `ambient.cave` starts playing in the Nether, whose
    /// dimension type deliberately sets nothing.
    ///
    /// `None` = the dimension declares no base, which is
    /// `AmbientSounds.EMPTY`'s meaning — the attribute's declared default.
    pub ambient_sounds: Option<crate::ambient::AmbientSounds>,
    /// `audio/background_music` — the layer BELOW the biome's (M147).
    ///
    /// **This is where the Overworld's music actually lives.**
    /// `DimensionTypes.java:39` sets `BackgroundMusic.OVERWORLD` here and
    /// `:124` sets `Musics.END` on the End; an ordinary biome like plains
    /// declares nothing, so with no dimension layer there is no music anywhere
    /// in the Overworld. M146 assumed the base was empty and was wrong — see
    /// `REWO_PLAN` §15's M147 entry.
    pub background_music: Option<crate::music::BackgroundMusic>,
}

impl DimensionTypeDef {
    /// The name prefix [`DimensionTypeDef::unresolved_holder`] stamps on
    /// itself. It is not a `minecraft:` identifier and cannot collide with a
    /// real registry entry, so "did we actually resolve this dimension?" is
    /// answerable from the value alone, in a log line or a test.
    pub const UNRESOLVED_NAME_PREFIX: &'static str = "rewo:unresolved_dimension_type/";

    /// The stand-in for a **packet** that names a dimension-type holder the
    /// synced registry does not contain — a negative id, or one past the end.
    /// That is the only fallback left: a registry *entry* that fails to parse
    /// is a hard error (see `rewo_net::dimension_parse`), so this value never
    /// stands in for malformed NBT and never claims to be a vanilla dimension.
    ///
    /// Its field values are the Overworld's, so an out-of-range holder
    /// degrades to exactly the pre-M16 behaviour instead of producing an
    /// impossible world — but its `name` says plainly that nothing was
    /// resolved, so it can never be mistaken for `minecraft:overworld`.
    pub fn unresolved_holder(holder: i32) -> Self {
        Self {
            name: format!("{}{holder}", Self::UNRESOLVED_NAME_PREFIX),
            shape: DimensionShape::OVERWORLD,
            has_fixed_time: false,
            // Preserve the explicitly-labelled fallback's pre-M16 Overworld
            // behavior until a real registry holder can be resolved.
            has_day_timeline: true,
            has_sky_light: true,
            skybox: Skybox::DEFAULT,
            ambient_light: 0.0,
            cardinal_light_type: CardinalLightType::DEFAULT,
            cardinal_light: CardinalLighting::DEFAULT,
            sky_color: None,
            fog_color: None,
            ambient_light_color: DEFAULT_AMBIENT_LIGHT_COLOR,
            sky_light_color: DEFAULT_SKY_LIGHT_COLOR,
            sky_light_factor: DEFAULT_SKY_LIGHT_FACTOR,
            cloud_color: DEFAULT_CLOUD_COLOR,
            cloud_height: DEFAULT_CLOUD_HEIGHT,
            // The Overworld's base, to match every other field of this
            // fallback — an unresolved holder should sound like the Overworld,
            // not like a dimension that declared silence.
            ambient_sounds: Some(crate::ambient::AmbientSounds::legacy_cave()),
            background_music: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cardinal_factors_are_the_decompiled_constants() {
        let d = CardinalLighting::DEFAULT;
        assert_eq!(
            (d.down, d.up, d.north, d.south, d.west, d.east),
            (0.5, 1.0, 0.8, 0.8, 0.6, 0.6)
        );
        let n = CardinalLighting::NETHER;
        assert_eq!(
            (n.down, n.up, n.north, n.south, n.west, n.east),
            (0.9, 0.9, 0.8, 0.8, 0.6, 0.6)
        );
        assert_eq!(CardinalLightType::Default.get(), CardinalLighting::DEFAULT);
        assert_eq!(CardinalLightType::Nether.get(), CardinalLighting::NETHER);
    }

    /// The mesher's face order is `[up, down, north, south, west, east]`, NOT
    /// Java's `Direction` ordinal order (`DOWN, UP, NORTH, SOUTH, WEST, EAST`).
    /// Getting this backwards would swap floor and ceiling shade.
    #[test]
    fn by_mesh_face_uses_the_mesher_order() {
        let d = CardinalLighting::DEFAULT;
        assert_eq!(d.by_mesh_face(0), 1.0, "face 0 is up");
        assert_eq!(d.by_mesh_face(1), 0.5, "face 1 is down");
        assert_eq!(d.by_mesh_face(2), 0.8);
        assert_eq!(d.by_mesh_face(3), 0.8);
        assert_eq!(d.by_mesh_face(4), 0.6);
        assert_eq!(d.by_mesh_face(5), 0.6);
        let n = CardinalLighting::NETHER;
        assert_eq!(n.by_mesh_face(0), 0.9);
        assert_eq!(n.by_mesh_face(1), 0.9);
    }

    #[test]
    fn enum_names_round_trip() {
        for s in [Skybox::None, Skybox::Overworld, Skybox::End] {
            assert_eq!(Skybox::parse(s.name()), Some(s));
        }
        for t in [CardinalLightType::Default, CardinalLightType::Nether] {
            assert_eq!(CardinalLightType::parse(t.name()), Some(t));
        }
        assert_eq!(Skybox::parse("nether"), None, "unknown → no guess");
        assert_eq!(CardinalLightType::parse("end"), None);
    }

    #[test]
    fn attribute_defaults_are_exact() {
        assert_eq!(DEFAULT_AMBIENT_LIGHT_COLOR, 0xFF00_0000u32 as i32);
        assert_eq!(DEFAULT_SKY_LIGHT_COLOR, 0xFFFF_FFFFu32 as i32);
        assert_eq!(DEFAULT_SKY_LIGHT_FACTOR, 1.0);
    }

    #[test]
    fn nether_shape_has_sixteen_sections() {
        assert_eq!(DimensionShape::NETHER.section_count(), 16);
        assert_eq!(DimensionShape::OVERWORLD.section_count(), 24);
        assert_eq!(DimensionShape::NETHER.min_section(), 0);
    }

    /// The `DimensionType` constructor's constraints, including the two
    /// (`% 16`) that our own section indexing depends on.
    #[test]
    fn shape_validity_matches_the_dimension_type_constructor() {
        assert_eq!(DimensionShape::MIN_Y, -2032);
        assert_eq!(DimensionShape::MAX_Y, 2031);
        assert_eq!(DimensionShape::Y_SIZE, 4064);
        assert!(DimensionShape::OVERWORLD.is_valid());
        assert!(DimensionShape::NETHER.is_valid());
        // height must be a positive multiple of 16, at least 16.
        assert!(!DimensionShape {
            min_y: 0,
            height: 0
        }
        .is_valid());
        assert!(!DimensionShape {
            min_y: 0,
            height: 8
        }
        .is_valid());
        assert!(!DimensionShape {
            min_y: 0,
            height: 100
        }
        .is_valid());
        assert!(!DimensionShape {
            min_y: 0,
            height: -16
        }
        .is_valid());
        // min_y must be a multiple of 16 and inside [MIN_Y, MAX_Y].
        assert!(!DimensionShape {
            min_y: -63,
            height: 384
        }
        .is_valid());
        assert!(!DimensionShape {
            min_y: -2048,
            height: 16
        }
        .is_valid());
        // min_y + height must not exceed MAX_Y + 1 = 2032.
        assert!(DimensionShape {
            min_y: 2016,
            height: 16
        }
        .is_valid());
        assert!(!DimensionShape {
            min_y: 2032,
            height: 16
        }
        .is_valid());
        assert!(!DimensionShape {
            min_y: 0,
            height: 4064
        }
        .is_valid());
    }

    /// The one non-parsed construction names itself, so a failure to resolve a
    /// holder can never read as "the server told us this is the Overworld".
    #[test]
    fn unresolved_holder_never_claims_to_be_vanilla() {
        let d = DimensionTypeDef::unresolved_holder(7);
        assert_eq!(d.name, "rewo:unresolved_dimension_type/7");
        assert!(d.name.starts_with(DimensionTypeDef::UNRESOLVED_NAME_PREFIX));
        assert_ne!(d.name, "minecraft:overworld");
        // Field values degrade to the pre-M16 Overworld behaviour.
        assert_eq!(d.shape, DimensionShape::OVERWORLD);
        assert!(d.has_sky_light);
        assert_eq!(d.skybox, Skybox::Overworld);
        assert_eq!(d.cardinal_light, CardinalLighting::DEFAULT);
        assert_eq!(d.sky_color, None);
        assert_eq!(d.fog_color, None);
        assert_eq!(d.ambient_light_color, DEFAULT_AMBIENT_LIGHT_COLOR);
        assert_eq!(d.sky_light_color, DEFAULT_SKY_LIGHT_COLOR);
        assert_eq!(d.sky_light_factor, DEFAULT_SKY_LIGHT_FACTOR);
        assert_ne!(
            DimensionTypeDef::unresolved_holder(7),
            DimensionTypeDef::unresolved_holder(8),
            "the holder that failed to resolve stays visible"
        );
    }
}
