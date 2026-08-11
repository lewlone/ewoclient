//! The **decompiled-JSON oracle** for `rewo dimensioncheck`.
//!
//! This module reads the real 26.2 datagen files under
//! `%APPDATA%/EwoClient/rewo/<version>/decompiled/data/minecraft/` and grades
//! them *independently of the production NBT parser*:
//!
//! - it never calls `rewo_net::dimension_parse`, never touches `rewo_proto::nbt`
//!   and never goes through the wire encoder — it walks `serde_json` values and
//!   applies the field rules itself, so a bug in the production parser cannot
//!   also corrupt its own oracle;
//! - it extracts every raw field the client consumes (`min_y`, `height`,
//!   `has_skylight`, `ambient_light`, optional `has_fixed_time`, optional
//!   `skybox` / `cardinal_light`, the `attributes` sky/fog/ambient/sky-light
//!   colours and the sky-light factor, and the `timelines` holder set);
//! - it applies a default **only** where the decompiled codec proves one, and it
//!   records which fields were defaulted so the report can name them;
//! - it resolves the `timelines` holder set through the datagen
//!   `data/minecraft/tags/timeline/*.json` files, so `has_day_timeline` is
//!   *derived from the shipped tag data* rather than asserted. `has_fixed_time`
//!   is read separately and plays no part in that derivation — the two fields
//!   are independent members of `DimensionType`.
//!
//! Everything fails closed: a missing directory, a missing file, a missing
//! required field, an unparseable colour, an unknown enum, an unresolvable tag —
//! all are errors naming the exact path, never a silent default.
//!
//! Codec ground truth (same decompile the production parser cites):
//! `net/minecraft/world/level/dimension/DimensionType.java`,
//! `net/minecraft/world/attribute/EnvironmentAttributes.java`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rewo_world::ambient::{AmbientAddition, AmbientMood, AmbientSounds};
use rewo_world::dimension::{CardinalLightType, DimensionShape, DimensionTypeDef, Skybox};
use serde_json::Value;

/// The attribute keys the client consumes, spelled out here rather than
/// imported: this oracle must be able to disagree with the production parser.
const K_SKY_COLOR: &str = "minecraft:visual/sky_color";
const K_AMBIENT_SOUNDS: &str = "minecraft:audio/ambient_sounds";
const K_FOG_COLOR: &str = "minecraft:visual/fog_color";
const K_AMBIENT_LIGHT_COLOR: &str = "minecraft:visual/ambient_light_color";
const K_SKY_LIGHT_COLOR: &str = "minecraft:visual/sky_light_color";
const K_SKY_LIGHT_FACTOR: &str = "minecraft:visual/sky_light_factor";
const K_CLOUD_COLOR: &str = "minecraft:visual/cloud_color";
const K_CLOUD_HEIGHT: &str = "minecraft:visual/cloud_height";

/// The timeline whose presence in the resolved holder set turns the day cycle
/// on. Grounded in `data/minecraft/timeline/day.json`.
const DAY_TIMELINE: &str = "minecraft:day";

// Independent codec defaults, transcribed directly from the 26.2 decompile:
// `EnvironmentAttributes.java` declares -16777216, -1 and 1.0F; the
// `DimensionType` codec declares false/OVERWORLD/DEFAULT. Do not import the
// production constants here: a wrong shared constant would let every leg of
// this oracle agree with the same bug.
const JSON_DEFAULT_AMBIENT_LIGHT_COLOR: i32 = 0xFF00_0000u32 as i32;
const JSON_DEFAULT_SKY_LIGHT_COLOR: i32 = 0xFFFF_FFFFu32 as i32;
const JSON_DEFAULT_SKY_LIGHT_FACTOR: f32 = 1.0;
/// `CLOUD_COLOR`'s attribute default — fully transparent, i.e. no clouds.
const JSON_DEFAULT_CLOUD_COLOR: i32 = 0;
const JSON_DEFAULT_CLOUD_HEIGHT: f32 = 192.33;

