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
#[derive(Clone, Debug, PartialEq)]
pub enum ItemGeometry {
    /// `minecraft:block/<name>` — reuse the block model the asset bake already
    /// baked, via that block's default state.
    Block(String),
    /// `builtin/generated` — extrude the sprite layers. Texture references are
    /// resolved and namespace-stripped (`minecraft:item/x` → `item/x`), in
    /// `layer0..layer4` order.
    Sprite(Vec<String>),
    /// M242 — a `minecraft:special` model drawn by a block-entity renderer
    /// (a chest, a shulker box, a head, a copper golem statue): the baked
    /// block-entity model of that name (`rewo:be/...`) and the affine (block
    /// units) that takes its model space to the item's: the special node's
    /// `transformation`, and for the statue its `setupAnim` root turn.
    Entity(String, crate::be_transform::Affine),
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
// Built once per item at bake time and held in a table; boxing `Resolved` would
// only add an indirection to every read.
#[allow(clippy::large_enum_variant)]
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
        /// `display.fixed` — the context `ItemFrameRenderer` draws a framed
        /// item through (`updateForNonLiving(..., FIXED, ...)`, M194).
        /// `item/generated` declares `rotation [0, 180, 0]`; block items take
        /// `block/block.json`'s `scale 0.5`.
        fixed: DisplayTransform,
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
            .map(|a| std::array::from_fn(|i| a[i].as_f64().unwrap_or(default[i] as f64) as f32))
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
    // (M38) — the last already through vanilla's absent-left fallback —
    // and `fixed` (M194).
    DisplayTransform,
    DisplayTransform,
    DisplayTransform,
    DisplayTransform,
    DisplayTransform,
    DisplayTransform,
    DisplayTransform,
);

fn resolve_chain(start: &str, read: &mut dyn FnMut(&str) -> Option<Value>) -> Option<ChainResult> {
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
    let mut fixed: Option<DisplayTransform> = None;
    let mut cur = strip_ns(start).to_string();

    for _ in 0..MAX_PARENT_DEPTH {
        if cur.starts_with("builtin/") {
            // `builtin/generated` is the extrusion; any other builtin (there
            // is only `builtin/entity`) is a bespoke renderer.
            if cur != "builtin/generated" {
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
                fixed.unwrap_or_default(),
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
            if fixed.is_none() {
                fixed = read_transform(d, "fixed");
            }
        }
        // No parent and no builtin: a bespoke model (shield, chest
        // template). Not an extruded sprite.
        let p = json.get("parent").and_then(|p| p.as_str())?;
        cur = strip_ns(p).to_string();
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

// ---------------------------------------------------------------------------
// Reducing a state-dependent definition (M40).
//
// M22 suppressed every definition type but `minecraft:model` — 147 of 1537
// items — on the grounds that they "branch on stack state this client does not
// track". That was true of the *tree*, and false of the *outcome*: surveying
// the real jar, **all 71 `select` definitions carry a `fallback`**, and every
// `condition` carries an `on_false`. For a stack with no components those are
// not defaults to fall back on, they are the answer. An untrimmed helmet has
// no `TRIM` component, `SelectItemModel` finds nothing to match, and vanilla
// renders the fallback — which is the plain helmet sprite.
//
// So the rule is not "suppress the type", it is **suppress the property we
// cannot evaluate**. Rewo can answer `trim_material` (absent), `charge_type`
// (none), `block_state` (absent), `using_item` (it has a use clock), and
// `display_context` (it knows which pass is drawing). It cannot answer
// `local_time` or `context_dimension`, and those two stay suppressed.
// ---------------------------------------------------------------------------

/// `ItemDisplayContext` — which pass is asking for the model.
///
/// It has to be an input rather than a constant because a spear's definition
/// selects *different geometry* per context: a flat sprite in a slot, a 3D
/// `_in_hand` model in the hand. Everything else in 26.2 that consults this
/// property splits the same way, `gui` against the rest, but that is a fact
/// about the data rather than a rule, so the match below is by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayContext {
    Gui,
    FirstPersonRightHand,
    FirstPersonLeftHand,
    ThirdPersonRightHand,
    ThirdPersonLeftHand,
    Ground,
    Fixed,
    Head,
    OnShelf,
}

impl DisplayContext {
    /// The name `ItemDisplayContext` serialises to, which is what a `when`
    /// entry carries.
    pub fn wire_name(self) -> &'static str {
        match self {
            DisplayContext::Gui => "gui",
            DisplayContext::FirstPersonRightHand => "firstperson_righthand",
            DisplayContext::FirstPersonLeftHand => "firstperson_lefthand",
            DisplayContext::ThirdPersonRightHand => "thirdperson_righthand",
            DisplayContext::ThirdPersonLeftHand => "thirdperson_lefthand",
            DisplayContext::Ground => "ground",
            DisplayContext::Fixed => "fixed",
            DisplayContext::Head => "head",
            DisplayContext::OnShelf => "on_shelf",
        }
    }
}

/// The stack state a definition may branch on, as far as Rewo can see it.
///
/// Every field but `display` describes a **component-free, unused** stack,
/// because that is the only kind Rewo can describe: `rewo_net::item_stack`
/// reports *whether* a patch was present, never what was in it. A stack that
/// does carry components is still drawn through this reduction, and that is
/// the one approximation here — a trimmed helmet renders untrimmed rather than
/// not at all. It is the same direction of error [`ItemModel::Unsupported`]
/// takes (show less, never show wrong geometry for the wrong item), traded for
/// a visible icon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionContext<'a> {
    pub display: DisplayContext,
    /// `minecraft:trim_material` — the trim material's **registry id**
    /// (`minecraft:quartz`), not its `asset_name` suffix, because that is what
    /// the item definition's `when` values are (M49).
    ///
    /// `None` means "this stack has no trim", which is not the same as "the
    /// property is unevaluable": a `select` over it then takes its fallback,
    /// which is vanilla's own answer for an untrimmed stack.
    pub trim_material: Option<&'a str>,
    /// `minecraft:using_item` — true while the player holds right-click on
    /// this stack. M38's use clock can answer it for the local player's hand;
    /// a slot icon is never using anything.
    pub using_item: bool,
    /// M234 — `UseDuration.useDuration(stack, owner)`, `getUseDuration -
    /// getUseItemRemainingTicks`: the ticks this stack has been used, 0
    /// unless it is the owner's use item (`entity.getUseItem() == itemStack`).
    pub use_ticks: i32,
    /// `getUseItemRemainingTicks()`, what `use_duration` reads with
    /// `"remaining": true` (0 likewise unless this is the use item).
    pub use_remaining: i32,
    /// M234 — `CrossbowPull.get`: 0 when charged or not this stack's use,
    /// else `useDuration / getChargeDuration`.
    pub crossbow_pull: f32,
    /// M234 — `Charge.get`: `"arrow"` or `"rocket"` for a charged crossbow,
    /// `None` for `ChargeType.NONE`.
    pub charge_type: Option<&'a str>,
}

