//! Held-item models (M22) — the item-definition layer, its two geometry
//! sources, and the third-person display transform.
//!
//! 26.x splits an item's appearance in two. `assets/minecraft/items/<item>.json`
//! is the **item-model definition**: a small tree that *chooses* a model from
//! the stack's state. `assets/minecraft/models/item/<name>.json` is the usual
//! parent-chained model with `textures` and `display`.
//!
//! ```text
//! items/diamond_sword.json   {"model": {"type":"minecraft:model",
//!                                       "model":"minecraft:item/diamond_sword"}}
//! models/item/diamond_sword  {"parent":"minecraft:item/handheld",
//!                             "textures":{"layer0":"minecraft:item/diamond_sword"}}
//! models/item/handheld       parent item/generated + display transforms
//! models/item/generated      parent builtin/generated + display transforms
//! ```
//!
//! **Only the plain `minecraft:model` definition is resolved** — 1390 of 26.2's
//! 1537 items. The other five types (`select`, `special`, `composite`,
//! `condition`, `range_dispatch`) branch on stack state this client does not
//! track (charge progress, trim material, banner patterns) or on a bespoke
//! renderer vanilla itself special-cases (shield, chest, conduit). They resolve
//! to [`ItemModel::Unsupported`], which suppresses the held item rather than
//! drawing a guess — the same rule M19 applies to an unknowable swing.
//!
//! The geometry then comes from one of two places, and **750 of the simple
//! definitions point straight at a `block/…` model**, which the block bake has
//! already produced. Only the sprite path needs new geometry, and that is
//! `ItemModelGenerator`'s extrusion (see [`crate::item_geometry`]).

use std::collections::HashMap;

use serde_json::Value;

/// Where a resolved item's geometry comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemGeometry {
    /// `minecraft:block/<name>` — reuse the block model the asset bake already
    /// baked, via that block's default state.
    Block(String),
    /// `builtin/generated` — extrude the sprite layers. Texture references are
    /// resolved and namespace-stripped (`minecraft:item/x` → `item/x`), in
    /// `layer0..layer4` order.
    Sprite(Vec<String>),
}

/// A `display` entry, **already through vanilla's deserializer**.
///
/// `ItemTransform.Deserializer` does two things the raw JSON does not show and
/// that are easy to miss: it multiplies `translation` by `0.0625` (model units
/// → block units, so the item lands in the same 0..1 space the `-0.5` centring
/// in `apply` assumes) and then clamps translation to ±5 and scale to ±4.
/// Storing the raw JSON numbers instead would put every item 16× too far out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayTransform {
    /// Degrees, XYZ.
    pub rotation: [f32; 3],
    /// **Block units** — the raw JSON value × 0.0625, clamped to ±5.
    pub translation: [f32; 3],
    /// Clamped to ±4.
    pub scale: [f32; 3],
}

