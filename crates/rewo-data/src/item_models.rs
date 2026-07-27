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

impl DisplayTransform {
    /// No rotation, no translation, unit scale — what an absent `display`
    /// entry means, and the only sensible value for a model that never renders
    /// in that context at all.
    pub const IDENTITY: Self = Self {
        rotation: [0.0; 3],
        translation: [0.0; 3],
        scale: [1.0; 3],
    };
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
        /// `display.ground` — the context `ItemEntityRenderer` renders a
        /// dropped stack through (`updateForNonLiving(..., GROUND, ...)`).
        ///
        /// Unlike the hand transforms this one is genuinely optional: an item
        /// whose chain never declares `ground` renders at the identity, which
        /// for a ground item is a legible (if unscaled) result rather than the
        /// wrong place. `item/generated` and `item/handheld` both declare it,
        /// so every extruded sprite has one in practice.
        ground: DisplayTransform,
        /// `display.gui` — how the item sits in a hotbar or inventory slot
        /// (M34).
        ///
        /// Absent for every extruded sprite, and the identity is right for
        /// them: a flat quad facing the viewer that fills the slot. Blocks
        /// inherit `block/block.json`'s `rotation [30, 225, 0], scale 0.625`,
        /// which is what makes a block in the hotbar read as a little cube seen
        /// from its top-front-right corner.
        gui: DisplayTransform,
        /// `display.firstperson_righthand` — the context the held item is
        /// drawn through in first person (M38).
        first_person_right: DisplayTransform,
        /// `display.firstperson_lefthand`, **with vanilla's fallback already
        /// applied**: `ItemTransforms`' builder replaces an absent left-hand
        /// entry with the right-hand one, and *only* in first person — the
        /// third-person left has no such fallback, which is why the field
        /// above it records absence instead.
        ///
        /// The left/right *mirror* is not baked in. `ItemTransform.apply`
        /// negates `translation.x`, `rotation.y` and `rotation.z` at draw time
        /// whenever the context is a left hand, and it does so to whichever
        /// transform was selected — including one that arrived through the
        /// fallback. Baking the mirror here would double it for the items that
        /// declare their own left entry (`handheld` authors it pre-mirrored,
        /// so the two cancel to the same pose).
        first_person_left: DisplayTransform,
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
type ChainResult = (
    bool,
    Vec<String>,
    // In order: `thirdperson_righthand`, `thirdperson_lefthand`, `ground`,
    // `gui` (M34), then `firstperson_righthand` and `firstperson_lefthand`
    // (M38) — the last already through vanilla's absent-left fallback.
    DisplayTransform,
    DisplayTransform,
    DisplayTransform,
    DisplayTransform,
    DisplayTransform,
    DisplayTransform,
);

fn resolve_chain(
    start: &str,
    read: &mut dyn FnMut(&str) -> Option<Value>,
) -> Option<ChainResult> {
    /// Deep enough for every vanilla chain (item → handheld → generated →
    /// builtin) with room to spare; a cycle would otherwise hang the bake.
    const MAX_PARENT_DEPTH: usize = 16;

    let mut textures: HashMap<String, String> = HashMap::new();
    let mut right: Option<DisplayTransform> = None;
    let mut left: Option<DisplayTransform> = None;
    let mut ground: Option<DisplayTransform> = None;
    let mut gui: Option<DisplayTransform> = None;
    let mut first_right: Option<DisplayTransform> = None;
    let mut first_left: Option<DisplayTransform> = None;
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
            return Some((
                true,
                layers(&textures),
                right,
                left.unwrap_or_default(),
                ground.unwrap_or_default(),
                // A sprite declares no `display.gui` anywhere in its chain, and
                // the identity is exactly right for it: a flat quad facing the
                // viewer, filling the slot. This is absence meaning something,
                // not a missing default.
                gui.unwrap_or_default(),
                first_right.unwrap_or_default(),
                // `ItemTransforms`' builder: an absent first-person left entry
                // becomes the right one. `item/generated` really does declare
                // only the right hand, so this fallback fires for every
                // extruded sprite that is not `handheld`.
                first_left.or(first_right).unwrap_or_default(),
            ));
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
            if ground.is_none() {
                ground = read_transform(d, "ground");
            }
            if gui.is_none() {
                gui = read_transform(d, "gui");
            }
            if first_right.is_none() {
                first_right = read_transform(d, "firstperson_righthand");
            }
            if first_left.is_none() {
                first_left = read_transform(d, "firstperson_lefthand");
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
            ground: BLOCK_GROUND,
            gui: BLOCK_GUI,
            first_person_right: BLOCK_FIRST_PERSON_RIGHT,
            first_person_left: BLOCK_FIRST_PERSON_LEFT,
        };
    }