impl<'a> SelectionContext<'a> {
    pub fn gui() -> Self {
        Self {
            display: DisplayContext::Gui,
            using_item: false,
            trim_material: None,
            use_ticks: 0,
            use_remaining: 0,
            crossbow_pull: 0.0,
            charge_type: None,
        }
    }
    pub fn hand() -> Self {
        Self {
            display: DisplayContext::FirstPersonRightHand,
            ..Self::gui()
        }
    }

    /// M234 — the same context for the owner's use item: `using_item`, the
    /// ticks used and remaining, and the crossbow's pull.
    pub fn with_use(self, use_ticks: i32, use_remaining: i32, crossbow_pull: f32) -> Self {
        Self {
            using_item: true,
            use_ticks,
            use_remaining,
            crossbow_pull,
            ..self
        }
    }

    /// M234 — a charged crossbow's `charge_type`.
    pub fn with_charge(self, charge_type: Option<&'a str>) -> Self {
        Self {
            charge_type,
            ..self
        }
    }

    /// The same context, for a stack wearing this trim material.
    pub fn with_trim(self, material: Option<&'a str>) -> Self {
        Self {
            trim_material: material,
            ..self
        }
    }
}

/// Properties whose value on a component-free stack is *known to be absent*,
/// so a `select` over them takes its fallback.
///
/// Listed rather than defaulted: a property missing from here is one nobody
/// has checked, and it suppresses the item. That is the same fail-closed rule
/// the registry lookups use — a version bump that adds a property makes the
/// item disappear, which is visible, instead of silently picking a branch.
const ABSENT_ON_A_PLAIN_STACK: &[&str] = &[
    "minecraft:trim_material",
    "minecraft:block_state",
    "minecraft:custom_model_data",
    "minecraft:main_hand",
];

/// Conditions that are false for a component-free, unused stack.
const FALSE_ON_A_PLAIN_STACK: &[&str] = &[
    "minecraft:broken",
    "minecraft:damaged",
    "minecraft:has_component",
    "minecraft:custom_model_data",
    "minecraft:fishing_rod/cast",
    "minecraft:bundle/has_selected_item",
    "minecraft:extended_view",
    "minecraft:selected",
    "minecraft:carried",
    "minecraft:keybind_down",
    "minecraft:view_entity",
];

/// Numeric properties that read **zero** on a component-free, unused stack, so
/// a `range_dispatch` over them takes the entry at or below zero.
/// `use_duration` and `crossbow/pull` read the context instead (M234).
const ZERO_ON_A_PLAIN_STACK: &[&str] = &[
    "minecraft:use_cycle",
    "minecraft:damage",
    "minecraft:cooldown",
];

/// Walk a definition tree down to the `minecraft:model` node this stack and
/// context select, or `None` where a property cannot be evaluated.
///
/// The recursion is what makes the coverage worth having: a bow is a
/// `condition` whose `on_true` is a `range_dispatch`, and reducing only the
/// top level would leave it suppressed. Depth is bounded because a definition
/// is a finite tree, but a hostile pack could nest arbitrarily, so the depth
/// is capped rather than trusted.
pub fn reduce_definition<'v>(
    node: &'v Value,
    ctx: SelectionContext<'_>,
) -> Result<&'v Value, String> {
    let mut node = node;
    for _ in 0..=16 {
        match chosen_child(node, ctx)? {
            None => return Ok(node),
            Some(next) => node = next,
        }
    }
    Err("(definition nested past 16 levels)".into())
}

fn blocked(node: &Value, property: Option<&str>) -> String {
    let kind = node
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("(no type)");
    match property {
        Some(p) => format!("{kind} ({p})"),
        None => kind.to_string(),
    }
}

