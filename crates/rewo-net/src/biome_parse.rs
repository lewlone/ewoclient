//! Parse the `minecraft:worldgen/biome` registry NBT into
//! `rewo_world::biome` types. The `minecraft:dimension_type` registry has its
//! own module ([`crate::dimension_parse`]) — the two share only
//! [`parse_color`], because they share the colour *codec*, not the entry shape.
//!
//! Wire shape (captured from the live 26.2 server's `registry_data`, matched to
//! the `Biome.NETWORK_CODEC` / `BiomeSpecialEffects.CODEC` codecs):
//!
//! ```text
//! biome compound:
//!   has_precipitation: Byte
//!   temperature: Float
//!   temperature_modifier: String (optional; ignored — color-irrelevant)
//!   downfall: Float
//!   effects: {
//!     water_color: String "#rrggbb"          (STRING_RGB_COLOR)
//!     foliage_color / dry_foliage_color / grass_color: String "#rrggbb" (opt)
//!     grass_color_modifier: String "none"|"dark_forest"|"swamp" (opt)
//!   }
//!   attributes: {                             (optional)
//!     "minecraft:visual/sky_color": String "#rrggbb"  (override form)
//!     "minecraft:visual/fog_color": String "#rrggbb"
//!     ... (or a {argument, modifier} compound — not applied for sky/fog)
//!   }
//! ```
//!
//! `STRING_RGB_COLOR` encodes over the wire as a `#`-prefixed 6-hex-digit
//! STRING (`withAlternative(hexColor(6).xmap(ARGB::opaque, …), INT)` — the
//! primary/string codec is used for writing). `hexColor`'s `.xmap(ARGB::opaque)`
//! means the decoded value is opaque, so we OR in `0xFF000000`.

use rewo_proto::nbt::Nbt;
use rewo_world::ambient::{AmbientAddition, AmbientMood, AmbientSounds};
use rewo_world::music::{BackgroundMusic, Music};
use rewo_world::biome::{BiomeDef, GrassModifier};
use rewo_world::weather::TemperatureModifier;

/// Default water color (`#3f76e4`, opaque) when a biome somehow omits it —
/// vanilla always sends it (`water_color` is a required field).
const DEFAULT_WATER: i32 = 0xFF3F_76E4u32 as i32;

/// A boolean NBT field. The codec writes `Byte`, so that is the only form a
/// vanilla server sends; `Int` is accepted for the same reason `as_f32` is
/// liberal.
fn as_bool(n: &Nbt) -> Option<bool> {
    match n {
        Nbt::Byte(v) => Some(*v != 0),
        Nbt::Int(v) => Some(*v != 0),
        _ => None,
    }
}

fn as_f32(n: &Nbt) -> Option<f32> {
    match n {
        Nbt::Float(v) => Some(*v),
        Nbt::Double(v) => Some(*v as f32),
        Nbt::Byte(v) => Some(*v as f32),
        Nbt::Short(v) => Some(*v as f32),
        Nbt::Int(v) => Some(*v as f32),
        _ => None,
    }
}

/// `#rrggbb` hex string → opaque ARGB int (`ARGB.opaque` of the parsed RGB).
fn parse_hex_color(s: &str) -> Option<i32> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some((v | 0xFF00_0000) as i32)
}

/// A bare `STRING_ARGB_COLOR` value: `hexColor(8)` — `#aarrggbb`, alpha
/// **preserved** — or the `ARGB_COLOR_CODEC` int fallback.
///
/// Distinct from [`parse_color`] on purpose. `STRING_RGB_COLOR` runs its six
/// digits through `ARGB::opaque`; this one does not, and the difference is
/// load-bearing for `visual/cloud_color`, whose alpha decides whether clouds
/// render at all.
pub fn parse_argb_color(n: &Nbt) -> Option<i32> {
    match n {
        Nbt::String(s) => {
            let hex = s.strip_prefix('#').unwrap_or(s);
            if hex.len() != 8 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            Some(u32::from_str_radix(hex, 16).ok()? as i32)
        }
        Nbt::Int(v) => Some(*v),
        _ => None,
    }
}