impl Default for DisplayTransform {
    /// `ItemTransform.NO_TRANSFORM` — identity.
    fn default() -> Self {
        Self {
            rotation: [0.0; 3],
            translation: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}

/// A resolved held item, or the reason it could not be resolved.
#[derive(Clone, Debug, PartialEq)]
pub enum ItemModel {
    Resolved {
        geometry: ItemGeometry,
        /// `display.thirdperson_righthand`, inherited down the parent chain.
        third_person_right: DisplayTransform,
        /// `display.thirdperson_lefthand`. Vanilla falls back to the right-hand
        /// transform mirrored only for *first* person; in third person an
        /// absent left-hand entry means the identity, so absence is recorded
        /// rather than substituted.
        third_person_left: DisplayTransform,
    },
    /// The definition is one of the five state-dependent / bespoke types.
    /// Carries the type name so the suppression is observable.
    Unsupported(String),
}

/// Every item's resolved model, keyed by registry name (`minecraft:…`).
#[derive(Clone, Debug, Default)]
pub struct ItemModels {
    by_name: HashMap<String, ItemModel>,
    /// Count of each unsupported definition type, for the load log and the gate.
    unsupported: HashMap<String, usize>,
}

impl ItemModels {
    pub fn get(&self, name: &str) -> Option<&ItemModel> {
        self.by_name.get(name)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// How many items resolved to real geometry.
    pub fn resolved_count(&self) -> usize {
        self.by_name
            .values()
            .filter(|m| matches!(m, ItemModel::Resolved { .. }))
            .count()
    }

    /// Unsupported definition types and their counts.
    pub fn unsupported_counts(&self) -> &HashMap<String, usize> {
        &self.unsupported
    }

    /// Build directly, for tests and oracles.
    pub fn from_entries(entries: impl IntoIterator<Item = (String, ItemModel)>) -> Self {
        let mut by_name = HashMap::new();
        let mut unsupported: HashMap<String, usize> = HashMap::new();
        for (k, v) in entries {
            if let ItemModel::Unsupported(kind) = &v {
                *unsupported.entry(kind.clone()).or_default() += 1;
            }
            by_name.insert(k, v);
        }
        Self {
            by_name,
            unsupported,
        }
    }
}

/// Strip a `minecraft:` namespace. Anything else is left alone so a
/// non-vanilla namespace cannot be silently treated as vanilla.
fn strip_ns(s: &str) -> &str {
    s.strip_prefix("minecraft:").unwrap_or(s)
}

/// Read `display.<key>` if present.
fn read_transform(display: &Value, key: &str) -> Option<DisplayTransform> {
    let d = display.get(key)?;
    let arr3 = |k: &str, default: [f32; 3]| -> [f32; 3] {
        d.get(k)
            .and_then(|v| v.as_array())
            .filter(|a| a.len() == 3)
            .map(|a| {
                std::array::from_fn(|i| a[i].as_f64().unwrap_or(default[i] as f64) as f32)
            })
            .unwrap_or(default)
    };
    // `ItemTransform.Deserializer`: translation × 0.0625 then clamp ±5;
    // scale clamp ±4; rotation passes through in degrees.
    let t = arr3("translation", [0.0; 3]);
    let s = arr3("scale", [1.0; 3]);
    Some(DisplayTransform {
        rotation: arr3("rotation", [0.0; 3]),
        translation: std::array::from_fn(|i| (t[i] * 0.0625).clamp(-5.0, 5.0)),
        scale: std::array::from_fn(|i| s[i].clamp(-4.0, 4.0)),
    })
}

/// Resolve one `models/item/<name>.json` chain: walk `parent` upward,
/// accumulating texture slots and display transforms (a child wins over its
/// parent, which is vanilla's `BlockModel` inheritance).
///
/// `read` returns the raw JSON for a model path such as `item/handheld`.
/// Returns `None` if the chain is broken or exceeds [`MAX_PARENT_DEPTH`], which
/// is a corrupt/renamed asset rather than a modelled case.
fn resolve_chain(
    start: &str,
    read: &mut dyn FnMut(&str) -> Option<Value>,
) -> Option<(bool, Vec<String>, DisplayTransform, DisplayTransform)> {
    /// Deep enough for every vanilla chain (item → handheld → generated →
    /// builtin) with room to spare; a cycle would otherwise hang the bake.
    const MAX_PARENT_DEPTH: usize = 16;

    let mut textures: HashMap<String, String> = HashMap::new();
    let mut right: Option<DisplayTransform> = None;
    let mut left: Option<DisplayTransform> = None;
    let mut is_generated = false;
    let mut cur = strip_ns(start).to_string();

    for _ in 0..MAX_PARENT_DEPTH {
        if cur.starts_with("builtin/") {
            // `builtin/generated` is the extrusion; any other builtin (there
            // is only `builtin/entity`) is a bespoke renderer.
            is_generated = cur == "builtin/generated";
            if !is_generated {
                return None;
            }
            // A generated model with no third-person transform anywhere in its
            // chain would render at the identity, which is never what vanilla
            // shows — treat it as unresolved rather than place it wrong.
            let right = right?;
            return Some((true, layers(&textures), right, left.unwrap_or_default()));
        }
        let json = read(&cur)?;
        // A child's own entries win, so only insert what is still missing.
        if let Some(t) = json.get("textures").and_then(|t| t.as_object()) {
            for (k, v) in t {
                if let Some(s) = v.as_str() {
                    textures.entry(k.clone()).or_insert_with(|| s.to_string());
                }
            }
        }
        if let Some(d) = json.get("display") {
            if right.is_none() {
                right = read_transform(d, "thirdperson_righthand");
            }
            if left.is_none() {
                left = read_transform(d, "thirdperson_lefthand");
            }
        }
        match json.get("parent").and_then(|p| p.as_str()) {
            Some(p) => cur = strip_ns(p).to_string(),
            // No parent and no builtin: a bespoke model (shield, chest
            // template). Not an extruded sprite.
            None => return None,
        }
    }
    None
}

/// `ItemModelGenerator.LAYERS` order, resolved and namespace-stripped. Stops at
/// the first missing layer, exactly as the generator's `break` does.
fn layers(textures: &HashMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..5 {
        match textures.get(&format!("layer{i}")) {
            Some(t) => out.push(strip_ns(t).to_string()),
            None => break,
        }
    }
    out
}

/// Resolve one item definition. `read_model` fetches `models/<path>.json`.
pub fn resolve_definition(
    def: &Value,
    read_model: &mut dyn FnMut(&str) -> Option<Value>,
) -> ItemModel {
    let Some(model) = def.get("model") else {
        return ItemModel::Unsupported("(no model)".into());
    };
    let kind = model
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("(no type)");
    if kind != "minecraft:model" {
        return ItemModel::Unsupported(kind.to_string());
    }
    let Some(reference) = model.get("model").and_then(|m| m.as_str()) else {
        return ItemModel::Unsupported("minecraft:model (no reference)".into());
    };
    let reference = strip_ns(reference);

    // A `block/…` reference reuses the block bake — no chain walk, no sprite.
    if let Some(block) = reference.strip_prefix("block/") {
        return ItemModel::Resolved {
            geometry: ItemGeometry::Block(block.to_string()),
            // Block items inherit `item/generated`-style transforms from
            // `models/item/<name>.json` when one exists; when it does not (the
            // common case — `models/item/dirt.json` is absent in 26.2) vanilla
            // uses the block model's own `display`, which for
            // `block/block`-parented models is the standard block transform.
            third_person_right: BLOCK_THIRD_PERSON,
            third_person_left: BLOCK_THIRD_PERSON,
        };
    }

    match resolve_chain(reference, read_model) {
        Some((true, layers, right, left)) if !layers.is_empty() => ItemModel::Resolved {
            geometry: ItemGeometry::Sprite(layers),
            third_person_right: right,
            third_person_left: left,
        },
        // A chain that reaches no `builtin/generated`, or one with no layer0,
        // is a bespoke model — suppressed, not guessed.
        _ => ItemModel::Unsupported(format!("model {reference} (not builtin/generated)")),
    }
}

/// `assets/minecraft/models/block/block.json`'s `thirdperson_righthand`:
/// `rotation [0, 45, 0], translation [0, 2.5, 0], scale [0.375, 0.375, 0.375]`.
/// Transcribed rather than read because block items reach their transform
/// through the *block* model chain, which the block bake does not retain.
pub const BLOCK_THIRD_PERSON: DisplayTransform = DisplayTransform {
    rotation: [0.0, 45.0, 0.0],
    // 2.5 model units × 0.0625 — already through the deserializer, like every
    // transform read from JSON.
    translation: [0.0, 2.5 * 0.0625, 0.0],
    scale: [0.375, 0.375, 0.375],
};

/// Resolve every item in `names` from the jar.
///
/// `read` is given a jar-relative path and returns its JSON. Items whose
/// definition file is missing are recorded as unsupported rather than dropped,
/// so the count is always the full registry.
pub fn resolve_all(
    names: impl IntoIterator<Item = String>,
    read: &mut dyn FnMut(&str) -> Option<Value>,
) -> ItemModels {
    let mut by_name = HashMap::new();
    let mut unsupported: HashMap<String, usize> = HashMap::new();
    for full in names {
        let short = strip_ns(&full).to_string();
        let def = read(&format!("assets/minecraft/items/{short}.json"));
        let resolved = match def {
            Some(d) => {
                let mut read_model =
                    |p: &str| read(&format!("assets/minecraft/models/{p}.json"));
                resolve_definition(&d, &mut read_model)
            }
            None => ItemModel::Unsupported("(missing definition)".into()),
        };
        if let ItemModel::Unsupported(kind) = &resolved {
            *unsupported.entry(kind_bucket(kind)).or_default() += 1;
        }
        by_name.insert(full, resolved);
    }
    let models = ItemModels {
        by_name,
        unsupported,
    };
    log::info!(
        "rewo-data: item models — {} of {} resolved ({} unsupported)",
        models.resolved_count(),
        models.len(),
        models.len() - models.resolved_count()
    );
    models
}

/// Collapse the per-model "not builtin/generated" reasons into one bucket so
/// the summary stays readable; the definition *types* stay distinct.
fn kind_bucket(kind: &str) -> String {
    if kind.starts_with("model ") {
        "(bespoke model)".to_string()
    } else {
        kind.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    /// A jar stub: `models/<path>` → JSON.
    fn reader(models: Vec<(&'static str, &'static str)>) -> impl FnMut(&str) -> Option<Value> {
        let map: HashMap<String, Value> = models
            .into_iter()
            .map(|(k, v)| (k.to_string(), json(v)))
            .collect();
        move |p: &str| map.get(p).cloned()
    }

    #[test]
    fn a_block_reference_reuses_the_block_model() {
        let m = resolve_definition(
            &json(r#"{"model":{"type":"minecraft:model","model":"minecraft:block/dirt"}}"#),
            &mut reader(vec![]),
        );
        assert_eq!(
            m,
            ItemModel::Resolved {
                geometry: ItemGeometry::Block("dirt".into()),
                third_person_right: BLOCK_THIRD_PERSON,
                third_person_left: BLOCK_THIRD_PERSON,
            }
        );
    }

    #[test]
    fn a_sword_walks_the_chain_to_builtin_generated() {
        let m = resolve_definition(
            &json(r#"{"model":{"type":"minecraft:model","model":"minecraft:item/diamond_sword"}}"#),
            &mut reader(vec![
                (
                    "item/diamond_sword",
                    r#"{"parent":"minecraft:item/handheld",
                        "textures":{"layer0":"minecraft:item/diamond_sword"}}"#,
                ),
                (
                    "item/handheld",
                    r#"{"parent":"item/generated","display":{
                          "thirdperson_righthand":{"rotation":[0,-90,55],
                            "translation":[0,4.0,0.5],"scale":[0.85,0.85,0.85]},
                          "thirdperson_lefthand":{"rotation":[0,90,-55],
                            "translation":[0,4.0,0.5],"scale":[0.85,0.85,0.85]}}}"#,
                ),
                (
                    "item/generated",
                    r#"{"parent":"builtin/generated","display":{
                          "thirdperson_righthand":{"rotation":[0,0,0],
                            "translation":[0,3,1],"scale":[0.55,0.55,0.55]}}}"#,
                ),
            ]),
        );
        match m {
            ItemModel::Resolved {
                geometry,
                third_person_right,
                third_person_left,
            } => {
                assert_eq!(geometry, ItemGeometry::Sprite(vec!["item/diamond_sword".into()]));
                // handheld wins over generated — the child's display is nearer.
                assert_eq!(third_person_right.rotation, [0.0, -90.0, 55.0]);
                assert_eq!(third_person_right.scale, [0.85, 0.85, 0.85]);
                // translation [0, 4.0, 0.5] × 0.0625 = [0, 0.25, 0.03125].
                assert_eq!(third_person_right.translation, [0.0, 0.25, 0.03125]);
                assert_eq!(third_person_left.rotation, [0.0, 90.0, -55.0]);
            }
            other => panic!("expected a sprite item, got {other:?}"),
        }
    }

    #[test]
    fn every_state_dependent_definition_type_is_suppressed() {
        for kind in [
            "minecraft:select",
            "minecraft:special",
            "minecraft:composite",
            "minecraft:condition",
            "minecraft:range_dispatch",
        ] {
            let m = resolve_definition(
                &json(&format!(r#"{{"model":{{"type":"{kind}"}}}}"#)),
                &mut reader(vec![]),
            );
            assert_eq!(
                m,
                ItemModel::Unsupported(kind.into()),
                "{kind} must suppress, not guess"
            );
        }
    }

    #[test]
    fn a_chain_that_never_reaches_builtin_generated_is_suppressed() {
        // `models/item/shield.json` has no parent at all.
        let m = resolve_definition(
            &json(r#"{"model":{"type":"minecraft:model","model":"minecraft:item/shield"}}"#),
            &mut reader(vec![("item/shield", r#"{"textures":{"layer0":"item/shield"}}"#)]),
        );
        assert!(matches!(m, ItemModel::Unsupported(_)), "got {m:?}");
    }

    #[test]
    fn a_parent_cycle_terminates_instead_of_hanging() {
        let m = resolve_definition(
            &json(r#"{"model":{"type":"minecraft:model","model":"minecraft:item/a"}}"#),
            &mut reader(vec![
                ("item/a", r#"{"parent":"item/b"}"#),
                ("item/b", r#"{"parent":"item/a"}"#),
            ]),
        );
        assert!(matches!(m, ItemModel::Unsupported(_)), "got {m:?}");
    }

    #[test]
    fn layers_stop_at_the_first_gap_like_the_generator_break() {
        let mut t = HashMap::new();
        t.insert("layer0".to_string(), "minecraft:item/a".to_string());
        t.insert("layer2".to_string(), "minecraft:item/c".to_string());
        // layer1 missing → the generator `break`s, so layer2 is never reached.
        assert_eq!(layers(&t), vec!["item/a".to_string()]);
    }
}