/// One `data/minecraft/dimension_type/*.json` file, read raw and graded.
#[derive(Clone, Debug, PartialEq)]
pub struct JsonDimension {
    pub name: String,
    pub path: PathBuf,
    pub min_y: i32,
    pub height: i32,
    pub has_sky_light: bool,
    pub ambient_light: f32,
    pub has_fixed_time: bool,
    pub skybox: Skybox,
    pub cardinal: CardinalLightType,
    pub sky_color: Option<i32>,
    pub fog_color: Option<i32>,
    pub ambient_light_color: i32,
    pub sky_light_color: i32,
    pub sky_light_factor: f32,
    /// `visual/cloud_color` — **ARGB**, alpha preserved. Read here rather than
    /// defaulted so the gate can actually catch a wrong cloud colour.
    pub cloud_color: i32,
    pub cloud_height: f32,
    /// `audio/ambient_sounds`. Parsed HERE, from the JSON, by hand — this
    /// module is the M16 gate's independent oracle and shares no code with
    /// `rewo_net::biome_parse`, so an agreement between the two is evidence
    /// rather than a tautology.
    pub ambient_sounds: Option<AmbientSounds>,
    /// The raw `timelines` entries, exactly as the file spells them.
    pub timelines_raw: Vec<String>,
    /// Every timeline id the holder set resolves to, tags expanded, sorted.
    pub timeline_ids: Vec<String>,
    pub has_day_timeline: bool,
    /// The fields this file omits, whose value below is a codec default. Named
    /// so the report can say which values were *proven present* and which were
    /// *proven absent*.
    pub defaulted: Vec<&'static str>,
}

impl JsonDimension {
    /// The definition this JSON file describes, in the same shape the network
    /// parser produces — so the two can be compared with `==` and no field can
    /// be quietly left out of the comparison.
    pub fn to_def(&self) -> DimensionTypeDef {
        DimensionTypeDef {
            name: self.name.clone(),
            shape: DimensionShape {
                min_y: self.min_y,
                height: self.height,
            },
            has_fixed_time: self.has_fixed_time,
            has_day_timeline: self.has_day_timeline,
            has_sky_light: self.has_sky_light,
            skybox: self.skybox,
            ambient_light: self.ambient_light,
            cardinal_light_type: self.cardinal,
            cardinal_light: self.cardinal.get(),
            sky_color: self.sky_color,
            fog_color: self.fog_color,
            ambient_light_color: self.ambient_light_color,
            sky_light_color: self.sky_light_color,
            sky_light_factor: self.sky_light_factor,
            cloud_color: self.cloud_color,
            cloud_height: self.cloud_height,
            ambient_sounds: self.ambient_sounds.clone(),
        }
    }

    /// Field-by-field comparison against a parsed definition. Returns the first
    /// disagreement, named — a whole-struct `!=` would only say "they differ".
    pub fn diff(&self, source: &str, holder: usize, d: &DimensionTypeDef) -> Result<(), String> {
        let want = self.to_def();
        let fail = |what: &str, got: String, expect: String| {
            Err(format!(
                "{source}[{holder}] {}: {what} is {got}, but {} says {expect}",
                d.name,
                self.path.display()
            ))
        };
        macro_rules! eq {
            ($what:literal, $got:expr, $want:expr) => {
                if $got != $want {
                    return fail($what, format!("{:?}", $got), format!("{:?}", $want));
                }
            };
        }
        eq!("registry name", d.name.as_str(), want.name.as_str());
        eq!("min_y", d.shape.min_y, want.shape.min_y);
        eq!("height", d.shape.height, want.shape.height);
        eq!(
            "section count",
            d.shape.section_count(),
            want.shape.section_count()
        );
        eq!("has_skylight", d.has_sky_light, want.has_sky_light);
        eq!("ambient_light", d.ambient_light, want.ambient_light);
        eq!("has_fixed_time", d.has_fixed_time, want.has_fixed_time);
        eq!("skybox", d.skybox, want.skybox);
        eq!(
            "cardinal_light",
            d.cardinal_light_type,
            want.cardinal_light_type
        );
        for face in 0..6 {
            eq!(
                "cardinal factor",
                d.cardinal_light.by_mesh_face(face),
                want.cardinal_light.by_mesh_face(face)
            );
        }
        eq!("sky_color", d.sky_color, want.sky_color);
        eq!("fog_color", d.fog_color, want.fog_color);
        eq!(
            "ambient_light_color",
            d.ambient_light_color,
            want.ambient_light_color
        );
        eq!("cloud_color", d.cloud_color, want.cloud_color);
        eq!("ambient_sounds", d.ambient_sounds, want.ambient_sounds);
        eq!("cloud_height", d.cloud_height, want.cloud_height);
        eq!("sky_light_color", d.sky_light_color, want.sky_light_color);
        eq!(
            "sky_light_factor",
            d.sky_light_factor,
            want.sky_light_factor
        );
        eq!(
            "has_day_timeline",
            d.has_day_timeline,
            want.has_day_timeline
        );
        // Nothing may be left out: if a field is ever added to
        // `DimensionTypeDef`, the whole-struct compare below fails until it is
        // graded above.
        if *d != want {
            return fail(
                "some field",
                format!("{d:?}"),
                format!("{want:?} (a field exists that this oracle does not grade)"),
            );
        }
        Ok(())
    }
}