/// A bare `STRING_RGB_COLOR` value: the `#rrggbb` string form (what vanilla
/// sends) or the `INT` fallback form.
///
/// `withAlternative` writes with its *primary* arm, so a real server always
/// emits the hex string, and `hexColor(6).xmap(ARGB::opaque, …)` makes that
/// value opaque. The `INT` arm is `RGB_COLOR_CODEC` = a bare `Codec.INT` with
/// no `ARGB.opaque`; we force alpha there too, which is a deliberate
/// deviation — it keeps every colour in this crate opaque-ARGB, and vanilla's
/// writer never takes that arm.
pub fn parse_color(n: &Nbt) -> Option<i32> {
    match n {
        Nbt::String(s) => parse_hex_color(s),
        // RGB_COLOR_CODEC fallback: a bare int (kept opaque for uniformity).
        Nbt::Int(v) => Some((*v as u32 | 0xFF00_0000) as i32),
        _ => None,
    }
}

/// An `attributes` color value: the bare override (string/int), OR a
/// `{argument, modifier}` compound. For `sky_color` / `fog_color` vanilla only
/// ever uses the override form; a modifier-form color is intentionally NOT
/// applied (documented deviation — it does not occur for sky/fog in vanilla).
fn parse_attr_color(n: &Nbt) -> Option<i32> {
    match n {
        Nbt::String(_) | Nbt::Int(_) => parse_color(n),
        _ => None,
    }
}

/// Parse one biome entry (`name` is the registry identifier).
pub fn parse_biome(name: &str, nbt: &Nbt) -> BiomeDef {
    let temperature = nbt.get("temperature").and_then(as_f32).unwrap_or(0.5);
    let downfall = nbt.get("downfall").and_then(as_f32).unwrap_or(0.5);
    // `ClimateSettings.hasPrecipitation` — a Byte on the wire. Its codec has
    // no default, so a real server always sends it; absent, the safer read is
    // "it rains", matching every vanilla biome but the deserts and the Nether.
    let has_precipitation = nbt
        .get("has_precipitation")
        .and_then(as_bool)
        .unwrap_or(true);
    // Optional, and absent for all but the frozen-ocean family.
    let temperature_modifier = nbt
        .get("temperature_modifier")
        .and_then(Nbt::as_str)
        .map(TemperatureModifier::from_name)
        .unwrap_or_default();

    let effects = nbt.get("effects");
    let eff = |k: &str| effects.and_then(|e| e.get(k));
    let water_color = eff("water_color")
        .and_then(parse_color)
        .unwrap_or(DEFAULT_WATER);
    let grass_override = eff("grass_color").and_then(parse_color);
    let foliage_override = eff("foliage_color").and_then(parse_color);
    let dry_foliage_override = eff("dry_foliage_color").and_then(parse_color);
    let grass_modifier = eff("grass_color_modifier")
        .and_then(Nbt::as_str)
        .map(GrassModifier::parse)
        .unwrap_or(GrassModifier::None);

    let (sky_color, fog_color) = attribute_sky_fog(nbt.get("attributes"));
    let ambient_sounds = attribute_ambient_sounds(nbt.get("attributes"));
    let background_music = attribute_background_music(nbt.get("attributes"));

    BiomeDef {
        name: name.to_string(),
        temperature,
        downfall,
        water_color,
        grass_override,
        foliage_override,
        dry_foliage_override,
        grass_modifier,
        sky_color,
        fog_color,
        has_precipitation,
        temperature_modifier,
        ambient_sounds,
        background_music,
    }
}

/// One `SoundEvent.CODEC` value. That codec is a `Holder<SoundEvent>` codec, so
/// a registry sound writes as its bare identifier STRING; the inline form is a
/// compound carrying `sound_id`. Only the former occurs in a vanilla biome, and
/// the latter is read rather than rejected because a data pack can produce it.
fn parse_sound_ref(n: &Nbt) -> Option<String> {
    match n {
        Nbt::String(s) => Some(s.clone()),
        Nbt::Compound(_) => n.get("sound_id").and_then(Nbt::as_str).map(str::to_string),
        _ => None,
    }
}

/// `ExtraCodecs.compactListCodec` — **a single element serialises as the bare
/// element, not as a one-element list**, and that is the form every vanilla
/// biome with additions takes (`nether_wastes.json` writes `"additions": {…}`).
/// A reader that only accepts a List therefore sees no additions at all on
/// every real server, which is silence rather than an error.
fn parse_additions(n: Option<&Nbt>) -> Vec<AmbientAddition> {
    let Some(n) = n else {
        return Vec::new();
    };
    let one = |e: &Nbt| -> Option<AmbientAddition> {
        Some(AmbientAddition {
            sound: e.get("sound").and_then(parse_sound_ref)?,
            // `Codec.DOUBLE`; accepted liberally for the same reason `as_f32`
            // is, since a hand-written pack may write a float.
            tick_chance: e.get("tick_chance").and_then(as_f64)?,
        })
    };
    match n {
        Nbt::List(items) => items.iter().filter_map(one).collect(),
        other => one(other).into_iter().collect(),
    }
}