/// The `cases` entry whose `when` names `want`.
fn case_for<'v>(node: &'v Value, want: &str) -> Option<&'v Value> {
    node.get("cases")
        .and_then(|c| c.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .find(|c| match c.get("when") {
            // `when` is either one name or a list of them, and the list form
            // is not rare — a spear's slot case names four contexts at once.
            Some(Value::String(s)) => s == want,
            Some(Value::Array(a)) => a.iter().any(|v| v.as_str() == Some(want)),
            _ => false,
        })
}

/// One step of [`reduce_definition`]: the child `node` selects in `ctx`, or
/// `None` when `node` is the `minecraft:model` leaf itself.
fn chosen_child<'v>(
    node: &'v Value,
    ctx: SelectionContext<'_>,
) -> Result<Option<&'v Value>, String> {
    let Some(kind) = node.get("type").and_then(|t| t.as_str()) else {
        return Err(blocked(node, None));
    };
    match kind {
        "minecraft:model" => Ok(None),
        "minecraft:select" => {
            let Some(property) = node.get("property").and_then(|p| p.as_str()) else {
                return Err(blocked(node, None));
            };
            let chosen = if property == "minecraft:display_context" {
                case_for(node, ctx.display.wire_name())
            } else if property == "minecraft:trim_material" {
                // M49. A trimmed stack picks the case whose `when` is its
                // material's registry id; an untrimmed one matches nothing and
                // falls back, which is what vanilla does.
                ctx.trim_material.and_then(|want| case_for(node, want))
            } else if property == "minecraft:local_time" {
                // M242 — the chest's Christmas texture (`pattern` "MM-dd",
                // `when` 12-24..26). Rewo keeps no calendar here, so the
                // select always takes its fallback: a plain chest all year.
                None
            } else if property == "minecraft:charge_type" {
                // M234 — `Charge.get`; `NONE` matches no vanilla case.
                ctx.charge_type.and_then(|want| case_for(node, want))
            } else if ABSENT_ON_A_PLAIN_STACK.contains(&property) {
                // The property reads absent, so no case matches. This is
                // vanilla's own answer, not a substitution.
                None
            } else {
                return Err(blocked(node, Some(property)));
            };
            let next = match chosen {
                Some(case) => case.get("model"),
                None => node.get("fallback"),
            };
            next.map(Some).ok_or_else(|| blocked(node, Some(property)))
        }
        "minecraft:condition" => {
            let Some(property) = node.get("property").and_then(|p| p.as_str()) else {
                return Err(blocked(node, None));
            };
            let value = if property == "minecraft:using_item" {
                ctx.using_item
            } else if FALSE_ON_A_PLAIN_STACK.contains(&property) {
                false
            } else {
                return Err(blocked(node, Some(property)));
            };
            node.get(if value { "on_true" } else { "on_false" })
                .map(Some)
                .ok_or_else(|| blocked(node, Some(property)))
        }
        "minecraft:range_dispatch" => {
            let Some(property) = node.get("property").and_then(|p| p.as_str()) else {
                return Err(blocked(node, None));
            };
            let raw = match property {
                "minecraft:use_duration" => {
                    if node.get("remaining").and_then(|r| r.as_bool()) == Some(true) {
                        ctx.use_remaining as f32
                    } else {
                        ctx.use_ticks as f32
                    }
                }
                "minecraft:crossbow/pull" => ctx.crossbow_pull,
                p if ZERO_ON_A_PLAIN_STACK.contains(&p) => 0.0,
                _ => return Err(blocked(node, Some(property))),
            };
            // `RangeSelectItemModel.get`: `property.get(..) * scale` (`scale`
            // defaults to 1), in float.
            let scale = node.get("scale").and_then(|s| s.as_f64()).unwrap_or(1.0) as f32;
            let value = raw * scale;
            // `RangeSelectItemModel` picks the **last** entry whose threshold
            // the value has reached, and its `fallback` when it has reached
            // none. At zero that is an entry with a threshold of zero or
            // below, which `clock` has and `brush` does not.
            let mut best: Option<(f32, &Value)> = None;
            let entries = node
                .get("entries")
                .and_then(|e| e.as_array())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            for e in entries {
                let (Some(t), Some(m)) =
                    (e.get("threshold").and_then(|t| t.as_f64()), e.get("model"))
                else {
                    return Err(blocked(node, Some(property)));
                };
                // Thresholds are floats in vanilla (`Codec.FLOAT`).
                let t = t as f32;
                if t <= value && best.is_none_or(|(bt, _)| t >= bt) {
                    best = Some((t, m));
                }
            }
            best.map(|(_, m)| m)
                .or_else(|| node.get("fallback"))
                .map(Some)
                .ok_or_else(|| blocked(node, Some(property)))
        }
        // M242 — a `special` is a leaf: `resolve_definition` maps the ones a
        // block-entity bake covers and suppresses the rest.
        "minecraft:special" => Ok(None),
        // `composite` layers several models into one draw: real work rather
        // than a branch to evaluate, and it stays suppressed.
        _ => Err(blocked(node, None)),
    }
}