// ------------------------------------------------------------------- loading

/// `%APPDATA%/EwoClient/rewo/<version>/decompiled/data/minecraft`.
pub fn default_data_root(version: &str) -> PathBuf {
    let mut p = dirs::config_dir().unwrap_or_default();
    p.push("EwoClient");
    p.push("rewo");
    p.push(version);
    p.push("decompiled");
    p.push("data");
    p.push("minecraft");
    p
}

/// Read and grade the named `dimension_type` entries from `data_root`, in the
/// given order. Every name must exist as `<data_root>/dimension_type/<path>.json`.
pub fn load(data_root: &Path, names: &[&str]) -> Result<Vec<JsonDimension>, String> {
    let dir = data_root.join("dimension_type");
    if !dir.is_dir() {
        return Err(format!(
            "decompiled dimension_type directory {} does not exist — this gate grades \
             the registry against the shipped 26.2 datagen JSON and refuses to run \
             without it (pass --decompiled <data/minecraft dir>)",
            dir.display()
        ));
    }
    // A file we do not grade would be a dimension the oracle silently ignores.
    let mut on_disk: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".json") {
            on_disk.push(format!("minecraft:{stem}"));
        }
    }
    on_disk.sort();
    let mut wanted: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    wanted.sort();
    if on_disk != wanted {
        return Err(format!(
            "{} holds {on_disk:?}, but this gate grades {wanted:?} — an ungraded \
             dimension_type file is a dimension the oracle cannot see",
            dir.display()
        ));
    }

    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let path = dir.join(format!("{}.json", short_path(name)?));
        out.push(load_one(data_root, name, &path)?);
    }
    Ok(out)
}

/// `minecraft:the_nether` -> `the_nether`. Any other namespace is not part of
/// the vanilla datagen tree and is rejected rather than guessed at.
fn short_path(name: &str) -> Result<&str, String> {
    name.strip_prefix("minecraft:")
        .ok_or_else(|| format!("`{name}` is not a minecraft: identifier"))
}