fn as_f64(n: &Nbt) -> Option<f64> {
    match n {
        Nbt::Double(v) => Some(*v),
        Nbt::Float(v) => Some(*v as f64),
        Nbt::Int(v) => Some(*v as f64),
        _ => None,
    }
}

fn as_i32(n: &Nbt) -> Option<i32> {
    match n {
        Nbt::Int(v) => Some(*v),
        Nbt::Short(v) => Some(*v as i32),
        Nbt::Byte(v) => Some(*v as i32),
        _ => None,
    }
}

/// `AmbientMoodSettings.CODEC` — every field is required, so a compound
/// missing one is not a mood. The JSON key for `soundPositionOffset` is
/// **`offset`**, not `sound_position_offset`.
fn parse_mood(n: &Nbt) -> Option<AmbientMood> {
    Some(AmbientMood {
        sound: n.get("sound").and_then(parse_sound_ref)?,
        tick_delay: n.get("tick_delay").and_then(as_i32)?,
        block_search_extent: n.get("block_search_extent").and_then(as_i32)?,
        sound_position_offset: n.get("offset").and_then(as_f64)?,
    })
}

/// Extract the `audio/ambient_sounds` override from an `attributes` compound
/// (biome **or** dimension type — they share the attribute map's shape, which
/// is why this takes the compound rather than a biome).
///
/// Returns `None` when the attribute is absent, which means *inherit*, and
/// `Some(EMPTY)` only if a pack explicitly declares an empty record — the two
/// are different, and collapsing them silences the Overworld's caves for any
/// biome that declares one.
pub fn attribute_ambient_sounds(attributes: Option<&Nbt>) -> Option<AmbientSounds> {
    let v = attributes?.get("minecraft:audio/ambient_sounds")?;
    // The bare-override form. The `{argument, modifier}` form is not applied,
    // for the same stated reason as `parse_attr_color`: no vanilla biome or
    // dimension writes one for this attribute, and guessing at a modifier's
    // semantics for a record type would be worse than inheriting.
    if !matches!(v, Nbt::Compound(_)) || v.get("modifier").is_some() {
        return None;
    }
    Some(AmbientSounds {
        loop_sound: v.get("loop").and_then(parse_sound_ref),
        mood: v.get("mood").and_then(parse_mood),
        additions: parse_additions(v.get("additions")),
    })
}

/// Extract the `audio/background_music` override from an `attributes` compound
/// (biome **or** dimension type), the sibling of
/// [`attribute_ambient_sounds`] (M145).
///
/// `None` means *inherit* and `Some(EMPTY)` means *silence*, which are
/// different for the same reason they are on the ambient attribute — and here
/// the difference is reachable in vanilla rather than hypothetical:
/// `OverworldBiomes.java:596` sets `BackgroundMusic.EMPTY` on a biome
/// explicitly, so collapsing the two gives that biome the Overworld's music.
pub fn attribute_background_music(attributes: Option<&Nbt>) -> Option<BackgroundMusic> {
    let v = attributes?.get("minecraft:audio/background_music")?;
    // The bare-override form only, exactly as `attribute_ambient_sounds`: no
    // vanilla biome or dimension writes a `{argument, modifier}` for this, and
    // guessing at a modifier's semantics for a record type would be worse than
    // inheriting.
    if !matches!(v, Nbt::Compound(_)) || v.get("modifier").is_some() {
        return None;
    }
    Some(BackgroundMusic {
        default_music: v.get("default").and_then(parse_music),
        creative_music: v.get("creative").and_then(parse_music),
        underwater_music: v.get("underwater").and_then(parse_music),
    })
}

