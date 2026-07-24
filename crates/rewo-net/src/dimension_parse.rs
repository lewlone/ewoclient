//! Parse the synced `minecraft:dimension_type` registry into one
//! registry-ordered `Vec<DimensionTypeDef>`.
//!
//! This is the **only** dimension-type parser in the crate. The live
//! `Connection`, the record/replay path and the tests all go through
//! [`parse_dimension_registry`], so the login shape a replay derives can never
//! drift from the one a live session derives.
//!
//! Ground truth (bundled 26.2 decompile,
//! `%APPDATA%/EwoClient/rewo/26.2/decompiled/`):
//!
//! - `net/minecraft/world/level/dimension/DimensionType.java` —
//!   `createDirectCodec(EnvironmentAttributeMap.NETWORK_CODEC)` is what the
//!   server encodes each entry with, so every required/optional field and every
//!   default below is read straight off that `RecordCodecBuilder`.
//! - `net/minecraft/world/attribute/EnvironmentAttributes.java` — the
//!   `visual/*` attribute defaults.
//! - `net/minecraft/world/attribute/EnvironmentAttributeMap.java` — the map is
//!   `Codec.dispatchedMap(EnvironmentAttributes.CODEC, …)` keyed by the
//!   attribute's **full registry identifier** (`minecraft:visual/sky_color`),
//!   and each value is `Codec.either(valueCodec, {modifier, argument})`.
//! - `net/minecraft/nbt/NbtOps.java` — `getNumberValue` accepts *any* numeric
//!   tag and `getBooleanValue` is `getNumberValue(..) != 0`, which is why the
//!   scalar readers here accept Byte/Short/Int/Long/Float/Double rather than
//!   pinning one tag.
//!
//! # Why this is fallible
//!
//! `min_y`, `height`, `has_skylight` and `ambient_light` have **no** codec
//! default; `skybox` and `cardinal_light` have defaults but a *present*
//! `optionalFieldOf` value that fails to decode is an error in DFU, not a
//! silent fallback (`OptionalFieldCodec` is non-lenient here). Treating a
//! malformed entry as "the Overworld" would hand the rest of the client a
//! confident, wrong vertical shape and lighting contract — exactly the failure
//! mode that made every Nether chunk mis-decode. So a bad entry fails the
//! connection through the same `Result<_, String>` channel the rest of
//! configuration uses, and no `DimensionTypeDef` is ever fabricated from
//! unparseable NBT.

use rewo_proto::nbt::Nbt;
use rewo_proto::reader::PacketReader;
use rewo_world::dimension::{
    CardinalLightType, DimensionShape, DimensionTypeDef, Skybox, DEFAULT_AMBIENT_LIGHT_COLOR,
    DEFAULT_SKY_LIGHT_COLOR, DEFAULT_SKY_LIGHT_FACTOR,
};

/// The registry the Configuration `registry_data` packet must name for this
/// parser to apply.
pub const DIMENSION_TYPE_REGISTRY: &str = "minecraft:dimension_type";

// -- the exact `EnvironmentAttributeMap` keys we consume --------------------
//
// `Codec.dispatchedMap(EnvironmentAttributes.CODEC, …)` keys by the attribute's
// registry identifier, and `EnvironmentAttributes.register` calls
// `Identifier.withDefaultNamespace(id)` — so the wire key is the `minecraft:`
// namespace joined to the registered path (`"visual/sky_color"`), never the
// bare path.
const ATTR_SKY_COLOR: &str = "minecraft:visual/sky_color";
const ATTR_FOG_COLOR: &str = "minecraft:visual/fog_color";
const ATTR_AMBIENT_LIGHT_COLOR: &str = "minecraft:visual/ambient_light_color";
const ATTR_SKY_LIGHT_COLOR: &str = "minecraft:visual/sky_light_color";
const ATTR_SKY_LIGHT_FACTOR: &str = "minecraft:visual/sky_light_factor";