fn load_one(data_root: &Path, name: &str, path: &Path) -> Result<JsonDimension, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let root: Value = serde_json::from_str(&text)
        .map_err(|e| format!("{}: malformed JSON: {e}", path.display()))?;
    let obj = root
        .as_object()
        .ok_or_else(|| format!("{}: top level is not an object", path.display()))?;
    let at = |what: &str| format!("{}: {what}", path.display());

    let mut defaulted: Vec<&'static str> = Vec::new();

    // -- required (`fieldOf`, no codec default) -----------------------------
    let min_y = req_i32(obj, "min_y").map_err(|e| at(&e))?;
    let height = req_i32(obj, "height").map_err(|e| at(&e))?;
    let shape = DimensionShape { min_y, height };
    if !shape.is_valid() {
        return Err(at(&format!(
            "min_y {min_y} / height {height} is not a shape this client can index \
             (min_y multiple of 16 in range, height a positive multiple of 16)"
        )));
    }
    let has_sky_light = req_bool(obj, "has_skylight").map_err(|e| at(&e))?;
    let ambient_light = req_f32(obj, "ambient_light").map_err(|e| at(&e))?;

    // -- optional, with the codec's default ---------------------------------
    let has_fixed_time = match obj.get("has_fixed_time") {
        None => {
            defaulted.push("has_fixed_time");
            false
        }
        Some(v) => v
            .as_bool()
            .ok_or_else(|| at("has_fixed_time is not a boolean"))?,
    };
    let skybox = match obj.get("skybox") {
        None => {
            defaulted.push("skybox");
            Skybox::Overworld
        }
        Some(v) => {
            let s = v.as_str().ok_or_else(|| at("skybox is not a string"))?;
            // Independent table: the three `Skybox` names the decompiled
            // `StringRepresentable` declares.
            match s {
                "none" => Skybox::None,
                "overworld" => Skybox::Overworld,
                "end" => Skybox::End,
                other => return Err(at(&format!("skybox `{other}` is not a known variant"))),
            }
        }
    };
    let cardinal = match obj.get("cardinal_light") {
        None => {
            defaulted.push("cardinal_light");
            CardinalLightType::Default
        }
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| at("cardinal_light is not a string"))?;
            match s {
                "default" => CardinalLightType::Default,
                "nether" => CardinalLightType::Nether,
                other => {
                    return Err(at(&format!(
                        "cardinal_light `{other}` is not a known variant"
                    )))
                }
            }
        }
    };

    // -- attributes ---------------------------------------------------------
    let attributes = match obj.get("attributes") {
        None => {
            defaulted.push("attributes");
            None
        }
        Some(v) => Some(
            v.as_object()
                .ok_or_else(|| at("attributes is not an object"))?,
        ),
    };
    let attr = |key: &str| attributes.and_then(|a| a.get(key));
    // sky/fog keep their `Option`: "absent" is not "black".
    let sky_color = opt_color(attr(K_SKY_COLOR), K_SKY_COLOR).map_err(|e| at(&e))?;
    if sky_color.is_none() {
        defaulted.push("attributes.visual/sky_color (absent, NOT black)");
    }
    let fog_color = opt_color(attr(K_FOG_COLOR), K_FOG_COLOR).map_err(|e| at(&e))?;
    if fog_color.is_none() {
        defaulted.push("attributes.visual/fog_color (absent, NOT black)");
    }
    let ambient_sounds = json_ambient_sounds(attr(K_AMBIENT_SOUNDS)).map_err(|e| at(&e))?;
    if ambient_sounds.is_none() {
        defaulted.push("attributes.audio/ambient_sounds (absent = SILENT, and the Nether really does declare nothing)");
    }
    let ambient_light_color =
        match opt_color(attr(K_AMBIENT_LIGHT_COLOR), K_AMBIENT_LIGHT_COLOR).map_err(|e| at(&e))? {
            Some(v) => v,
            None => {
                defaulted.push("attributes.visual/ambient_light_color");
                JSON_DEFAULT_AMBIENT_LIGHT_COLOR
            }
        };
    let sky_light_color =
        match opt_color(attr(K_SKY_LIGHT_COLOR), K_SKY_LIGHT_COLOR).map_err(|e| at(&e))? {
            Some(v) => v,
            None => {
                defaulted.push("attributes.visual/sky_light_color");
                JSON_DEFAULT_SKY_LIGHT_COLOR
            }
        };
    // ARGB, so the 8-digit reader — the Overworld's `#ccffffff` is 80% opaque
    // and a 6-digit reader would reject it outright.
    let cloud_color = match opt_argb(attr(K_CLOUD_COLOR), K_CLOUD_COLOR).map_err(|e| at(&e))? {
        Some(v) => v,
        None => {
            defaulted.push("attributes.visual/cloud_color (absent = NO CLOUDS)");
            JSON_DEFAULT_CLOUD_COLOR
        }
    };
    let cloud_height = match attr(K_CLOUD_HEIGHT) {
        None => {
            defaulted.push("attributes.visual/cloud_height");
            JSON_DEFAULT_CLOUD_HEIGHT
        }
        Some(Value::Object(_)) => {
            return Err(at(&format!(
                "attribute `{K_CLOUD_HEIGHT}` uses the {{modifier, argument}} form,                  which the client does not model"
            )))
        }
        Some(v) => v
            .as_f64()
            .ok_or_else(|| at(&format!("attribute `{K_CLOUD_HEIGHT}` is not a number")))?
            as f32,
    };
    let sky_light_factor = match attr(K_SKY_LIGHT_FACTOR) {
        None => {
            defaulted.push("attributes.visual/sky_light_factor");
            JSON_DEFAULT_SKY_LIGHT_FACTOR
        }
        Some(Value::Object(_)) => {
            return Err(at(&format!(
                "attribute `{K_SKY_LIGHT_FACTOR}` uses the {{modifier, argument}} form, \
                 which the client does not model"
            )))
        }
        Some(v) => {
            let n = v
                .as_f64()
                .ok_or_else(|| at(&format!("attribute `{K_SKY_LIGHT_FACTOR}` is not a number")))?
                as f32;
            // `AttributeTypes.FLOAT` with `valueRange(UNIT_FLOAT)` validates.
            if !(0.0..=1.0).contains(&n) {
                return Err(at(&format!(
                    "attribute `{K_SKY_LIGHT_FACTOR}` = {n} is outside its unit range"
                )));
            }
            n
        }
    };

    // -- timelines, resolved through the shipped tag files ------------------
    let timelines_raw = match obj.get("timelines") {
        None => {
            defaulted.push("timelines (empty holder set)");
            Vec::new()
        }
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| at("a timelines entry is not a string"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(at("timelines is neither a string nor a list")),
    };
    let mut timeline_ids = BTreeSet::new();
    for entry in &timelines_raw {
        resolve_timeline(data_root, entry, &mut timeline_ids, &mut BTreeSet::new(), 0)?;
    }
    let has_day_timeline = timeline_ids.contains(DAY_TIMELINE);
    let timeline_ids: Vec<String> = timeline_ids.into_iter().collect();

    Ok(JsonDimension {
        name: name.to_string(),
        path: path.to_path_buf(),
        min_y,
        height,
        has_sky_light,
        ambient_light,
        has_fixed_time,
        skybox,
        cardinal,
        sky_color,
        fog_color,
        ambient_light_color,
        sky_light_color,
        sky_light_factor,
        cloud_color,
        cloud_height,
        ambient_sounds,
        timelines_raw,
        timeline_ids,
        has_day_timeline,
        defaulted,
    })
}