/// One `Music` record — `sound`, `min_delay`, `max_delay`,
/// `replace_current_music`.
///
/// **`replace_current_music` defaults to `false`** (`Music.CODEC` uses
/// `optionalFieldOf("replace_current_music", false)`), and the delays are
/// required. A record missing either delay is malformed rather than defaulted,
/// so it yields `None` — inheriting is a better answer than inventing a window
/// that decides how often the track plays.
fn parse_music(v: &Nbt) -> Option<Music> {
    let sound = parse_sound_ref(v.get("sound")?)?;
    Some(Music {
        sound,
        min_delay: v.get("min_delay").and_then(Nbt::as_i64)? as i32,
        max_delay: v.get("max_delay").and_then(Nbt::as_i64)? as i32,
        replace_current_music: v
            .get("replace_current_music")
            .and_then(as_bool)
            .unwrap_or(false),
    })
}

/// Extract `visual/sky_color` + `visual/fog_color` overrides from an
/// `attributes` compound (biome or dimension). Returns `(sky, fog)`.
pub fn attribute_sky_fog(attributes: Option<&Nbt>) -> (Option<i32>, Option<i32>) {
    let Some(attrs) = attributes else {
        return (None, None);
    };
    let sky = attrs
        .get("minecraft:visual/sky_color")
        .and_then(parse_attr_color);
    let fog = attrs
        .get("minecraft:visual/fog_color")
        .and_then(parse_attr_color);
    (sky, fog)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Nbt {
        Nbt::String(v.into())
    }

    #[test]
    fn hex_color_is_opaque() {
        assert_eq!(parse_hex_color("#3f76e4"), Some(0xFF3F76E4u32 as i32));
        assert_eq!(parse_hex_color("#000000"), Some(0xFF000000u32 as i32));
        assert_eq!(parse_hex_color("nope"), None);
        assert_eq!(parse_hex_color("#12345"), None); // wrong length
        assert_eq!(parse_hex_color("#zzzzzz"), None); // non-hex
    }

    #[test]
    fn parses_plains_shape() {
        // Mirrors the captured plains biome NBT.
        let biome = Nbt::Compound(vec![
            (
                "effects".into(),
                Nbt::Compound(vec![("water_color".into(), s("#3f76e4"))]),
            ),
            ("has_precipitation".into(), Nbt::Byte(1)),
            ("temperature".into(), Nbt::Float(0.8)),
            ("downfall".into(), Nbt::Float(0.4)),
            (
                "attributes".into(),
                Nbt::Compound(vec![("minecraft:visual/sky_color".into(), s("#78a7ff"))]),
            ),
        ]);
        let def = parse_biome("minecraft:plains", &biome);
        assert_eq!(def.name, "minecraft:plains");
        assert_eq!(def.temperature, 0.8);
        assert_eq!(def.downfall, 0.4);
        assert_eq!(def.water_color, 0xFF3F76E4u32 as i32);
        assert_eq!(def.grass_modifier, GrassModifier::None);
        assert_eq!(def.grass_override, None);
        assert_eq!(def.sky_color, Some(0xFF78A7FFu32 as i32));
        assert_eq!(def.fog_color, None);
    }

    #[test]
    fn parses_swamp_shape() {
        // Swamp: grass modifier + foliage/dry overrides + a water_fog attribute
        // in {argument, modifier} form (which we ignore, correctly).
        let biome = Nbt::Compound(vec![
            (
                "effects".into(),
                Nbt::Compound(vec![
                    ("dry_foliage_color".into(), s("#7b5334")),
                    ("grass_color_modifier".into(), s("swamp")),
                    ("foliage_color".into(), s("#6a7039")),
                    ("water_color".into(), s("#617b64")),
                ]),
            ),
            ("temperature".into(), Nbt::Float(0.8)),
            ("downfall".into(), Nbt::Float(0.9)),
            (
                "attributes".into(),
                Nbt::Compound(vec![
                    ("minecraft:visual/sky_color".into(), s("#78a7ff")),
                    (
                        "minecraft:visual/water_fog_end_distance".into(),
                        Nbt::Compound(vec![
                            ("argument".into(), Nbt::Float(0.85)),
                            ("modifier".into(), s("multiply")),
                        ]),
                    ),
                ]),
            ),
        ]);
        let def = parse_biome("minecraft:swamp", &biome);
        assert_eq!(def.grass_modifier, GrassModifier::Swamp);
        assert_eq!(def.foliage_override, Some(0xFF6A7039u32 as i32));
        assert_eq!(def.dry_foliage_override, Some(0xFF7B5334u32 as i32));
        assert_eq!(def.water_color, 0xFF617B64u32 as i32);
        assert_eq!(def.sky_color, Some(0xFF78A7FFu32 as i32));
    }

    #[test]
    fn dimension_base_sky_fog() {
        let attrs = Nbt::Compound(vec![
            ("minecraft:visual/fog_color".into(), s("#c0d8ff")),
            ("minecraft:visual/sky_color".into(), s("#78a7ff")),
        ]);
        let (sky, fog) = attribute_sky_fog(Some(&attrs));
        assert_eq!(sky, Some(0xFF78A7FFu32 as i32));
        assert_eq!(fog, Some(0xFFC0D8FFu32 as i32));
    }
}