/// Whether `node` branches on the owner's use or a crossbow's charge (M234):
/// a `using_item` condition, a `use_duration` / `crossbow/pull` range
/// dispatch, or a `charge_type` select.
fn branches_on_use(node: &Value) -> bool {
    let property = node.get("property").and_then(|p| p.as_str());
    match node.get("type").and_then(|t| t.as_str()) {
        Some("minecraft:condition") => property == Some("minecraft:using_item"),
        Some("minecraft:range_dispatch") => matches!(
            property,
            Some("minecraft:use_duration" | "minecraft:crossbow/pull")
        ),
        Some("minecraft:select") => property == Some("minecraft:charge_type"),
        _ => false,
    }
}

/// M234 — every `minecraft:model` leaf a definition can reach in `ctx`'s
/// display context as the owner's use and a crossbow's charge vary: all the
/// branches of the nodes [`branches_on_use`] names, and the branch `ctx`
/// selects everywhere else. Empty for a definition that never branches on
/// use (there is nothing to bake beyond the plain model); a leaf the walk
/// cannot reach (behind a `special`) is skipped.
pub fn use_variant_leaves<'v>(root: &'v Value, ctx: SelectionContext<'_>) -> Vec<&'v Value> {
    fn go<'v>(
        node: &'v Value,
        ctx: SelectionContext<'_>,
        depth: u32,
        dynamic: &mut bool,
        out: &mut Vec<&'v Value>,
    ) {
        if depth > 16 {
            return;
        }
        if branches_on_use(node) {
            *dynamic = true;
            let mut children: Vec<&Value> = Vec::new();
            for key in ["on_true", "on_false", "fallback"] {
                children.extend(node.get(key));
            }
            for list in ["cases", "entries"] {
                let entries = node
                    .get(list)
                    .and_then(|c| c.as_array())
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                children.extend(entries.iter().filter_map(|e| e.get("model")));
            }
            for c in children {
                go(c, ctx, depth + 1, dynamic, out);
            }
            return;
        }
        match chosen_child(node, ctx) {
            // Only a `minecraft:model` leaf is something the bake can draw
            // (M242 made a `special` a leaf too).
            Ok(None) if node.get("type").and_then(|t| t.as_str()) == Some("minecraft:model") => {
                if !out.iter().any(|o| std::ptr::eq(*o, node)) {
                    out.push(node);
                }
            }
            Ok(Some(next)) => go(next, ctx, depth + 1, dynamic, out),
            Ok(None) | Err(_) => {}
        }
    }
    let mut dynamic = false;
    let mut out = Vec::new();
    go(root, ctx, 0, &mut dynamic, &mut out);
    if dynamic {
        out
    } else {
        Vec::new()
    }
}

/// Resolve one item definition. `read_model` fetches `models/<path>.json`.
pub fn resolve_definition(
    def: &Value,
    read_model: &mut dyn FnMut(&str) -> Option<Value>,
    ctx: SelectionContext<'_>,
) -> ItemModel {
    let Some(root) = def.get("model") else {
        return ItemModel::Unsupported("(no model)".into());
    };
    // Walk the branches this stack and context select. A plain
    // `minecraft:model` reduces to itself, so this costs nothing for the 1390
    // items that were already resolving.
    let model = match reduce_definition(root, ctx) {
        Ok(m) => m,
        // The reason names the node the walk **stopped at**, not the root: a
        // `condition` whose chosen branch is a `special` is blocked by the
        // special, and bucketing it under `condition` would read as "the
        // reduction cannot do conditions", which is the opposite of true.
        Err(reason) => return ItemModel::Unsupported(reason),
    };
    if model.get("type").and_then(|t| t.as_str()) == Some("minecraft:special") {
        return special_item(model, read_model);
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
            fixed: BLOCK_FIXED,
        };
    }

    match resolve_chain(reference, read_model) {
        Some((true, layers, right, left, ground, gui, first_right, first_left, fixed))
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
                fixed,
            }
        }
        // A chain that reaches no `builtin/generated`, or one with no layer0,
        // is a bespoke model — suppressed, not guessed.
        _ => ItemModel::Unsupported(format!("model {reference} (not builtin/generated)")),
    }
}

/// M242 — the block-entity model a `minecraft:special` item model names, for
/// the special renderers a block-entity bake already covers: the chest
/// family, shulker boxes, the six mob heads and the player head, and the
/// copper golem statues. `None` for the rest (the shield, the trident,
/// banners, the decorated pot, the conduit), which stay suppressed.
pub fn special_block_entity(spec: &Value) -> Option<String> {
    let s = |k: &str| spec.get(k).and_then(|v| v.as_str());
    Some(match s("type")? {
        "minecraft:chest" => match s("texture")? {
            "minecraft:normal" => "rewo:be/chest".into(),
            "minecraft:trapped" => "rewo:be/trapped_chest".into(),
            "minecraft:ender" => "rewo:be/ender_chest".into(),
            "minecraft:copper" => "rewo:be/copper_chest".into(),
            "minecraft:copper_exposed" => "rewo:be/exposed_copper_chest".into(),
            "minecraft:copper_weathered" => "rewo:be/weathered_copper_chest".into(),
            "minecraft:copper_oxidized" => "rewo:be/oxidized_copper_chest".into(),
            _ => return None,
        },
        "minecraft:shulker_box" => match s("texture")?.strip_prefix("minecraft:shulker")? {
            "" => "rewo:be/shulker_box".into(),
            colour => format!("rewo:be/{}_shulker_box", colour.strip_prefix('_')?),
        },
        "minecraft:head" => match s("kind")? {
            "skeleton" => "rewo:be/skeleton_skull".into(),
            "wither_skeleton" => "rewo:be/wither_skeleton_skull".into(),
            "zombie" => "rewo:be/zombie_head".into(),
            "creeper" => "rewo:be/creeper_head".into(),
            "dragon" => "rewo:be/dragon_head".into(),
            "piglin" => "rewo:be/piglin_head".into(),
            _ => return None,
        },
        "minecraft:player_head" => "rewo:be/player_head".into(),
        "minecraft:copper_golem_statue" => {
            let texture = s("texture")?;
            let weather = ["exposed", "weathered", "oxidized"]
                .into_iter()
                .find(|w| texture.ends_with(&format!("_{w}.png")))
                .unwrap_or("unaffected");
            format!("rewo:be/statue/{weather}/{}", s("pose")?)
        }
        _ => return None,
    })
}