// ------------------------------------------------------------------ timelines

/// Expand one holder-set entry into concrete timeline ids.
///
/// `#namespace:path` is a tag, read from `<data_root>/tags/timeline/<path>.json`
/// and expanded recursively; anything else is a direct timeline id, which must
/// exist as `<data_root>/timeline/<path>.json`. Both existence checks are what
/// make this a *proof* from the shipped files rather than a restatement: a tag
/// that stopped containing `minecraft:day` changes the answer here.
fn resolve_timeline(
    data_root: &Path,
    entry: &str,
    out: &mut BTreeSet<String>,
    seen: &mut BTreeSet<String>,
    depth: usize,
) -> Result<(), String> {
    if depth > 16 {
        return Err(format!("timeline tag `{entry}` nests more than 16 deep"));
    }
    let Some(tag) = entry.strip_prefix('#') else {
        // A direct timeline. It must be a real file, or the holder set names
        // something the datagen tree does not ship.
        let path = data_root
            .join("timeline")
            .join(format!("{}.json", short_path(entry)?));
        if !path.is_file() {
            return Err(format!(
                "timeline `{entry}` has no file at {} — the holder set names a timeline \
                 the decompiled data does not ship",
                path.display()
            ));
        }
        out.insert(entry.to_string());
        return Ok(());
    };
    if !seen.insert(tag.to_string()) {
        return Err(format!("timeline tag `#{tag}` is cyclic"));
    }
    let path = data_root
        .join("tags")
        .join("timeline")
        .join(format!("{}.json", short_path(tag)?));
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "timeline tag `#{tag}`: {}: {e} — the day-cycle mapping is derived from these \
             tag files and cannot be assumed",
            path.display()
        )
    })?;
    let root: Value = serde_json::from_str(&text)
        .map_err(|e| format!("{}: malformed JSON: {e}", path.display()))?;
    let values = root
        .get("values")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{}: no `values` array", path.display()))?;
    for v in values {
        // Both the plain-string and the `{id, required}` forms tags may use.
        let id = match v {
            Value::String(s) => s.as_str(),
            Value::Object(o) => o
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{}: a values entry has no string `id`", path.display()))?,
            _ => {
                return Err(format!(
                    "{}: a values entry is not a tag entry",
                    path.display()
                ))
            }
        };
        resolve_timeline(data_root, id, out, seen, depth + 1)?;
    }
    seen.remove(tag);
    Ok(())
}

// ------------------------------------------------------------ field readers

fn req_i32(obj: &serde_json::Map<String, Value>, key: &str) -> Result<i32, String> {
    let v = obj
        .get(key)
        .ok_or_else(|| format!("missing required field `{key}`"))?;
    let n = v
        .as_i64()
        .ok_or_else(|| format!("field `{key}` is not an integer"))?;
    i32::try_from(n).map_err(|_| format!("field `{key}` = {n} does not fit in an i32"))
}

fn req_bool(obj: &serde_json::Map<String, Value>, key: &str) -> Result<bool, String> {
    obj.get(key)
        .ok_or_else(|| format!("missing required field `{key}`"))?
        .as_bool()
        .ok_or_else(|| format!("field `{key}` is not a boolean"))
}

fn req_f32(obj: &serde_json::Map<String, Value>, key: &str) -> Result<f32, String> {
    Ok(obj
        .get(key)
        .ok_or_else(|| format!("missing required field `{key}`"))?
        .as_f64()
        .ok_or_else(|| format!("field `{key}` is not a number"))? as f32)
}