#[cfg(test)]
mod ambient_tests {
    use super::*;

    fn s(v: &str) -> Nbt {
        Nbt::String(v.into())
    }

    /// The real `nether_wastes` shape, transcribed from
    /// `data/minecraft/worldgen/biome/nether_wastes.json` — the only kind of
    /// biome in vanilla that sets this attribute at all.
    ///
    /// Note `additions` is a **bare object**, not a one-element array:
    /// `ExtraCodecs.compactListCodec` writes a single element that way, and
    /// that is the form every vanilla Nether biome ships. A reader that only
    /// accepts a List finds no additions on any real server — silence, with no
    /// error anywhere.
    fn nether_wastes_attributes() -> Nbt {
        Nbt::Compound(vec![(
            "minecraft:audio/ambient_sounds".into(),
            Nbt::Compound(vec![
                ("loop".into(), s("minecraft:ambient.nether_wastes.loop")),
                (
                    "mood".into(),
                    Nbt::Compound(vec![
                        ("sound".into(), s("minecraft:ambient.nether_wastes.mood")),
                        ("tick_delay".into(), Nbt::Int(6000)),
                        ("block_search_extent".into(), Nbt::Int(8)),
                        ("offset".into(), Nbt::Double(2.0)),
                    ]),
                ),
                (
                    "additions".into(),
                    Nbt::Compound(vec![
                        (
                            "sound".into(),
                            s("minecraft:ambient.nether_wastes.additions"),
                        ),
                        ("tick_chance".into(), Nbt::Double(0.0111)),
                    ]),
                ),
            ]),
        )])
    }

    #[test]
    fn a_nether_biome_decodes_all_three_features() {
        let attrs = nether_wastes_attributes();
        let a = attribute_ambient_sounds(Some(&attrs)).expect("present");
        assert_eq!(
            a.loop_sound.as_deref(),
            Some("minecraft:ambient.nether_wastes.loop")
        );
        let mood = a.mood.as_ref().expect("mood");
        assert_eq!(mood.sound, "minecraft:ambient.nether_wastes.mood");
        assert_eq!(mood.tick_delay, 6000);
        assert_eq!(mood.block_search_extent, 8);
        assert_eq!(mood.sound_position_offset, 2.0);
        // The compact form yields exactly ONE addition, not zero.
        assert_eq!(a.additions.len(), 1, "compactListCodec's bare-object form");
        assert_eq!(
            a.additions[0].sound,
            "minecraft:ambient.nether_wastes.additions"
        );
        assert_eq!(a.additions[0].tick_chance, 0.0111);
    }

    /// The list form must work too — a data pack with two additions writes an
    /// array, and `additions` is a genuine `List` whose entries each get their
    /// own per-tick Bernoulli trial.
    #[test]
    fn additions_accept_both_the_compact_and_the_list_form() {
        let two = Nbt::Compound(vec![(
            "minecraft:audio/ambient_sounds".into(),
            Nbt::Compound(vec![(
                "additions".into(),
                Nbt::List(vec![
                    Nbt::Compound(vec![
                        ("sound".into(), s("a")),
                        ("tick_chance".into(), Nbt::Double(0.5)),
                    ]),
                    Nbt::Compound(vec![
                        ("sound".into(), s("b")),
                        ("tick_chance".into(), Nbt::Double(0.25)),
                    ]),
                ]),
            )]),
        )]);
        let a = attribute_ambient_sounds(Some(&two)).expect("present");
        assert_eq!(a.additions.len(), 2);
        assert_eq!(a.additions[0].sound, "a");
        assert_eq!(a.additions[1].tick_chance, 0.25);
        // …and neither form invents a loop or a mood.
        assert!(a.loop_sound.is_none() && a.mood.is_none());
    }