    match resolve_chain(reference, read_model) {
        Some((true, layers, right, left, ground, gui, first_right, first_left))
            if !layers.is_empty() =>
        {
            ItemModel::Resolved {
                geometry: ItemGeometry::Sprite(layers),
                third_person_right: right,
                third_person_left: left,
                ground,
                gui,
                first_person_right: first_right,
                first_person_left: first_left,
            }
        }
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

/// `assets/minecraft/models/block/block.json`'s `ground`:
/// `rotation [0,0,0], translation [0, 3, 0], scale [0.25, 0.25, 0.25]`.
/// Transcribed for the same reason as [`BLOCK_THIRD_PERSON`] — a block item
/// reaches its transforms through the *block* model chain, which the block
/// bake does not retain. Note the ground scale is 0.25, not the hand's 0.375:
/// a dropped block is visibly smaller than a held one.
/// `block/block.json`'s `firstperson_righthand`: `rotation [0, 45, 0],
/// scale 0.4`. A held block is markedly smaller in first person than in third
/// (0.4 against 0.375 — very close, but the rotations differ: the third-person
/// pose tilts the cube, the first-person one only spins it).
pub const BLOCK_FIRST_PERSON_RIGHT: DisplayTransform = DisplayTransform {
    rotation: [0.0, 45.0, 0.0],
    translation: [0.0; 3],
    scale: [0.4, 0.4, 0.4],
};

/// `block/block.json`'s `firstperson_lefthand`: `rotation [0, 225, 0],
/// scale 0.4`.
///
/// Declared explicitly rather than inherited, and note it is **not** the
/// mirror of the right-hand entry — 225 is 45 + 180, a further half-turn, on
/// top of which `ItemTransform.apply` still negates `rotation.y` at draw time.
/// The block models author the two independently.
pub const BLOCK_FIRST_PERSON_LEFT: DisplayTransform = DisplayTransform {
    rotation: [0.0, 225.0, 0.0],
    translation: [0.0; 3],
    scale: [0.4, 0.4, 0.4],
};

/// `block/block.json`'s `gui`: `rotation [30, 225, 0], scale 0.625`, no
/// translation. This is what makes a block in a hotbar slot read as a little
/// cube seen from its top-front-right corner rather than as a flat face.
pub const BLOCK_GUI: DisplayTransform = DisplayTransform {
    rotation: [30.0, 225.0, 0.0],
    translation: [0.0; 3],
    scale: [0.625, 0.625, 0.625],
};

pub const BLOCK_GROUND: DisplayTransform = DisplayTransform {
    rotation: [0.0; 3],
    translation: [0.0, 3.0 * 0.0625, 0.0],
    scale: [0.25, 0.25, 0.25],
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
                ground: BLOCK_GROUND,
                // A block item inherits block/block.json rather than the
                // identity — this is what tilts a block in the hotbar into the
                // familiar corner-on cube.
                gui: BLOCK_GUI,
                first_person_right: BLOCK_FIRST_PERSON_RIGHT,
                first_person_left: BLOCK_FIRST_PERSON_LEFT,
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
                ground: _,
                gui,
                ..
            } => {
                assert_eq!(geometry, ItemGeometry::Sprite(vec!["item/diamond_sword".into()]));
                // A sprite declares no `display.gui` anywhere in its chain, and
                // the identity is what makes it fill the slot as a flat quad
                // facing the viewer. Substituting the block transform here
                // would tilt every sword in the hotbar.
                assert_eq!(gui, DisplayTransform::IDENTITY);
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


    /// The two `gui` cases are genuinely different, and which one an item gets
    /// is decided by whether its geometry came from a block model. A single
    /// shared default would tilt every sword or flatten every block.
    #[test]
    fn a_block_tilts_in_the_slot_and_a_sprite_does_not() {
        assert_eq!(BLOCK_GUI.rotation, [30.0, 225.0, 0.0]);
        assert_eq!(BLOCK_GUI.scale, [0.625; 3]);
        assert_eq!(BLOCK_GUI.translation, [0.0; 3]);
        assert_eq!(DisplayTransform::IDENTITY.rotation, [0.0; 3]);
        assert_eq!(DisplayTransform::IDENTITY.scale, [1.0; 3]);
        assert_ne!(BLOCK_GUI.rotation, DisplayTransform::IDENTITY.rotation);
    }

    /// **An absent `firstperson_lefthand` falls back to the right-hand entry,
    /// and only in first person.** `item/generated` declares just the right
    /// hand; `ItemTransforms`' builder does
    /// `if (firstPersonLeftHand == NO_TRANSFORM) firstPersonLeftHand = firstPersonRightHand`
    /// — and there is no equivalent line for the third-person pair, which is
    /// why the third-person left stays at the identity here.
    #[test]
    fn an_absent_first_person_left_falls_back_to_the_right() {
        let m = resolve_definition(
            &json(r#"{"model":{"type":"minecraft:model","model":"item/generated"}}"#),
            &mut reader(vec![(
                "item/generated",
                r#"{"parent":"builtin/generated",
                    "textures":{"layer0":"item/stick"},
                    "display":{
                      "firstperson_righthand":{"rotation":[0,-90,25],"translation":[1.13,3.2,1.13],"scale":[0.68,0.68,0.68]},
                      "thirdperson_righthand":{"rotation":[0,-90,55]}}}"#,
            )]),
        );
        match m {
            ItemModel::Resolved {
                first_person_right,
                first_person_left,
                third_person_left,
                ..
            } => {
                assert_eq!(first_person_left, first_person_right, "the fallback fired");
                assert_eq!(first_person_right.rotation, [0.0, -90.0, 25.0]);
                assert_eq!(first_person_right.scale, [0.68, 0.68, 0.68]);
                // No such fallback in third person: absence stays absence.
                assert_eq!(third_person_left, DisplayTransform::IDENTITY);
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    /// A declared left-hand entry is kept **as authored**, not mirrored here.
    /// `ItemTransform.apply` negates `translation.x`, `rotation.y` and
    /// `rotation.z` at draw time for either hand's transform; baking the
    /// mirror as well would double it. `handheld` authors its left entry
    /// pre-mirrored, so the two cancel — which is only true if this stage
    /// leaves it alone.
    #[test]
    fn a_declared_first_person_left_is_not_pre_mirrored() {
        let m = resolve_definition(
            &json(r#"{"model":{"type":"minecraft:model","model":"item/handheld"}}"#),
            &mut reader(vec![(
                "item/handheld",
                r#"{"parent":"builtin/generated",
                    "textures":{"layer0":"item/stick"},
                    "display":{
                      "firstperson_righthand":{"rotation":[0,-90,25],"translation":[1.13,3.2,1.13],"scale":[0.68,0.68,0.68]},
                      "firstperson_lefthand":{"rotation":[0,90,-25],"translation":[1.13,3.2,1.13],"scale":[0.68,0.68,0.68]},
                      "thirdperson_righthand":{"rotation":[0,-90,55]}}}"#,
            )]),
        );
        match m {
            ItemModel::Resolved {
                first_person_left, ..
            } => {
                assert_eq!(
                    first_person_left.rotation,
                    [0.0, 90.0, -25.0],
                    "kept as authored; the draw-time mirror turns it into the right-hand pose"
                );
                // The deserializer's 1/16 scaling still applies to translation.
                assert!((first_person_left.translation[0] - 1.13 * 0.0625).abs() < 1e-6);
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    /// A model that declares its own `display.gui` beats the inherited one —
    /// 17 vanilla item models do (the conduit is one, at rotation [30, 45, 0]).
    #[test]
    fn a_models_own_gui_entry_wins_over_the_chain() {
        let m = resolve_definition(
            &json(r#"{"model":{"type":"minecraft:model","model":"item/thing"}}"#),
            &mut reader(vec![
                (
                    "item/thing",
                    r#"{"parent":"item/generated",
                        "display":{"gui":{"rotation":[30,45,0],"scale":[1,1,1]}}}"#,
                ),
                (
                    "item/generated",
                    r#"{"parent":"builtin/generated",
                        "textures":{"layer0":"item/thing"},
                        "display":{"thirdperson_righthand":{"rotation":[0,0,0]},
                                   "gui":{"rotation":[0,180,0]}}}"#,
                ),
            ]),
        );
        match m {
            ItemModel::Resolved { gui, .. } => {
                assert_eq!(gui.rotation, [30.0, 45.0, 0.0], "the child's entry wins");
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }
}