/// `ExtraCodecs.STRING_RGB_COLOR`: `hexColor(6)` made opaque by `ARGB::opaque`,
/// with a bare-int alternative. Written out here rather than reusing the
/// network path's colour reader, so the two can disagree.
/// The ARGB twin of [`opt_color`]: `hexColor(8)`, alpha **kept**.
fn opt_argb(v: Option<&Value>, key: &str) -> Result<Option<i32>, String> {
    let Some(v) = v else { return Ok(None) };
    match v {
        Value::Object(_) => Err(format!(
            "attribute `{key}` uses the {{modifier, argument}} form, which the client              does not model"
        )),
        Value::String(s) => {
            let hex = s.strip_prefix('#').unwrap_or(s);
            if hex.len() != 8 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!(
                    "attribute `{key}` = `{s}` is not an 8-digit #aarrggbb colour"
                ));
            }
            Ok(Some(u32::from_str_radix(hex, 16)
                .map_err(|e| format!("attribute `{key}` = `{s}`: {e}"))?
                as i32))
        }
        Value::Number(n) => Ok(Some(
            n.as_i64()
                .ok_or_else(|| format!("attribute `{key}` is not an integer colour"))?
                as i32,
        )),
        _ => Err(format!("attribute `{key}` is not a colour")),
    }
}