    /// **The ambiguity that only an empty modifier library keeps safe.**
    ///
    /// `EnvironmentAttributeMap.Entry`'s codec is
    /// `Codec.either(attribute.valueCodec(), {modifier, argument})`. For a
    /// colour the two forms are distinguishable by NBT *type* — bare is a
    /// String, modifier is a Compound — which is how `parse_attr_color` tells
    /// them apart. **For `AmbientSounds` the bare form IS a compound**, and
    /// all three of its fields are `optionalFieldOf`, so a modifier compound
    /// decodes as a perfectly valid *empty* record instead of failing.
    ///
    /// Empty is not harmless: it is the difference between "inherit the
    /// dimension's cave mood" and "this biome declares silence". So the
    /// modifier form is reported as absent (inherit), not as an empty
    /// override.
    #[test]
    fn a_modifier_form_reads_as_absent_rather_than_as_an_empty_record() {
        let modifier_form = Nbt::Compound(vec![(
            "minecraft:audio/ambient_sounds".into(),
            Nbt::Compound(vec![
                ("modifier".into(), s("minecraft:override")),
                (
                    "argument".into(),
                    Nbt::Compound(vec![("loop".into(), s("minecraft:some.loop"))]),
                ),
            ]),
        )]);
        assert_eq!(attribute_ambient_sounds(Some(&modifier_form)), None);
        // The failure this guards: a reader that just projects the fields sees
        // no `loop`/`mood`/`additions` keys and returns Some(EMPTY), which
        // suppresses the dimension base.
        let empty_override = Nbt::Compound(vec![(
            "minecraft:audio/ambient_sounds".into(),
            Nbt::Compound(vec![]),
        )]);
        let explicit = attribute_ambient_sounds(Some(&empty_override)).expect("present");
        assert!(explicit.is_empty(), "an explicitly empty record IS empty");
        assert_ne!(
            attribute_ambient_sounds(Some(&modifier_form)),
            Some(explicit),
            "and it must not be confused with the modifier form"
        );
    }

    /// Absent means *inherit*, at three levels: no `attributes` compound at
    /// all, an `attributes` compound without the key, and a value of the wrong
    /// NBT type.
    #[test]
    fn absence_is_inherit_at_every_level() {
        assert_eq!(attribute_ambient_sounds(None), None);
        let no_key = Nbt::Compound(vec![("minecraft:visual/sky_color".into(), s("#78a7ff"))]);
        assert_eq!(attribute_ambient_sounds(Some(&no_key)), None);
        let wrong_type = Nbt::Compound(vec![("minecraft:audio/ambient_sounds".into(), s("nope"))]);
        assert_eq!(attribute_ambient_sounds(Some(&wrong_type)), None);
    }

    /// A mood missing any of its four mandatory fields is not a mood. The
    /// codec has no defaults, so a partial compound is malformed rather than a
    /// mood with a zero somewhere — and a zero `tick_delay` would be a divide
    /// by zero in the moodiness step.
    #[test]
    fn a_partial_mood_is_no_mood() {
        for missing in ["sound", "tick_delay", "block_search_extent", "offset"] {
            let mut fields: Vec<(String, Nbt)> = vec![
                ("sound".into(), s("minecraft:ambient.cave")),
                ("tick_delay".into(), Nbt::Int(6000)),
                ("block_search_extent".into(), Nbt::Int(8)),
                ("offset".into(), Nbt::Double(2.0)),
            ];
            fields.retain(|(k, _)| k != missing);
            let attrs = Nbt::Compound(vec![(
                "minecraft:audio/ambient_sounds".into(),
                Nbt::Compound(vec![("mood".into(), Nbt::Compound(fields))]),
            )]);
            let a = attribute_ambient_sounds(Some(&attrs)).expect("present");
            assert!(a.mood.is_none(), "missing {missing} should void the mood");
        }
    }

    /// The whole point of the biome projection: `parse_biome` reaches it.
    #[test]
    fn parse_biome_carries_the_attribute_through() {
        let biome = Nbt::Compound(vec![
            (
                "effects".into(),
                Nbt::Compound(vec![("water_color".into(), s("#3f76e4"))]),
            ),
            ("temperature".into(), Nbt::Float(2.0)),
            ("downfall".into(), Nbt::Float(0.0)),
            ("attributes".into(), nether_wastes_attributes()),
        ]);
        let def = parse_biome("minecraft:nether_wastes", &biome);
        let a = def.ambient_sounds.expect("the nether wastes declare one");
        assert!(a.loop_sound.is_some() && a.mood.is_some() && a.additions.len() == 1);
    }
}
