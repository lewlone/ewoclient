//! Parse the `minecraft:worldgen/biome` + `minecraft:dimension_type` registry
//! NBT into `rewo_world::biome` types.
//!
//! Wire shape (captured from the live 26.2 server's `registry_data`, matched to
//! the `Biome.NETWORK_CODEC` / `BiomeSpecialEffects.CODEC` /
//! `DimensionType.NETWORK_CODEC` codecs):
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
use rewo_world::biome::{BiomeDef, GrassModifier};

/// Default water color (`#3f76e4`, opaque) when a biome somehow omits it —
/// vanilla always sends it (`water_color` is a required field).
const DEFAULT_WATER: i32 = 0xFF3F_76E4u32 as i32;

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

/// A bare `STRING_RGB_COLOR` value: the `#rrggbb` string form (what vanilla
/// sends) or the `INT` fallback form.
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
    }
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