fn opt_color(v: Option<&Value>, key: &str) -> Result<Option<i32>, String> {
    let Some(v) = v else { return Ok(None) };
    match v {
        Value::Object(_) => Err(format!(
            "attribute `{key}` uses the {{modifier, argument}} form, which the client \
             does not model"
        )),
        Value::String(s) => {
            let hex = s.strip_prefix('#').unwrap_or(s);
            if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!(
                    "attribute `{key}` = `{s}` is not a 6-digit #rrggbb colour"
                ));
            }
            let rgb = u32::from_str_radix(hex, 16)
                .map_err(|e| format!("attribute `{key}` = `{s}`: {e}"))?;
            Ok(Some((rgb | 0xFF00_0000) as i32))
        }
        Value::Number(n) => {
            let raw = n
                .as_i64()
                .ok_or_else(|| format!("attribute `{key}` is not an integer colour"))?;
            Ok(Some(((raw as u32) | 0xFF00_0000) as i32))
        }
        _ => Err(format!("attribute `{key}` is not a colour")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four entries this gate grades, read from the real decompiled tree.
    /// Fails closed — there is no "the files were not there so we skipped"
    /// arm, because that is exactly the hole this oracle exists to close.
    fn decompiled() -> Vec<JsonDimension> {
        load(
            &default_data_root("26.2"),
            &[
                "minecraft:overworld",
                "minecraft:overworld_caves",
                "minecraft:the_end",
                "minecraft:the_nether",
            ],
        )
        .expect("the decompiled dimension_type JSON must be readable")
    }

    /// Every raw field the client consumes, read out of the shipped files.
    /// If a datagen file changes, this is the first thing that fails.
    #[test]
    fn the_decompiled_json_holds_the_fields_this_client_consumes() {
        let dims = decompiled();
        assert_eq!(dims.len(), 4);

        let ow = &dims[0];
        assert_eq!(ow.name, "minecraft:overworld");
        assert_eq!((ow.min_y, ow.height), (-64, 384));
        assert!(ow.has_sky_light);
        assert_eq!(ow.ambient_light, 0.0);
        assert!(!ow.has_fixed_time, "overworld.json omits has_fixed_time");
        assert_eq!(
            ow.skybox,
            Skybox::Overworld,
            "absent skybox → codec default"
        );
        assert_eq!(ow.cardinal, CardinalLightType::Default);
        assert_eq!(ow.sky_color, Some(0xFF78_A7FFu32 as i32));
        assert_eq!(ow.fog_color, Some(0xFFC0_D8FFu32 as i32));
        assert_eq!(ow.ambient_light_color, 0xFF0A_0A0Au32 as i32);
        assert_eq!(ow.sky_light_color, JSON_DEFAULT_SKY_LIGHT_COLOR);
        assert_eq!(ow.sky_light_factor, JSON_DEFAULT_SKY_LIGHT_FACTOR);
        assert!(ow.has_day_timeline);
        assert!(ow.defaulted.contains(&"skybox"));
        assert!(ow.defaulted.contains(&"has_fixed_time"));

        // Caves differs only in `has_ceiling`, which the client never reads.
        let caves = &dims[1];
        assert_eq!(caves.name, "minecraft:overworld_caves");
        assert_eq!(caves.to_def(), {
            let mut d = ow.to_def();
            d.name = caves.name.clone();
            d
        });

        let end = &dims[2];
        assert_eq!((end.min_y, end.height), (0, 256));
        assert!(end.has_sky_light, "the End has a sky light engine");
        assert_eq!(end.ambient_light, 0.25);
        assert!(end.has_fixed_time);
        assert_eq!(end.skybox, Skybox::End);
        assert_eq!(end.sky_color, Some(0xFF00_0000u32 as i32));
        assert_eq!(end.fog_color, Some(0xFF18_1318u32 as i32));
        assert_eq!(end.ambient_light_color, 0xFF3F_473Fu32 as i32);
        assert_eq!(end.sky_light_color, 0xFFAC_60CDu32 as i32);
        assert_eq!(end.sky_light_factor, 0.0);
        assert!(!end.has_day_timeline);

        let nether = &dims[3];
        assert_eq!((nether.min_y, nether.height), (0, 256));
        assert!(!nether.has_sky_light);
        assert_eq!(nether.ambient_light, 0.1);
        assert!(nether.has_fixed_time);
        assert_eq!(nether.skybox, Skybox::None);
        assert_eq!(nether.cardinal, CardinalLightType::Nether);
        // Absent, not black: the_nether.json carries no sky/fog colour at all.
        assert_eq!(nether.sky_color, None);
        assert_eq!(nether.fog_color, None);
        assert_eq!(nether.ambient_light_color, 0xFF30_2821u32 as i32);
        assert_eq!(nether.sky_light_color, 0xFF7A_7AFFu32 as i32);
        assert_eq!(nether.sky_light_factor, 0.0);
        assert!(!nether.has_day_timeline);
    }

    /// `has_day_timeline` is decided by the `timelines` holder set, and
    /// `has_fixed_time` by its own field: the End and the Nether both fix time
    /// *and* have no day timeline, but nothing in the derivation links them —
    /// the Overworld proves the "no fixed time, day timeline" corner and the
    /// resolution below never reads `has_fixed_time` at all.
    #[test]
    fn the_day_timeline_comes_from_the_timelines_tag_not_has_fixed_time() {
        let dims = decompiled();
        for d in &dims {
            let expanded_day = d.timeline_ids.iter().any(|t| t == DAY_TIMELINE);
            assert_eq!(d.has_day_timeline, expanded_day);
        }
        assert_eq!(dims[0].timelines_raw, vec!["#minecraft:in_overworld"]);
        assert_eq!(dims[2].timelines_raw, vec!["#minecraft:in_end"]);
        assert_eq!(dims[3].timelines_raw, vec!["#minecraft:in_nether"]);
        // The Overworld: day cycle on, time not fixed.
        assert!(dims[0].has_day_timeline && !dims[0].has_fixed_time);
        // The End / Nether: time fixed, day cycle off — but for the separate
        // reason that their tag simply does not contain `minecraft:day`.
        assert!(dims[2].has_fixed_time && !dims[2].has_day_timeline);
        assert!(dims[3].has_fixed_time && !dims[3].has_day_timeline);
        assert!(!dims[2].timeline_ids.contains(&DAY_TIMELINE.to_string()));
        // `#minecraft:universal` is shared by all three and carries no day.
        for d in &dims {
            assert!(
                d.timeline_ids
                    .contains(&"minecraft:villager_schedule".to_string()),
                "{}: {:?}",
                d.name,
                d.timeline_ids
            );
        }
    }

    /// The shipped tag tree is what proves the day-cycle mapping: only
    /// `#minecraft:in_overworld` expands to `minecraft:day`, and it does so
    /// without reference to `has_fixed_time`.
    #[test]
    fn the_timeline_tags_decide_the_day_cycle() {
        let root = default_data_root("26.2");
        let mut ids = BTreeSet::new();
        resolve_timeline(
            &root,
            "#minecraft:in_overworld",
            &mut ids,
            &mut BTreeSet::new(),
            0,
        )
        .unwrap();
        assert!(ids.contains(DAY_TIMELINE), "{ids:?}");
        for tag in ["#minecraft:in_nether", "#minecraft:in_end"] {
            let mut ids = BTreeSet::new();
            resolve_timeline(&root, tag, &mut ids, &mut BTreeSet::new(), 0).unwrap();
            assert!(!ids.contains(DAY_TIMELINE), "{tag}: {ids:?}");
        }
    }

    /// A holder set naming a timeline the tree does not ship is an error, not a
    /// quiet `false`.
    #[test]
    fn an_unshipped_timeline_is_an_error() {
        let root = default_data_root("26.2");
        let mut ids = BTreeSet::new();
        assert!(resolve_timeline(
            &root,
            "minecraft:no_such_timeline",
            &mut ids,
            &mut BTreeSet::new(),
            0
        )
        .is_err());
    }

    #[test]
    fn a_missing_directory_fails_closed() {
        let missing = std::env::temp_dir().join("rewo-no-such-decompile");
        let _ = std::fs::remove_dir_all(&missing);
        let err = load(&missing, &["minecraft:overworld"]).unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
    }

    #[test]
    fn colours_are_six_digit_opaque_hex() {
        assert_eq!(
            opt_color(Some(&Value::String("#78a7ff".into())), "k").unwrap(),
            Some(0xFF78_A7FFu32 as i32)
        );
        assert_eq!(opt_color(None, "k").unwrap(), None);
        // The 8-digit form (`cloud_color`) is not what the consumed keys use.
        assert!(opt_color(Some(&Value::String("#ccffffff".into())), "k").is_err());
        assert!(opt_color(Some(&Value::String("#zz00ff".into())), "k").is_err());
        assert!(opt_color(Some(&serde_json::json!({"modifier": "x"})), "k").is_err());
    }
}