/// `Transformation`'s JSON form (`translation`, `left_rotation`, `scale`,
/// `right_rotation`; quaternions `[x, y, z, w]`) as `T · L · S · R`.
fn transformation_affine(t: &Value) -> crate::be_transform::Affine {
    use crate::be_transform::{mul, scale, translation};
    let arr = |k: &str, n: usize, d: f32| -> Vec<f32> {
        t.get(k)
            .and_then(|v| v.as_array())
            .filter(|a| a.len() == n)
            .map(|a| {
                a.iter()
                    .map(|x| x.as_f64().unwrap_or(d as f64) as f32)
                    .collect()
            })
            .unwrap_or_else(|| vec![d; n])
    };
    let quat = |k: &str| {
        let q = t
            .get(k)
            .and_then(|v| v.as_array())
            .filter(|a| a.len() == 4)
            .map(|a| [0, 1, 2, 3].map(|i| a[i].as_f64().unwrap_or(0.0) as f32))
            .unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let [x, y, z, w] = q;
        [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - z * w),
                2.0 * (x * z + y * w),
                0.0,
            ],
            [
                2.0 * (x * y + z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - x * w),
                0.0,
            ],
            [
                2.0 * (x * z - y * w),
                2.0 * (y * z + x * w),
                1.0 - 2.0 * (x * x + y * y),
                0.0,
            ],
        ]
    };
    let tr = arr("translation", 3, 0.0);
    let sc = arr("scale", 3, 1.0);
    let m = translation(tr[0], tr[1], tr[2]);
    let m = mul(&m, &quat("left_rotation"));
    let m = mul(&m, &scale(sc[0], sc[1], sc[2]));
    mul(&m, &quat("right_rotation"))
}