/// Why one `minecraft:dimension_type` entry could not be understood.
///
/// Every variant names the exact field, so a server that sends something we do
/// not model produces a diagnostic that points at the field rather than a
/// silently-substituted Overworld.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DimensionTypeError {
    /// The entry's NBT root was not a compound.
    NotACompound,
    /// A `fieldOf` (no codec default) field was absent.
    MissingField(&'static str),
    /// A field was present with a tag its codec cannot read (`Codec.INT` /
    /// `Codec.BOOL` / `Codec.FLOAT` all need a numeric tag; the enums need a
    /// string).
    WrongType(&'static str),
    /// A numeric field decoded but is outside its codec's `intRange` /
    /// `valueRange`, or (for `min_y`/`height`) violates a constraint
    /// `DimensionType`'s compact constructor enforces.
    OutOfRange(&'static str),
    /// `StringRepresentable.fromEnum` had no such name — e.g. a `skybox` of
    /// `"nether"`, which is not one of `none` / `overworld` / `end`.
    UnknownEnum { field: &'static str, value: String },
    /// An `attributes` entry we consume was present in the
    /// `{modifier, argument}` form rather than as a plain override. We only
    /// model the override arm of `Codec.either`, and applying the attribute's
    /// default instead would silently discard what the server actually said.
    AttributeModifierUnsupported(&'static str),
    /// An `attributes` entry we consume was present but its value could not be
    /// decoded at all (a malformed `#rrggbb`, or a non-numeric factor).
    BadAttribute(&'static str),
    /// The holder set names a timeline/tag whose visual contents are not in
    /// the exact 26.2 built-in tag reports. Choosing day or identity would be
    /// a guess, so the registry entry is rejected.
    UnsupportedTimelines(String),
}

impl std::fmt::Display for DimensionTypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotACompound => write!(f, "entry NBT is not a compound"),
            Self::MissingField(k) => write!(f, "missing required field `{k}`"),
            Self::WrongType(k) => write!(f, "field `{k}` has a tag its codec cannot read"),
            Self::OutOfRange(k) => write!(f, "field `{k}` is outside its codec's range"),
            Self::UnknownEnum { field, value } => {
                write!(f, "field `{field}` has unknown value `{value}`")
            }
            Self::AttributeModifierUnsupported(k) => write!(
                f,
                "attribute `{k}` uses the {{modifier, argument}} form, which we do not model"
            ),
            Self::BadAttribute(k) => write!(f, "attribute `{k}` has an undecodable value"),
            Self::UnsupportedTimelines(v) => write!(
                f,
                "timelines holder set `{v}` is not one of the exact 26.2 built-ins"
            ),
        }
    }
}

/// Resolve whether the dimension's `HolderSet<Timeline>` contains
/// `minecraft:day`. The tag expansions are copied from the 26.2 datagen
/// reports under `data/minecraft/tags/timeline/`. An absent field is the
/// codec's empty-holder-set default. Unknown entries are errors because their
/// visual tracks are not available here and either fallback would be invented.
fn has_day_timeline(nbt: &Nbt) -> Result<bool, DimensionTypeError> {
    let Some(value) = nbt.get("timelines") else {
        return Ok(false);
    };
    fn known(value: &str) -> Result<bool, DimensionTypeError> {
        match value {
            "#minecraft:in_overworld" | "minecraft:day" => Ok(true),
            "#minecraft:in_nether"
            | "#minecraft:in_end"
            | "#minecraft:universal"
            | "minecraft:moon"
            | "minecraft:early_game"
            | "minecraft:villager_schedule" => Ok(false),
            other => Err(DimensionTypeError::UnsupportedTimelines(other.to_string())),
        }
    }
    match value {
        Nbt::String(s) => known(s),
        Nbt::List(values) => {
            let mut day = false;
            for value in values {
                let s = value
                    .as_str()
                    .ok_or(DimensionTypeError::WrongType("timelines"))?;
                day |= known(s)?;
            }
            Ok(day)
        }
        _ => Err(DimensionTypeError::WrongType("timelines")),
    }
}

/// Any numeric tag as `f64`, mirroring `NbtOps.getNumberValue` (which accepts
/// every `NumericTag` and lets the codec pick `intValue`/`floatValue`).
fn as_number(n: &Nbt) -> Option<f64> {
    match n {
        Nbt::Byte(v) => Some(*v as f64),
        Nbt::Short(v) => Some(*v as f64),
        Nbt::Int(v) => Some(*v as f64),
        Nbt::Long(v) => Some(*v as f64),
        Nbt::Float(v) => Some(*v as f64),
        Nbt::Double(v) => Some(*v),
        _ => None,
    }
}

/// `Codec.INT.fieldOf(key)` — required, any numeric tag, `Number::intValue`.
fn req_int(nbt: &Nbt, key: &'static str) -> Result<i32, DimensionTypeError> {
    let v = nbt.get(key).ok_or(DimensionTypeError::MissingField(key))?;
    let n = as_number(v).ok_or(DimensionTypeError::WrongType(key))?;
    if !(i32::MIN as f64..=i32::MAX as f64).contains(&n) {
        return Err(DimensionTypeError::OutOfRange(key));
    }
    Ok(n as i32)
}

/// `Codec.BOOL.fieldOf(key)` — required. `NbtOps.getBooleanValue` is
/// `getNumberValue(..).doubleValue() != 0`, so any non-zero number is `true`.
fn req_bool(nbt: &Nbt, key: &'static str) -> Result<bool, DimensionTypeError> {
    let v = nbt.get(key).ok_or(DimensionTypeError::MissingField(key))?;
    let n = as_number(v).ok_or(DimensionTypeError::WrongType(key))?;
    Ok(n != 0.0)
}

/// `Codec.FLOAT.fieldOf(key)` — required, any numeric tag.
fn req_f32(nbt: &Nbt, key: &'static str) -> Result<f32, DimensionTypeError> {
    let v = nbt.get(key).ok_or(DimensionTypeError::MissingField(key))?;
    let n = as_number(v).ok_or(DimensionTypeError::WrongType(key))?;
    Ok(n as f32)
}

/// `Codec.BOOL.optionalFieldOf(key, default)` — absent takes the default, but a
/// *present* value that cannot decode is an error (DFU's `OptionalFieldCodec`
/// is not lenient here).
fn opt_bool(nbt: &Nbt, key: &'static str, default: bool) -> Result<bool, DimensionTypeError> {
    match nbt.get(key) {
        None => Ok(default),
        Some(v) => Ok(as_number(v).ok_or(DimensionTypeError::WrongType(key))? != 0.0),
    }
}

/// `StringRepresentable.fromEnum(..).optionalFieldOf(key, default)`. Absent
/// takes the default; a present non-string, or a string that names no variant,
/// is an error — an unknown `skybox` must never quietly become `overworld`.
fn opt_enum<T>(
    nbt: &Nbt,
    key: &'static str,
    default: T,
    parse: fn(&str) -> Option<T>,
) -> Result<T, DimensionTypeError> {
    let Some(v) = nbt.get(key) else {
        return Ok(default);
    };
    let s = v.as_str().ok_or(DimensionTypeError::WrongType(key))?;
    parse(s).ok_or_else(|| DimensionTypeError::UnknownEnum {
        field: key,
        value: s.to_string(),
    })
}

/// One `attributes` colour override (`AttributeTypes.RGB_COLOR` =
/// `ExtraCodecs.STRING_RGB_COLOR`). `Ok(None)` means the key is absent — which
/// for `sky_color` / `fog_color` is meaningfully different from "present and
/// zero", so it is never collapsed into the attribute's `0` default.
///
/// The value decode itself is [`crate::biome_parse::parse_color`], the same one
/// the biome layer uses for the same codec.
fn attr_color(attrs: Option<&Nbt>, key: &'static str) -> Result<Option<i32>, DimensionTypeError> {
    let Some(v) = attrs.and_then(|a| a.get(key)) else {
        return Ok(None);
    };
    if matches!(v, Nbt::Compound(_)) {
        return Err(DimensionTypeError::AttributeModifierUnsupported(key));
    }
    crate::biome_parse::parse_color(v)
        .map(Some)
        .ok_or(DimensionTypeError::BadAttribute(key))
}

/// One `attributes` float override (`AttributeTypes.FLOAT`), validated against
/// the attribute's `AttributeRange` — `EnvironmentAttribute::valueCodec` is
/// `type.valueCodec().validate(valueRange::validate)`, so an out-of-range value
/// is a decode error rather than something to clamp.
fn attr_f32(
    attrs: Option<&Nbt>,
    key: &'static str,
    range: std::ops::RangeInclusive<f32>,
) -> Result<Option<f32>, DimensionTypeError> {
    let Some(v) = attrs.and_then(|a| a.get(key)) else {
        return Ok(None);
    };
    if matches!(v, Nbt::Compound(_)) {
        return Err(DimensionTypeError::AttributeModifierUnsupported(key));
    }
    let n = as_number(v).ok_or(DimensionTypeError::BadAttribute(key))? as f32;
    if !range.contains(&n) {
        return Err(DimensionTypeError::OutOfRange(key));
    }
    Ok(Some(n))
}

/// Parse one `minecraft:dimension_type` entry's NBT into the unified
/// definition. `name` is the entry's registry identifier, carried for
/// diagnostics only — selection is always by raw registry id.
pub fn parse_dimension_type(name: &str, nbt: &Nbt) -> Result<DimensionTypeDef, DimensionTypeError> {
    if !matches!(nbt, Nbt::Compound(_)) {
        return Err(DimensionTypeError::NotACompound);
    }

    // -- required fields (`fieldOf`, no default) ----------------------------
    let min_y = req_int(nbt, "min_y")?;
    let height = req_int(nbt, "height")?;
    let shape = DimensionShape { min_y, height };
    if !shape.is_valid() {
        // `Codec.intRange` on each, plus `DimensionType`'s compact
        // constructor. Report the field the constraint is about.
        if !(DimensionShape::MIN_Y..=DimensionShape::MAX_Y).contains(&min_y) || min_y % 16 != 0 {
            return Err(DimensionTypeError::OutOfRange("min_y"));
        }
        return Err(DimensionTypeError::OutOfRange("height"));
    }
    let has_sky_light = req_bool(nbt, "has_skylight")?;
    let ambient_light = req_f32(nbt, "ambient_light")?;

    // -- optional fields with exact codec defaults --------------------------
    let has_fixed_time = opt_bool(nbt, "has_fixed_time", false)?;
    let has_day_timeline = has_day_timeline(nbt)?;
    let skybox = opt_enum(nbt, "skybox", Skybox::DEFAULT, Skybox::parse)?;
    let cardinal_light_type = opt_enum(
        nbt,
        "cardinal_light",
        CardinalLightType::DEFAULT,
        CardinalLightType::parse,
    )?;

    // `attributeMapCodec.optionalFieldOf("attributes", EnvironmentAttributeMap
    // .EMPTY)` — absent means "no overrides at all", i.e. every attribute
    // default below. Present-but-not-a-compound cannot be a map.
    let attributes = match nbt.get("attributes") {
        None => None,
        Some(a) if matches!(a, Nbt::Compound(_)) => Some(a),
        Some(_) => return Err(DimensionTypeError::WrongType("attributes")),
    };

    // `sky_color` / `fog_color` keep `Option`: their attribute default is the
    // literal `0`, but "the dimension sets no base colour" (vanilla's Nether)
    // must not read as opaque black to the biome colour stack.
    let sky_color = attr_color(attributes, ATTR_SKY_COLOR)?;
    let fog_color = attr_color(attributes, ATTR_FOG_COLOR)?;
    // These three do take their exact `EnvironmentAttributes` defaults.
    let ambient_light_color =
        attr_color(attributes, ATTR_AMBIENT_LIGHT_COLOR)?.unwrap_or(DEFAULT_AMBIENT_LIGHT_COLOR);
    let sky_light_color =
        attr_color(attributes, ATTR_SKY_LIGHT_COLOR)?.unwrap_or(DEFAULT_SKY_LIGHT_COLOR);
    let sky_light_factor =
        attr_f32(attributes, ATTR_SKY_LIGHT_FACTOR, 0.0..=1.0)?.unwrap_or(DEFAULT_SKY_LIGHT_FACTOR);

    Ok(DimensionTypeDef {
        name: name.to_string(),
        shape,
        has_fixed_time,
        has_day_timeline,
        has_sky_light,
        skybox,
        ambient_light,
        cardinal_light_type,
        cardinal_light: cardinal_light_type.get(),
        sky_color,
        fog_color,
        ambient_light_color,
        sky_light_color,
        sky_light_factor,
    })
}

/// Parse `count` `minecraft:dimension_type` registry entries **in raw wire
/// order** from a `registry_data` body; the reader must sit just past the entry
/// count. The returned vector's index *is* the holder registry id — nothing is
/// sorted, deduplicated or keyed by name.
pub fn parse_dimension_registry(
    r: &mut PacketReader,
    count: usize,
) -> Result<Vec<DimensionTypeDef>, String> {
    let mut out = Vec::with_capacity(count.min(1024));
    for idx in 0..count {
        let name = r
            .identifier()
            .map_err(|e| format!("dimension_type[{idx}]: entry name: {e}"))?;
        let has_data = r
            .bool()
            .map_err(|e| format!("dimension_type[{idx}] {name}: data flag: {e}"))?;
        if !has_data {
            // We answer `select_known_packs` with an empty list, which tells the
            // server to inline the data for every entry. An omitted one leaves
            // the dimension genuinely unknowable — and guessing "Overworld"
            // here is what silently produced a wrong shape before M16.
            return Err(format!(
                "dimension_type[{idx}] {name}: entry carried no NBT, but we asked \
                 for every entry inline (empty known-packs reply)"
            ));
        }
        let nbt = r
            .nbt()
            .map_err(|e| format!("dimension_type[{idx}] {name}: nbt: {e}"))?;
        let def = parse_dimension_type(&name, &nbt)
            .map_err(|e| format!("dimension_type[{idx}] {name}: {e}"))?;
        out.push(def);
    }
    Ok(out)
}

/// Parse a whole Configuration `registry_data` packet body. `Ok(None)` means
/// the packet was some other registry and this parser does not apply.
///
/// The record/replay path uses this so it derives the login shape from the very
/// same definitions a live session does.
pub fn parse_dimension_registry_packet(
    body: &[u8],
) -> Result<Option<Vec<DimensionTypeDef>>, String> {
    let mut r = PacketReader::new(body);
    let Ok(registry) = r.identifier() else {
        return Ok(None);
    };
    if registry != DIMENSION_TYPE_REGISTRY {
        return Ok(None);
    }
    let count = r
        .count("registry entries", 1)
        .map_err(|e| format!("dimension_type registry: entry count: {e}"))?;
    parse_dimension_registry(&mut r, count).map(Some)
}

/// The four built-in `minecraft:dimension_type` entries in the raw registry
/// order a vanilla 26.2 server actually syncs, so the index of a name here is
/// the holder id that server assigns it.
///
/// **Measured, not assumed.** It is *not* the declaration order of
/// `BuiltinDimensionTypes` (which is `overworld, the_nether, the_end,
/// overworld_caves`): the entries are loaded as datapack files and land in the
/// registry sorted by identifier, so `the_end` precedes `the_nether`. This
/// constant is graded against a captured `registry_data` packet by
/// `rewo dimensioncheck` and against the live server by
/// `rewo play --dimension-check`; nothing in the client *selects* by it —
/// selection is always by the raw id the packet carries.
pub const BUILTIN_ORDER: [&str; 4] = [
    "minecraft:overworld",
    "minecraft:overworld_caves",
    "minecraft:the_end",
    "minecraft:the_nether",
];

/// The transcribed built-ins as a `registry_data` packet body, in
/// [`BUILTIN_ORDER`]. Feed it to [`parse_dimension_registry_packet`] — the same
/// production entry point a live Configuration uses — so nothing downstream can
/// tell a fixture from a socket.
pub fn builtin_registry_body() -> Vec<u8> {
    builtin::registry_packet(&[
        (BUILTIN_ORDER[0], builtin::overworld()),
        (BUILTIN_ORDER[1], builtin::overworld_caves()),
        (BUILTIN_ORDER[2], builtin::the_end()),
        (BUILTIN_ORDER[3], builtin::the_nether()),
    ])
}

pub mod builtin {
    //! Network-NBT transcriptions of the bundled built-in
    //! `data/minecraft/dimension_type/*.json`, plus the minimal encoder that
    //! turns them into `registry_data` wire bytes.
    //!
    //! Not test-only: `rewo dimensioncheck` is a shipped, serverless oracle and
    //! grades a *captured* server registry against exactly these, through the
    //! production parser. Keeping one copy is the point — a transcription the
    //! tests trusted but the binary could not reach would be free to drift.
    //!
    //! The JSON is the *datapack* form; the wire form is the same tree under
    //! `NbtOps` (`DimensionType.NETWORK_CODEC`): booleans become `Byte`,
    //! `ambient_light` a `Float`, `min_y`/`height` `Int`s, and the colour
    //! attributes stay `#rrggbb` strings because `STRING_RGB_COLOR` writes with
    //! its primary (hex-string) arm. Non-`visual/*` attributes the client does
    //! not consume are kept in the Overworld fixture so the parser is exercised
    //! against a realistically noisy map.

    use rewo_proto::nbt::Nbt;

    pub fn s(v: &str) -> Nbt {
        Nbt::String(v.into())
    }

    fn c(entries: Vec<(&str, Nbt)>) -> Nbt {
        Nbt::Compound(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    /// `data/minecraft/dimension_type/overworld.json`.
    pub fn overworld() -> Nbt {
        c(vec![
            ("ambient_light", Nbt::Float(0.0)),
            (
                "attributes",
                c(vec![
                    (
                        "minecraft:gameplay/nether_portal_spawns_piglin",
                        Nbt::Byte(1),
                    ),
                    ("minecraft:gameplay/respawn_anchor_works", Nbt::Byte(0)),
                    ("minecraft:visual/ambient_light_color", s("#0a0a0a")),
                    ("minecraft:visual/cloud_color", s("#ccffffff")),
                    ("minecraft:visual/cloud_height", Nbt::Float(192.33)),
                    ("minecraft:visual/fog_color", s("#c0d8ff")),
                    ("minecraft:visual/sky_color", s("#78a7ff")),
                ]),
            ),
            ("coordinate_scale", Nbt::Double(1.0)),
            ("default_clock", s("minecraft:overworld")),
            ("has_ceiling", Nbt::Byte(0)),
            ("has_ender_dragon_fight", Nbt::Byte(0)),
            ("has_skylight", Nbt::Byte(1)),
            ("height", Nbt::Int(384)),
            ("infiniburn", s("#minecraft:infiniburn_overworld")),
            ("logical_height", Nbt::Int(384)),
            ("min_y", Nbt::Int(-64)),
            ("monster_spawn_block_light_limit", Nbt::Int(0)),
            ("timelines", s("#minecraft:in_overworld")),
        ])
    }

    /// `data/minecraft/dimension_type/overworld_caves.json` — identical to the
    /// Overworld apart from `has_ceiling`, which the client does not consume.
    /// It exists in the fixtures precisely to pin that: two entries that differ
    /// only in a field we ignore must still be two independent registry slots.
    pub fn overworld_caves() -> Nbt {
        let Nbt::Compound(mut entries) = overworld() else {
            unreachable!()
        };
        for (k, v) in entries.iter_mut() {
            if k == "has_ceiling" {
                *v = Nbt::Byte(1);
            }
        }
        Nbt::Compound(entries)
    }

    /// `data/minecraft/dimension_type/the_nether.json` — no `sky_color`, no
    /// `fog_color`, `cardinal_light: nether`, `skybox: none`, no skylight.
    pub fn the_nether() -> Nbt {
        c(vec![
            ("ambient_light", Nbt::Float(0.1)),
            (
                "attributes",
                c(vec![
                    ("minecraft:gameplay/can_start_raid", Nbt::Byte(0)),
                    ("minecraft:gameplay/fast_lava", Nbt::Byte(1)),
                    ("minecraft:gameplay/piglins_zombify", Nbt::Byte(0)),
                    ("minecraft:gameplay/respawn_anchor_works", Nbt::Byte(1)),
                    ("minecraft:gameplay/sky_light_level", Nbt::Float(4.0)),
                    ("minecraft:visual/ambient_light_color", s("#302821")),
                    ("minecraft:visual/fog_end_distance", Nbt::Float(96.0)),
                    ("minecraft:visual/fog_start_distance", Nbt::Float(10.0)),
                    ("minecraft:visual/sky_light_color", s("#7a7aff")),
                    ("minecraft:visual/sky_light_factor", Nbt::Float(0.0)),
                ]),
            ),
            ("cardinal_light", s("nether")),
            ("coordinate_scale", Nbt::Double(8.0)),
            ("has_ceiling", Nbt::Byte(1)),
            ("has_ender_dragon_fight", Nbt::Byte(0)),
            ("has_fixed_time", Nbt::Byte(1)),
            ("has_skylight", Nbt::Byte(0)),
            ("height", Nbt::Int(256)),
            ("infiniburn", s("#minecraft:infiniburn_nether")),
            ("logical_height", Nbt::Int(128)),
            ("min_y", Nbt::Int(0)),
            ("monster_spawn_block_light_limit", Nbt::Int(15)),
            ("skybox", s("none")),
            ("timelines", s("#minecraft:in_nether")),
        ])
    }

    /// `data/minecraft/dimension_type/the_end.json` — `skybox: end`, fixed
    /// time, skylight on but `sky_light_factor` 0, and a tinted sky light.
    pub fn the_end() -> Nbt {
        c(vec![
            ("ambient_light", Nbt::Float(0.25)),
            (
                "attributes",
                c(vec![
                    ("minecraft:gameplay/respawn_anchor_works", Nbt::Byte(0)),
                    ("minecraft:visual/ambient_light_color", s("#3f473f")),
                    ("minecraft:visual/fog_color", s("#181318")),
                    ("minecraft:visual/sky_color", s("#000000")),
                    ("minecraft:visual/sky_light_color", s("#ac60cd")),
                    ("minecraft:visual/sky_light_factor", Nbt::Float(0.0)),
                ]),
            ),
            ("coordinate_scale", Nbt::Double(1.0)),
            ("default_clock", s("minecraft:the_end")),
            ("has_ceiling", Nbt::Byte(0)),
            ("has_ender_dragon_fight", Nbt::Byte(1)),
            ("has_fixed_time", Nbt::Byte(1)),
            ("has_skylight", Nbt::Byte(1)),
            ("height", Nbt::Int(256)),
            ("infiniburn", s("#minecraft:infiniburn_end")),
            ("logical_height", Nbt::Int(256)),
            ("min_y", Nbt::Int(0)),
            ("monster_spawn_block_light_limit", Nbt::Int(0)),
            ("skybox", s("end")),
            ("timelines", s("#minecraft:in_end")),
        ])
    }

    // -- minimal network-NBT writer (tests only) ---------------------------
    //
    // `rewo-proto` only reads NBT; the fixtures need to *write* it to exercise
    // the wire path end to end. Kept deliberately tiny: exactly the tags the
    // fixtures above use, in the layout `rewo_proto::nbt::read_payload` reads.

    fn write_string(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    fn tag_id(n: &Nbt) -> u8 {
        match n {
            Nbt::End => 0,
            Nbt::Byte(_) => 1,
            Nbt::Short(_) => 2,
            Nbt::Int(_) => 3,
            Nbt::Long(_) => 4,
            Nbt::Float(_) => 5,
            Nbt::Double(_) => 6,
            Nbt::ByteArray(_) => 7,
            Nbt::String(_) => 8,
            Nbt::List(_) => 9,
            Nbt::Compound(_) => 10,
            Nbt::IntArray(_) => 11,
            Nbt::LongArray(_) => 12,
        }
    }

    fn write_payload(out: &mut Vec<u8>, n: &Nbt) {
        match n {
            Nbt::End => {}
            Nbt::Byte(v) => out.push(*v as u8),
            Nbt::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
            Nbt::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
            Nbt::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
            Nbt::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
            Nbt::Double(v) => out.extend_from_slice(&v.to_be_bytes()),
            Nbt::String(v) => write_string(out, v),
            Nbt::Compound(entries) => {
                for (k, v) in entries {
                    out.push(tag_id(v));
                    write_string(out, k);
                    write_payload(out, v);
                }
                out.push(0); // TAG_End
            }
            other => panic!("fixture writer does not handle {other:?}"),
        }
    }

    /// Network NBT: tag byte, then the nameless payload.
    pub fn write_network_nbt(out: &mut Vec<u8>, n: &Nbt) {
        out.push(tag_id(n));
        write_payload(out, n);
    }

    /// A whole `registry_data` body for `minecraft:dimension_type`, in exactly
    /// the given order — the wire's raw registry order.
    pub fn registry_packet(entries: &[(&str, Nbt)]) -> Vec<u8> {
        let mut w = rewo_proto::writer::PacketWriter::default();
        w.string(super::DIMENSION_TYPE_REGISTRY);
        w.varint(entries.len() as i32);
        let mut body = w.buf;
        for (name, nbt) in entries {
            let mut w = rewo_proto::writer::PacketWriter::default();
            w.string(name).bool(true);
            body.extend_from_slice(&w.buf);
            write_network_nbt(&mut body, nbt);
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use super::builtin::*;
    use super::*;

    fn parse(name: &str, nbt: &Nbt) -> DimensionTypeDef {
        parse_dimension_type(name, nbt).expect("fixture must parse")
    }

    /// Overworld: the shape, the required scalars, and every visual attribute —
    /// two set explicitly, two taken from the exact codec default.
    #[test]
    fn overworld_entry_is_exact() {
        let d = parse("minecraft:overworld", &overworld());
        assert_eq!(d.name, "minecraft:overworld");
        assert_eq!(
            d.shape,
            DimensionShape {
                min_y: -64,
                height: 384
            }
        );
        assert_eq!(d.shape.section_count(), 24);
        assert!(d.has_sky_light);
        assert!(!d.has_fixed_time);
        assert!(d.has_day_timeline);
        assert_eq!(d.ambient_light, 0.0);
        assert_eq!(d.skybox, Skybox::Overworld, "absent skybox → codec default");
        assert_eq!(d.cardinal_light_type, CardinalLightType::Default);
        assert_eq!(
            d.cardinal_light,
            rewo_world::dimension::CardinalLighting::DEFAULT
        );
        assert_eq!(d.sky_color, Some(0xFF78_A7FFu32 as i32));
        assert_eq!(d.fog_color, Some(0xFFC0_D8FFu32 as i32));
        assert_eq!(d.ambient_light_color, 0xFF0A_0A0Au32 as i32);
        // Neither sky-light attribute is set → the exact EnvironmentAttributes
        // defaults, not zero and not the End's values.
        assert_eq!(d.sky_light_color, DEFAULT_SKY_LIGHT_COLOR);
        assert_eq!(d.sky_light_color, -1);
        assert_eq!(d.sky_light_factor, DEFAULT_SKY_LIGHT_FACTOR);
        assert_eq!(d.sky_light_factor, 1.0);
    }

    /// Overworld Caves differs from the Overworld only in `has_ceiling`, which
    /// the client does not consume — so every field we *do* consume matches,
    /// and the two are distinguished by registry slot alone.
    #[test]
    fn overworld_caves_matches_overworld_in_every_consumed_field() {
        let ow = parse("minecraft:overworld", &overworld());
        let caves = parse("minecraft:overworld_caves", &overworld_caves());
        assert_eq!(caves.shape, ow.shape);
        assert_eq!(caves.has_sky_light, ow.has_sky_light);
        assert_eq!(caves.has_fixed_time, ow.has_fixed_time);
        assert_eq!(caves.has_day_timeline, ow.has_day_timeline);
        assert_eq!(caves.ambient_light, ow.ambient_light);
        assert_eq!(caves.skybox, ow.skybox);
        assert_eq!(caves.cardinal_light_type, ow.cardinal_light_type);
        assert_eq!(caves.sky_color, ow.sky_color);
        assert_eq!(caves.fog_color, ow.fog_color);
        assert_eq!(caves.ambient_light_color, ow.ambient_light_color);
        assert_eq!(caves.sky_light_color, ow.sky_light_color);
        assert_eq!(caves.sky_light_factor, ow.sky_light_factor);
        assert_ne!(caves.name, ow.name);
    }

    /// The Nether is where every M15-era assumption breaks: 0..256 not
    /// -64..320, no skylight, fixed time, `skybox: none`, NETHER cardinal
    /// shade, and — critically — no base sky or fog colour at all.
    #[test]
    fn nether_entry_is_exact() {
        let d = parse("minecraft:the_nether", &the_nether());
        assert_eq!(
            d.shape,
            DimensionShape {
                min_y: 0,
                height: 256
            }
        );
        assert_eq!(d.shape, DimensionShape::NETHER);
        assert_eq!(d.shape.section_count(), 16);
        assert!(!d.has_sky_light);
        assert!(d.has_fixed_time);
        assert!(!d.has_day_timeline);
        assert_eq!(d.ambient_light, 0.1);
        assert_eq!(d.skybox, Skybox::None);
        assert!(!d.skybox.has_end_flashes());
        assert_eq!(d.cardinal_light_type, CardinalLightType::Nether);
        assert_eq!(
            d.cardinal_light,
            rewo_world::dimension::CardinalLighting::NETHER
        );
        assert_eq!(d.cardinal_light.down, 0.9);
        assert_eq!(d.cardinal_light.up, 0.9);
        // Absent, NOT the attribute's literal `0` default — an inferred black
        // base would tint the whole Nether sky/fog stack.
        assert_eq!(d.sky_color, None);
        assert_eq!(d.fog_color, None);
        assert_eq!(d.ambient_light_color, 0xFF30_2821u32 as i32);
        assert_eq!(d.sky_light_color, 0xFF7A_7AFFu32 as i32);
        assert_eq!(d.sky_light_factor, 0.0);
    }

    /// The End: `skybox: end` (the only entry with end flashes), fixed time,
    /// skylight on but fully damped, and an explicitly black `sky_color` — the
    /// case that proves `Some(0)` and `None` are not interchangeable.
    #[test]
    fn end_entry_is_exact() {
        let d = parse("minecraft:the_end", &the_end());
        assert_eq!(
            d.shape,
            DimensionShape {
                min_y: 0,
                height: 256
            }
        );
        assert!(d.has_sky_light);
        assert!(d.has_fixed_time);
        assert!(!d.has_day_timeline);
        assert_eq!(d.ambient_light, 0.25);
        assert_eq!(d.skybox, Skybox::End);
        assert!(d.skybox.has_end_flashes());
        assert_eq!(d.cardinal_light_type, CardinalLightType::Default);
        assert_eq!(d.sky_color, Some(0xFF00_0000u32 as i32));
        assert_ne!(d.sky_color, None, "an explicit #000000 is a real override");
        assert_eq!(d.fog_color, Some(0xFF18_1318u32 as i32));
        assert_eq!(d.ambient_light_color, 0xFF3F_473Fu32 as i32);
        assert_eq!(d.sky_light_color, 0xFFAC_60CDu32 as i32);
        assert_eq!(d.sky_light_factor, 0.0);
        // The Nether's shape numbers match, but nothing else does.
        let nether = parse("minecraft:the_nether", &the_nether());
        assert_eq!(d.shape, nether.shape);
        assert_ne!(d.has_sky_light, nether.has_sky_light);
        assert_ne!(d.skybox, nether.skybox);
        assert_ne!(d.cardinal_light_type, nether.cardinal_light_type);
    }

    #[test]
    fn timelines_are_decoded_from_the_holder_set_not_fixed_time() {
        let mut direct_day = overworld();
        {
            let Nbt::Compound(entries) = &mut direct_day else {
                unreachable!()
            };
            let timelines = entries
                .iter_mut()
                .find(|(key, _)| key == "timelines")
                .expect("fixture timelines");
            timelines.1 =
                Nbt::List(vec![s("minecraft:villager_schedule"), s("minecraft:day")]);
        }
        assert!(parse("test:direct_day", &direct_day).has_day_timeline);

        // Fixed time is a separate member of DimensionType. It does not own
        // timeline selection; this deliberately unusual but codec-valid entry
        // proves the two fields cannot be conflated.
        let Nbt::Compound(entries) = &mut direct_day else {
            unreachable!()
        };
        entries.push(("has_fixed_time".into(), Nbt::Byte(1)));
        let parsed = parse("test:fixed_with_day", &direct_day);
        assert!(parsed.has_fixed_time);
        assert!(parsed.has_day_timeline);

        let mut unknown = overworld();
        let Nbt::Compound(entries) = &mut unknown else {
            unreachable!()
        };
        entries
            .iter_mut()
            .find(|(key, _)| key == "timelines")
            .expect("fixture timelines")
            .1 = s("#example:unknown_visual_tracks");
        assert_eq!(
            parse_dimension_type("test:unknown", &unknown),
            Err(DimensionTypeError::UnsupportedTimelines(
                "#example:unknown_visual_tracks".into()
            ))
        );
    }

    /// The whole wire path, in an order no name-sort would produce. Index is
    /// the raw holder id and nothing else.
    #[test]
    fn registry_keeps_raw_wire_order() {
        // Alphabetically this would be overworld, overworld_caves, the_end,
        // the_nether. The server may send any order at all, and the ids follow
        // the order — so the fixture uses a deliberately different one.
        let body = registry_packet(&[
            ("minecraft:the_nether", the_nether()),
            ("minecraft:the_end", the_end()),
            ("minecraft:overworld_caves", overworld_caves()),
            ("minecraft:overworld", overworld()),
        ]);
        let defs = parse_dimension_registry_packet(&body)
            .expect("registry must parse")
            .expect("packet is the dimension_type registry");
        assert_eq!(defs.len(), 4);
        assert_eq!(defs[0].name, "minecraft:the_nether");
        assert_eq!(defs[1].name, "minecraft:the_end");
        assert_eq!(defs[2].name, "minecraft:overworld_caves");
        assert_eq!(defs[3].name, "minecraft:overworld");
        // Raw id 0 is the Nether here, not the Overworld.
        assert_eq!(defs[0].shape, DimensionShape::NETHER);
        assert!(!defs[0].has_sky_light);
        assert_eq!(defs[0].cardinal_light_type, CardinalLightType::Nether);
        assert_eq!(defs[3].shape, DimensionShape::OVERWORLD);
        assert!(defs[3].has_sky_light);
        // A name-keyed lookup would have put the Overworld at 0; the sorted
        // order is genuinely different from the wire order.
        let mut sorted: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        sorted.sort_unstable();
        let wire: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_ne!(sorted, wire, "fixture must not be name-sorted");
    }

    #[test]
    fn other_registries_are_not_this_parsers_business() {
        let mut w = rewo_proto::writer::PacketWriter::default();
        w.string("minecraft:worldgen/biome").varint(0);
        assert_eq!(parse_dimension_registry_packet(&w.buf), Ok(None));
    }

    // -- malformed entries must not masquerade as vanilla ------------------

    fn without(nbt: &Nbt, key: &str) -> Nbt {
        let Nbt::Compound(entries) = nbt else {
            unreachable!()
        };
        Nbt::Compound(entries.iter().filter(|(k, _)| k != key).cloned().collect())
    }

    fn with(nbt: &Nbt, key: &str, value: Nbt) -> Nbt {
        let Nbt::Compound(mut entries) = without(nbt, key) else {
            unreachable!()
        };
        entries.push((key.to_string(), value));
        Nbt::Compound(entries)
    }

    fn with_attr(nbt: &Nbt, key: &str, value: Nbt) -> Nbt {
        let attrs = nbt
            .get("attributes")
            .cloned()
            .unwrap_or(Nbt::Compound(vec![]));
        with(nbt, "attributes", with(&attrs, key, value))
    }

    #[test]
    fn missing_required_fields_are_errors_not_overworld() {
        for key in ["min_y", "height", "has_skylight", "ambient_light"] {
            let broken = without(&the_nether(), key);
            let err = parse_dimension_type("minecraft:the_nether", &broken)
                .expect_err("a missing required field must not decode");
            assert!(
                matches!(&err, DimensionTypeError::MissingField(k) if *k == key),
                "{key}: got {err:?}"
            );
        }
        // And the failure reaches the connection's error channel, named.
        let body = registry_packet(&[("minecraft:the_nether", without(&the_nether(), "height"))]);
        let err = parse_dimension_registry_packet(&body).unwrap_err();
        assert!(err.contains("dimension_type[0]"), "{err}");
        assert!(err.contains("minecraft:the_nether"), "{err}");
        assert!(err.contains("height"), "{err}");
    }

    #[test]
    fn unknown_enum_values_are_errors_not_defaults() {
        let bad_sky = with(&the_nether(), "skybox", s("nether"));
        assert_eq!(
            parse_dimension_type("x", &bad_sky),
            Err(DimensionTypeError::UnknownEnum {
                field: "skybox",
                value: "nether".into()
            })
        );
        let bad_cardinal = with(&the_nether(), "cardinal_light", s("the_end"));
        assert_eq!(
            parse_dimension_type("x", &bad_cardinal),
            Err(DimensionTypeError::UnknownEnum {
                field: "cardinal_light",
                value: "the_end".into()
            })
        );
        // A non-string where the enum lives is a type error, not a default.
        let numeric_sky = with(&the_nether(), "skybox", Nbt::Int(1));
        assert_eq!(
            parse_dimension_type("x", &numeric_sky),
            Err(DimensionTypeError::WrongType("skybox"))
        );
    }

    #[test]
    fn shape_constraints_are_enforced() {
        // height must be a multiple of 16 (our section indexing divides by it).
        assert_eq!(
            parse_dimension_type("x", &with(&the_nether(), "height", Nbt::Int(100))),
            Err(DimensionTypeError::OutOfRange("height"))
        );
        assert_eq!(
            parse_dimension_type("x", &with(&the_nether(), "height", Nbt::Int(0))),
            Err(DimensionTypeError::OutOfRange("height"))
        );
        assert_eq!(
            parse_dimension_type("x", &with(&the_nether(), "min_y", Nbt::Int(-63))),
            Err(DimensionTypeError::OutOfRange("min_y"))
        );
        assert_eq!(
            parse_dimension_type("x", &with(&the_nether(), "min_y", Nbt::Int(-4096))),
            Err(DimensionTypeError::OutOfRange("min_y"))
        );
        // A non-numeric tag where a number belongs.
        assert_eq!(
            parse_dimension_type("x", &with(&the_nether(), "min_y", s("0"))),
            Err(DimensionTypeError::WrongType("min_y"))
        );
    }

    #[test]
    fn malformed_visual_attributes_are_errors_not_codec_defaults() {
        // A colour we cannot decode must not silently become the default.
        assert_eq!(
            parse_dimension_type(
                "x",
                &with_attr(&the_end(), "minecraft:visual/sky_light_color", s("#zz00ff"))
            ),
            Err(DimensionTypeError::BadAttribute(
                "minecraft:visual/sky_light_color"
            ))
        );
        // The `{modifier, argument}` arm of `Codec.either` is not modelled;
        // applying the default in its place would discard the server's value.
        let modifier = Nbt::Compound(vec![
            ("argument".into(), Nbt::Float(0.5)),
            ("modifier".into(), s("multiply")),
        ]);
        assert_eq!(
            parse_dimension_type(
                "x",
                &with_attr(&the_end(), "minecraft:visual/sky_light_factor", modifier)
            ),
            Err(DimensionTypeError::AttributeModifierUnsupported(
                "minecraft:visual/sky_light_factor"
            ))
        );
        // `SKY_LIGHT_FACTOR` carries `valueRange(UNIT_FLOAT)`, and
        // `EnvironmentAttribute::valueCodec` validates rather than clamps.
        assert_eq!(
            parse_dimension_type(
                "x",
                &with_attr(
                    &the_end(),
                    "minecraft:visual/sky_light_factor",
                    Nbt::Float(1.5)
                )
            ),
            Err(DimensionTypeError::OutOfRange(
                "minecraft:visual/sky_light_factor"
            ))
        );
    }

    #[test]
    fn an_entry_without_nbt_is_an_error() {
        let mut w = rewo_proto::writer::PacketWriter::default();
        w.string(DIMENSION_TYPE_REGISTRY).varint(1);
        w.string("minecraft:overworld").bool(false);
        let err = parse_dimension_registry_packet(&w.buf).unwrap_err();
        assert!(err.contains("carried no NBT"), "{err}");
    }

    /// `NbtOps.getNumberValue` accepts any numeric tag, so a server that writes
    /// `min_y` as a Long or `has_skylight` as an Int is still exact — pinning
    /// one tag would reject valid data.
    #[test]
    fn numeric_tags_are_interchangeable_like_nbt_ops() {
        let n = with(&the_nether(), "min_y", Nbt::Long(0));
        let n = with(&n, "height", Nbt::Short(256));
        let n = with(&n, "has_skylight", Nbt::Int(0));
        let n = with(&n, "ambient_light", Nbt::Double(0.1));
        let d = parse("minecraft:the_nether", &n);
        assert_eq!(d.shape, DimensionShape::NETHER);
        assert!(!d.has_sky_light);
        assert!((d.ambient_light - 0.1).abs() < 1e-6);
    }
}