/// `audio/ambient_sounds` out of a dimension-type JSON file.
///
/// Hand-written against `AmbientSounds.CODEC` rather than shared with the
/// network parser, because this module is the gate's *independent* oracle.
/// Two differences from the NBT side are deliberate and are what make the
/// agreement meaningful: the JSON reader is **strict** (a malformed record is
/// an error, not a silent `None`), and it re-derives the `compactListCodec`
/// rule from the file rather than inheriting the other reader's opinion of it.
fn json_ambient_sounds(v: Option<&Value>) -> Result<Option<AmbientSounds>, String> {
    let Some(v) = v else { return Ok(None) };
    let obj = v
        .as_object()
        .ok_or_else(|| format!("{K_AMBIENT_SOUNDS} is not an object"))?;
    // The `{argument, modifier}` form. `ofNotInterpolated`'s one-arg overload
    // supplies an EMPTY modifier library, so OVERRIDE is the only legal
    // modifier and this form never appears — but every field of `AmbientSounds`
    // is optional, so a modifier compound would decode as a *valid empty
    // record* rather than failing. Rejecting it explicitly is the difference
    // between "inherit the base" and "this biome declares silence".
    if obj.contains_key("modifier") || obj.contains_key("argument") {
        return Err(format!("{K_AMBIENT_SOUNDS} is a modifier form, not a value"));
    }
    let sound = |x: &Value| -> Result<String, String> {
        x.as_str()
            .map(str::to_string)
            .ok_or_else(|| "a sound must be a bare identifier string".to_string())
    };
    let loop_sound = match obj.get("loop") {
        Some(x) => Some(sound(x)?),
        None => None,
    };
    let mood = match obj.get("mood") {
        None => None,
        Some(m) => {
            let m = m.as_object().ok_or("mood is not an object")?;
            let get_i = |k: &str| -> Result<i32, String> {
                m.get(k)
                    .and_then(Value::as_i64)
                    .map(|v| v as i32)
                    .ok_or_else(|| format!("mood.{k} missing"))
            };
            Some(AmbientMood {
                sound: sound(m.get("sound").ok_or("mood.sound missing")?)?,
                tick_delay: get_i("tick_delay")?,
                block_search_extent: get_i("block_search_extent")?,
                // Vanilla's JSON key is `offset`; the record field is
                // `soundPositionOffset`.
                sound_position_offset: m
                    .get("offset")
                    .and_then(Value::as_f64)
                    .ok_or("mood.offset missing")?,
            })
        }
    };
    let one_addition = |a: &Value| -> Result<AmbientAddition, String> {
        let a = a.as_object().ok_or("an addition is not an object")?;
        Ok(AmbientAddition {
            sound: sound(a.get("sound").ok_or("addition.sound missing")?)?,
            tick_chance: a
                .get("tick_chance")
                .and_then(Value::as_f64)
                .ok_or("addition.tick_chance missing")?,
        })
    };
    // `ExtraCodecs.compactListCodec`: one element writes as the BARE element.
    let additions = match obj.get("additions") {
        None => Vec::new(),
        Some(Value::Array(xs)) => xs.iter().map(one_addition).collect::<Result<_, _>>()?,
        Some(one) => vec![one_addition(one)?],
    };
    Ok(Some(AmbientSounds {
        loop_sound,
        mood,
        additions,
    }))
}