/// M242 — a special item: its block-entity geometry and the `base` model's
/// display transforms (each context from the nearest model in the parent
/// chain that declares it; a missing left hand takes the right, as
/// `ItemTransforms.Deserializer` does per model).
fn special_item(node: &Value, read_model: &mut dyn FnMut(&str) -> Option<Value>) -> ItemModel {
    let Some(spec) = node.get("model") else {
        return ItemModel::Unsupported(blocked(node, None));
    };
    let Some(key) = special_block_entity(spec) else {
        return ItemModel::Unsupported(blocked(node, None));
    };
    // The special node's `transformation` (`Transformation`: translation ·
    // left rotation · scale · right rotation), in block units, before the
    // display transform — the item-side twin of the block-entity renderer's
    // own transform (the shulker box's flip, the heads' turn). The statue's
    // renderer adds none, but its model's `setupAnim` turns the root by π
    // about Z, which Rewo keeps in the block-entity transform, not the bake.
    let mut xf = node
        .get("transformation")
        .map_or(crate::be_transform::IDENTITY, transformation_affine);
    if spec.get("type").and_then(|t| t.as_str()) == Some("minecraft:copper_golem_statue") {
        xf = crate::be_transform::mul(
            &xf,
            &crate::be_transform::rot_xyz(0.0, 0.0, std::f32::consts::PI),
        );
    }
    let Some(base) = node.get("base").and_then(|b| b.as_str()) else {
        return ItemModel::Unsupported("minecraft:special (no base)".into());
    };
    let mut t: [Option<DisplayTransform>; 7] = [None; 7];
    let mut cur = strip_ns(base).to_string();
    for _ in 0..16 {
        let Some(json) = read_model(&cur) else {
            break;
        };
        if let Some(d) = json.get("display") {
            let right = read_transform(d, "thirdperson_righthand");
            let first_right = read_transform(d, "firstperson_righthand");
            let here = [
                right,
                read_transform(d, "thirdperson_lefthand").or(right),
                read_transform(d, "ground"),
                read_transform(d, "gui"),
                first_right,
                read_transform(d, "firstperson_lefthand").or(first_right),
                read_transform(d, "fixed"),
            ];
            for (slot, v) in t.iter_mut().zip(here) {
                if slot.is_none() {
                    *slot = v;
                }
            }
        }
        match json.get("parent").and_then(|p| p.as_str()) {
            Some(p) if !strip_ns(p).starts_with("builtin/") => cur = strip_ns(p).to_string(),
            _ => break,
        }
    }
    let [r, l, ground, gui, fr, fl, fixed] = t.map(Option::unwrap_or_default);
    ItemModel::Resolved {
        geometry: ItemGeometry::Entity(key, xf),
        third_person_right: r,
        third_person_left: l,
        ground,
        gui,
        first_person_right: fr,
        first_person_left: fl,
        fixed,
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

/// `block/block.json`'s `fixed`: `rotation [0, 0, 0], scale 0.5` — a block
/// in an item frame (M194). Note 26.2's rotation is zero; `item/generated`'s
/// `fixed` is the one with the half-turn.
pub const BLOCK_FIXED: DisplayTransform = DisplayTransform {
    rotation: [0.0; 3],
    translation: [0.0; 3],
    scale: [0.5, 0.5, 0.5],
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
    ctx: SelectionContext<'_>,
) -> ItemModels {
    let mut by_name = HashMap::new();
    let mut unsupported: HashMap<String, usize> = HashMap::new();
    for full in names {
        let short = strip_ns(&full).to_string();
        let def = read(&format!("assets/minecraft/items/{short}.json"));
        let resolved = match def {
            Some(d) => {
                let mut read_model = |p: &str| read(&format!("assets/minecraft/models/{p}.json"));
                resolve_definition(&d, &mut read_model, ctx)
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

    /// M242 — the special renderers a block-entity bake covers map to its
    /// models; the rest stay suppressed.
    #[test]
    fn special_item_models_map_to_block_entity_models() {
        let k = |j: &str| special_block_entity(&json(j));
        assert_eq!(
            k(r#"{"type":"minecraft:chest","texture":"minecraft:normal"}"#).as_deref(),
            Some("rewo:be/chest")
        );
        assert_eq!(
            k(r#"{"type":"minecraft:chest","texture":"minecraft:copper_oxidized"}"#).as_deref(),
            Some("rewo:be/oxidized_copper_chest")
        );
        assert_eq!(
            k(r#"{"type":"minecraft:shulker_box","texture":"minecraft:shulker"}"#).as_deref(),
            Some("rewo:be/shulker_box")
        );
        assert_eq!(
            k(r#"{"type":"minecraft:shulker_box","texture":"minecraft:shulker_light_blue"}"#)
                .as_deref(),
            Some("rewo:be/light_blue_shulker_box")
        );
        assert_eq!(
            k(r#"{"type":"minecraft:head","kind":"wither_skeleton"}"#).as_deref(),
            Some("rewo:be/wither_skeleton_skull")
        );
        assert_eq!(
            k(r#"{"type":"minecraft:copper_golem_statue","pose":"sitting","texture":"minecraft:textures/entity/copper_golem/copper_golem_weathered.png"}"#).as_deref(),
            Some("rewo:be/statue/weathered/sitting")
        );
        assert_eq!(
            k(r#"{"type":"minecraft:copper_golem_statue","pose":"standing","texture":"minecraft:textures/entity/copper_golem/copper_golem.png"}"#).as_deref(),
            Some("rewo:be/statue/unaffected/standing")
        );
        assert_eq!(k(r#"{"type":"minecraft:shield"}"#), None);
        assert_eq!(k(r#"{"type":"minecraft:banner","color":"white"}"#), None);
    }

    /// The chest's Christmas select takes its fallback, and the special
    /// leaf resolves with the base model's display.
    #[test]
    fn a_chest_item_resolves_to_its_block_entity_with_the_template_display() {
        let def = json(
            r#"{"model":{"type":"minecraft:select","property":"minecraft:local_time","pattern":"MM-dd",
                "cases":[{"when":["12-24"],"model":{"type":"minecraft:special","base":"minecraft:item/chest",
                  "model":{"type":"minecraft:chest","texture":"minecraft:christmas"}}}],
                "fallback":{"type":"minecraft:special","base":"minecraft:item/chest",
                  "model":{"type":"minecraft:chest","texture":"minecraft:normal"}}}}"#,
        );
        let mut read = |p: &str| match p {
            "item/chest" => Some(json(r#"{"parent":"minecraft:item/template_chest"}"#)),
            "item/template_chest" => Some(json(
                r#"{"display":{"gui":{"rotation":[30,45,0],"scale":[0.625,0.625,0.625]},
                    "thirdperson_righthand":{"rotation":[75,315,0],"translation":[0,2.5,0],"scale":[0.375,0.375,0.375]}}}"#,
            )),
            _ => None,
        };
        match resolve_definition(&def, &mut read, SelectionContext::gui()) {
            ItemModel::Resolved {
                geometry: ItemGeometry::Entity(k, _),
                gui,
                third_person_left,
                ..
            } => {
                assert_eq!(k, "rewo:be/chest");
                assert_eq!(gui.rotation, [30.0, 45.0, 0.0]);
                assert_eq!(
                    third_person_left.rotation,
                    [75.0, 315.0, 0.0],
                    "left takes the right"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    /// The bow's tree, as the 26.2 jar ships it (`items/bow.json`).
    const BOW: &str = r#"{"type":"minecraft:condition","property":"minecraft:using_item",
        "on_false":{"type":"minecraft:model","model":"minecraft:item/bow"},
        "on_true":{"type":"minecraft:range_dispatch","property":"minecraft:use_duration",
          "scale":0.05,
          "entries":[
            {"threshold":0.65,"model":{"type":"minecraft:model","model":"minecraft:item/bow_pulling_1"}},
            {"threshold":0.9,"model":{"type":"minecraft:model","model":"minecraft:item/bow_pulling_2"}}],
          "fallback":{"type":"minecraft:model","model":"minecraft:item/bow_pulling_0"}}}"#;

    fn leaf_ref(node: &Value, ctx: SelectionContext<'_>) -> String {
        reduce_definition(node, ctx)
            .ok()
            .and_then(|l| l.get("model"))
            .and_then(|m| m.as_str())
            .unwrap_or("(none)")
            .to_string()
    }

    /// M234 — `use_duration` times `scale`, in float, against float
    /// thresholds: 13 ticks reach 0.65 and 18 reach 0.9.
    #[test]
    fn a_bows_pull_frames_change_at_thirteen_and_eighteen_ticks() {
        let bow = json(BOW);
        let hand = SelectionContext::hand();
        assert_eq!(leaf_ref(&bow, hand), "minecraft:item/bow");
        let at = |t: i32| leaf_ref(&bow, hand.with_use(t, 72000 - t, 0.0));
        assert_eq!(at(0), "minecraft:item/bow_pulling_0");
        assert_eq!(at(12), "minecraft:item/bow_pulling_0");
        assert_eq!(at(13), "minecraft:item/bow_pulling_1");
        assert_eq!(at(17), "minecraft:item/bow_pulling_1");
        assert_eq!(at(18), "minecraft:item/bow_pulling_2");
        assert_eq!(at(400), "minecraft:item/bow_pulling_2");
    }

    /// `"remaining": true` reads the remaining ticks instead, and `charge_type`
    /// picks its case (NONE takes the fallback).
    #[test]
    fn remaining_and_charge_type_are_read_from_the_context() {
        let remaining = json(
            r#"{"type":"minecraft:range_dispatch","property":"minecraft:use_duration",
                "remaining":true,
                "entries":[{"threshold":10,"model":{"type":"minecraft:model","model":"a"}}],
                "fallback":{"type":"minecraft:model","model":"b"}}"#,
        );
        let hand = SelectionContext::hand();
        assert_eq!(leaf_ref(&remaining, hand.with_use(50, 9, 0.0)), "b");
        assert_eq!(leaf_ref(&remaining, hand.with_use(0, 10, 0.0)), "a");
        let charge = json(
            r#"{"type":"minecraft:select","property":"minecraft:charge_type",
                "cases":[{"when":"arrow","model":{"type":"minecraft:model","model":"a"}},
                         {"when":"rocket","model":{"type":"minecraft:model","model":"r"}}],
                "fallback":{"type":"minecraft:model","model":"none"}}"#,
        );
        assert_eq!(leaf_ref(&charge, hand), "none");
        assert_eq!(leaf_ref(&charge, hand.with_charge(Some("arrow"))), "a");
        assert_eq!(leaf_ref(&charge, hand.with_charge(Some("rocket"))), "r");
    }

    /// The bake's leaf list: every model a use can reach (the plain one
    /// included), nothing for an item that never branches on use, and a
    /// display-context select is walked as the context selects it.
    #[test]
    fn use_variant_leaves_lists_what_a_use_can_reach() {
        let bow = json(BOW);
        let refs: Vec<&str> = use_variant_leaves(&bow, SelectionContext::hand())
            .iter()
            .filter_map(|l| l.get("model").and_then(|m| m.as_str()))
            .collect();
        assert_eq!(
            refs,
            [
                // `on_true` first (its fallback, then its entries), then
                // `on_false`.
                "minecraft:item/bow_pulling_0",
                "minecraft:item/bow_pulling_1",
                "minecraft:item/bow_pulling_2",
                "minecraft:item/bow"
            ]
        );
        let plain = json(r#"{"type":"minecraft:model","model":"minecraft:item/stick"}"#);
        assert!(use_variant_leaves(&plain, SelectionContext::hand()).is_empty());
        // A spyglass-like display select over a use condition: in the hand
        // the condition's two branches, in a slot nothing dynamic at all.
        let ctxd = json(
            r#"{"type":"minecraft:select","property":"minecraft:display_context",
                "cases":[{"when":"gui","model":{"type":"minecraft:model","model":"slot"}}],
                "fallback":{"type":"minecraft:condition","property":"minecraft:using_item",
                  "on_false":{"type":"minecraft:model","model":"rest"},
                  "on_true":{"type":"minecraft:model","model":"used"}}}"#,
        );
        assert_eq!(use_variant_leaves(&ctxd, SelectionContext::hand()).len(), 2);
        assert!(use_variant_leaves(&ctxd, SelectionContext::gui()).is_empty());
        // Behind a `special` (the shield, the trident) there is nothing the
        // bake could draw, so nothing is listed.
        let special = json(
            r#"{"type":"minecraft:condition","property":"minecraft:using_item",
                "on_false":{"type":"minecraft:special","base":"x","model":{"type":"minecraft:shield"}},
                "on_true":{"type":"minecraft:special","base":"y","model":{"type":"minecraft:shield"}}}"#,
        );
        assert!(use_variant_leaves(&special, SelectionContext::hand()).is_empty());
    }

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
            SelectionContext::hand(),
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
                fixed: BLOCK_FIXED,
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
            SelectionContext::hand(),
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
                assert_eq!(
                    geometry,
                    ItemGeometry::Sprite(vec!["item/diamond_sword".into()])
                );
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

    /// M40: `composite` and `special` are the two types the reduction does not
    /// evaluate, and a branching type with no `property` at all is malformed —
    /// all three must suppress rather than guess.
    #[test]
    fn a_definition_that_cannot_be_reduced_is_suppressed() {
        for kind in [
            "minecraft:special",
            "minecraft:composite",
            "minecraft:select",
            "minecraft:condition",
            "minecraft:range_dispatch",
        ] {
            let m = resolve_definition(
                &json(&format!(r#"{{"model":{{"type":"{kind}"}}}}"#)),
                &mut reader(vec![]),
                SelectionContext::hand(),
            );
            assert_eq!(
                m,
                ItemModel::Unsupported(kind.into()),
                "{kind} must suppress, not guess"
            );
        }
    }

    /// The reduction's own rules, on trees small enough to read.
    #[test]
    fn a_select_over_an_absent_component_takes_its_fallback() {
        let def = json(
            r#"{"model":{"type":"minecraft:select","property":"minecraft:trim_material",
                 "cases":[{"when":"minecraft:gold","model":{"type":"minecraft:model",
                   "model":"minecraft:block/gold_block"}}],
                 "fallback":{"type":"minecraft:model","model":"minecraft:block/dirt"}}}"#,
        );
        let m = resolve_definition(&def, &mut reader(vec![]), SelectionContext::gui());
        assert!(
            matches!(&m, ItemModel::Resolved { geometry: ItemGeometry::Block(b), .. } if b == "dirt"),
            "an untrimmed stack matches no case, so vanilla renders the fallback; got {m:?}"
        );
    }

    #[test]
    fn a_select_over_the_display_context_answers_per_context() {
        let def = json(
            r#"{"model":{"type":"minecraft:select","property":"minecraft:display_context",
                 "cases":[{"when":["gui","ground"],"model":{"type":"minecraft:model",
                   "model":"minecraft:block/dirt"}}],
                 "fallback":{"type":"minecraft:model","model":"minecraft:block/stone"}}}"#,
        );
        let in_slot = resolve_definition(&def, &mut reader(vec![]), SelectionContext::gui());
        let in_hand = resolve_definition(&def, &mut reader(vec![]), SelectionContext::hand());
        assert!(
            matches!(&in_slot, ItemModel::Resolved { geometry: ItemGeometry::Block(b), .. } if b == "dirt"),
            "the list form of `when` must match any of its names; got {in_slot:?}"
        );
        assert!(
            matches!(&in_hand, ItemModel::Resolved { geometry: ItemGeometry::Block(b), .. } if b == "stone"),
            "got {in_hand:?}"
        );
    }

    #[test]
    fn an_unevaluable_property_names_itself_in_the_reason() {
        let def = json(
            r#"{"model":{"type":"minecraft:select","property":"minecraft:context_dimension",
                 "cases":[],"fallback":{"type":"minecraft:model","model":"minecraft:block/dirt"}}}"#,
        );
        let m = resolve_definition(&def, &mut reader(vec![]), SelectionContext::gui());
        assert_eq!(
            m,
            ItemModel::Unsupported("minecraft:select (minecraft:context_dimension)".into()),
            "a property nobody has checked must suppress, and say which one"
        );
    }

    #[test]
    fn the_reduction_recurses_through_nested_branches() {
        // A bow's shape: a condition whose true branch is a range dispatch.
        let def = json(
            r#"{"model":{"type":"minecraft:condition","property":"minecraft:using_item",
                 "on_false":{"type":"minecraft:model","model":"minecraft:block/dirt"},
                 "on_true":{"type":"minecraft:range_dispatch","property":"minecraft:use_duration",
                   "entries":[{"threshold":0.65,"model":{"type":"minecraft:model",
                     "model":"minecraft:block/gold_block"}}],
                   "fallback":{"type":"minecraft:model","model":"minecraft:block/stone"}}}}"#,
        );
        let rest = resolve_definition(&def, &mut reader(vec![]), SelectionContext::hand());
        let using = resolve_definition(
            &def,
            &mut reader(vec![]),
            SelectionContext::hand().with_use(0, 72000, 0.0),
        );
        assert!(
            matches!(&rest, ItemModel::Resolved { geometry: ItemGeometry::Block(b), .. } if b == "dirt"),
            "got {rest:?}"
        );
        assert!(
            matches!(&using, ItemModel::Resolved { geometry: ItemGeometry::Block(b), .. } if b == "stone"),
            "a use just started has not reached the 0.65 threshold, so the              dispatch takes its fallback; got {using:?}"
        );
    }

    #[test]
    fn a_chain_that_never_reaches_builtin_generated_is_suppressed() {
        // `models/item/shield.json` has no parent at all.
        let m = resolve_definition(
            &json(r#"{"model":{"type":"minecraft:model","model":"minecraft:item/shield"}}"#),
            &mut reader(vec![(
                "item/shield",
                r#"{"textures":{"layer0":"item/shield"}}"#,
            )]),
            SelectionContext::hand(),
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
            SelectionContext::hand(),
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
            SelectionContext::hand(),
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
            SelectionContext::hand(),
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
            SelectionContext::hand(),
        );
        match m {
            ItemModel::Resolved { gui, .. } => {
                assert_eq!(gui.rotation, [30.0, 45.0, 0.0], "the child's entry wins");
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }
}
