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
    /// Several models drawn as one item, each placed by an affine in block
    /// units: a `minecraft:special` drawn by a block-entity renderer (M242:
    /// a chest, a shulker box, a head, a statue; M243: the conduit's shell,
    /// the decorated pot's base and four sides), or a `minecraft:composite`
    /// of block models (M243: a bed's head and foot). The affine is the
    /// node's `transformation`, plus whatever the renderer adds per part.
    ///
    /// The flag is the base model's `gui_light` (M244): `true` for `side`
    /// (the default, a block's 3D slot lighting), `false` for `front` (the
    /// flat item lighting a banner and a decorated pot use in a slot).
    Parts(Vec<(PartSource, crate::be_transform::Affine)>, bool),
}

/// Where one [`ItemGeometry::Parts`] part's quads come from.
#[derive(Clone, Debug, PartialEq)]
pub enum PartSource {
    /// `minecraft:block/<name>`, baked like a block item's.
    Block(String),
    /// A baked block-entity model (`rewo:be/...`).
    Entity(String),
    /// M244 — a baked block-entity model drawn with its textures multiplied
    /// by an sRGB colour: vanilla's per-vertex tint (a banner's dye on its
    /// greyscale base layer), baked into texture copies because item
    /// vertices carry no colour.
    TintedEntity(String, u32),
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
        /// `display.head` — the context `CustomHeadLayer` draws a worn
        /// non-skull item through (M274; `updateForLiving(..., HEAD, ...)`).
        /// `item/generated` declares `rotation [0, 180, 0], translation
        /// [0, 13, 7]`; `block/block.json` declares none (the identity).
        head: DisplayTransform,
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
    // `fixed` (M194) and `head` (M274).
    DisplayTransform,
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
    let mut head: Option<DisplayTransform> = None;
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
                head.unwrap_or_default(),
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
            if head.is_none() {
                head = read_transform(d, "head");
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
    /// `minecraft:context_dimension` — the owner's level, which the clock's
    /// `select` switches on. `None` matches no case (vanilla's answer with no
    /// level), so the clock takes its fallback.
    pub dimension: Option<&'a str>,
    /// The compass and clock properties' current values.
    pub needles: Needles,
}

/// What the numeric needle properties read (`CompassAngle`, `Time`), in
/// **turns** (0..1) before the definition's `scale`.
///
/// A compass is a `range_dispatch` over `minecraft:compass`, whose value
/// depends on where the owner is and which way they face; the clock's depends
/// on the time of day. The caller computes them once per frame and the
/// reduction only compares them with the thresholds. All zero is the model at
/// the first threshold: the compass's `item/compass_16` and the clock's
/// `item/clock_00`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Needles {
    /// `CompassAngleState` for `target: spawn`.
    pub compass_spawn: f32,
    /// ...for `target: recovery` (the last death location).
    pub compass_recovery: f32,
    /// `Time` with `source: daytime`: the sun angle over 360.
    pub clock_daytime: f32,
}

/// `Mth.positiveModulo(float, float)`.
fn positive_modulo(x: f32, y: f32) -> f32 {
    (x % y + y) % y
}

/// `NeedleDirectionHelper.standardWobbler`: the damped spring a compass or
/// clock needle swings on. Vanilla keeps one per item model and advances it
/// at most once per game tick, so it is driven here by the tick number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wobbler {
    rotation: f32,
    delta_rotation: f32,
    last_update_tick: i64,
    /// `0.8` for the compass (`CompassAngleState`), `0.9` for the clock.
    factor: f32,
}

impl Wobbler {
    pub fn new(factor: f32) -> Self {
        Self {
            rotation: 0.0,
            delta_rotation: 0.0,
            last_update_tick: 0,
            factor,
        }
    }

    /// The current rotation, in turns.
    pub fn rotation(&self) -> f32 {
        self.rotation
    }

    /// `shouldUpdate(tick)` then `update(tick, target)`: pull the needle
    /// toward `target` (turns), once per distinct tick.
    pub fn update(&mut self, tick: i64, target: f32) {
        if self.last_update_tick == tick {
            return;
        }
        self.last_update_tick = tick;
        let temp = positive_modulo(target - self.rotation + 0.5, 1.0) - 0.5;
        self.delta_rotation += temp * 0.1;
        self.delta_rotation *= self.factor;
        self.rotation = positive_modulo(self.rotation + self.delta_rotation, 1.0);
    }
}

/// `CompassAngleState.getRotationTowardsCompassTarget`'s two inputs for an
/// owner at `owner` (feet position) whose body faces `body_yaw_deg`, with the
/// target block `target`; `None` when the target is on top of the owner
/// (`isValidCompassTargetPos`'s `distToCenterSqr < 1.0E-5`, which sends the
/// needle spinning). The dimension test is the caller's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompassAim {
    /// `getAngleFromEntityToPos`: `atan2(dz, dx) / 2π` toward the block's
    /// centre.
    pub angle_to_target: f32,
    /// `getWrappedVisualRotationY`: the owner's body yaw over 360, wrapped.
    pub owner_y: f32,
}

impl CompassAim {
    pub fn new(owner: [f64; 3], body_yaw_deg: f32, target: [i32; 3]) -> Option<Self> {
        let centre = |v: i32| f64::from(v) + 0.5;
        let (dx, dy, dz) = (
            centre(target[0]) - owner[0],
            centre(target[1]) - owner[1],
            centre(target[2]) - owner[2],
        );
        // The comparison is against a float literal widened to double.
        if dx * dx + dy * dy + dz * dz < f64::from(1.0E-5f32) {
            return None;
        }
        Some(Self {
            // `Math.atan2(..) / (float)(Math.PI * 2)`.
            angle_to_target: (dz.atan2(dx) / f64::from((std::f64::consts::PI * 2.0) as f32)) as f32,
            owner_y: positive_modulo(body_yaw_deg / 360.0, 1.0),
        })
    }

    /// The needle for any owner but the local player (and the value a
    /// settled local needle converges on): `0.5 - (ownerY - 0.25 - angle)`.
    pub fn turns(&self) -> f32 {
        positive_modulo(0.5 - (self.owner_y - 0.25 - self.angle_to_target), 1.0)
    }

    /// What the local player's wobbler is pulled toward each tick.
    pub fn wobble_target(&self) -> f32 {
        0.5 - (self.owner_y - 0.25)
    }

    /// The local player's needle: the angle to the target plus the wobbler's
    /// swing (`angleToTarget + wobbler.rotation()`).
    pub fn turns_wobbled(&self, wobbler: &Wobbler) -> f32 {
        positive_modulo(self.angle_to_target + wobbler.rotation(), 1.0)
    }
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
            dimension: Some("minecraft:overworld"),
            needles: Needles {
                compass_spawn: 0.0,
                compass_recovery: 0.0,
                clock_daytime: 0.0,
            },
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

    /// The same context for an owner in `dimension`, with these needle values.
    pub fn with_needles(self, dimension: Option<&'a str>, needles: Needles) -> Self {
        Self {
            dimension,
            needles,
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

/// [`reduce_definition`], plus the `transformation`s of the branch nodes the
/// walk passed through, composed parent first — vanilla's
/// `Transformation.compose(parent, this)` as each `Unbaked.bake` hands its
/// matrix down (M244). The leaf's own `transformation` is not included; the
/// leaf's resolver reads it. In 26.2 only the copper golem statue's
/// `select` (and the shield's and trident's `condition`, above specials
/// Rewo does not map) carries one.
fn reduce_placed<'v>(
    node: &'v Value,
    ctx: SelectionContext<'_>,
) -> Result<(&'v Value, crate::be_transform::Affine), String> {
    let mut node = node;
    let mut xf = crate::be_transform::IDENTITY;
    for _ in 0..=16 {
        match chosen_child(node, ctx)? {
            None => return Ok((node, xf)),
            Some(next) => {
                if let Some(t) = node.get("transformation") {
                    xf = crate::be_transform::mul(&xf, &transformation_affine(t));
                }
                node = next;
            }
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
            } else if property == "minecraft:context_dimension" {
                // The clock: an overworld case with the day/night dial, and a
                // fallback that spins at random everywhere else.
                ctx.dimension.and_then(|want| case_for(node, want))
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
                // `CompassAngle`. A lodestone target needs the stack's
                // `lodestone_tracker`, which `has_component` reads as absent
                // here, so only the targets a plain stack reaches are known.
                "minecraft:compass" => match node.get("target").and_then(|t| t.as_str()) {
                    Some("spawn") => ctx.needles.compass_spawn,
                    Some("recovery") => ctx.needles.compass_recovery,
                    _ => return Err(blocked(node, Some(property))),
                },
                // `Time`. `random` spins in the Nether and the End; a still
                // needle is the recorded approximation.
                "minecraft:time" => match node.get("source").and_then(|t| t.as_str()) {
                    Some("daytime") => ctx.needles.clock_daytime,
                    Some("random") => 0.0,
                    _ => return Err(blocked(node, Some(property))),
                },
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
        // M243 — so is a `composite`: `resolve_definition` draws one whose
        // models are all plain block models (the beds) and suppresses the
        // rest (the open bundle, whose middle layer is the selected item).
        "minecraft:composite" => Ok(None),
        _ => Err(blocked(node, None)),
    }
}

/// Whether `node` branches on live state the draw path supplies: the owner's
/// use or a crossbow's charge (M234) -- a `using_item` condition, a
/// `use_duration` / `crossbow/pull` range dispatch, a `charge_type` select --
/// or a needle (the compass's and clock's range dispatches).
fn branches_on_use(node: &Value) -> bool {
    let property = node.get("property").and_then(|p| p.as_str());
    match node.get("type").and_then(|t| t.as_str()) {
        Some("minecraft:condition") => property == Some("minecraft:using_item"),
        Some("minecraft:range_dispatch") => matches!(
            property,
            Some(
                "minecraft:use_duration"
                    | "minecraft:crossbow/pull"
                    | "minecraft:compass"
                    | "minecraft:time"
            )
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
    let (model, outer) = match reduce_placed(root, ctx) {
        Ok(m) => m,
        // The reason names the node the walk **stopped at**, not the root: a
        // `condition` whose chosen branch is a `special` is blocked by the
        // special, and bucketing it under `condition` would read as "the
        // reduction cannot do conditions", which is the opposite of true.
        Err(reason) => return ItemModel::Unsupported(reason),
    };
    match model.get("type").and_then(|t| t.as_str()) {
        Some("minecraft:special") => return special_item(model, &outer, read_model),
        Some("minecraft:composite") => return composite_item(model, &outer, read_model),
        // A plain model under a transformed branch would need its geometry
        // placed; no 26.2 item has one, so the branch transform is dropped
        // here rather than every block and sprite item becoming `Parts`.
        _ => {}
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
            head: DisplayTransform::IDENTITY,
        };
    }

    match resolve_chain(reference, read_model) {
        Some((true, layers, right, left, ground, gui, first_right, first_left, fixed, head))
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
                head,
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
        // M243 — `ConduitSpecialRenderer` draws the shell model part alone.
        "minecraft:conduit" => crate::block_entity_models::CONDUIT.0.into(),
        // M246 — `ShieldSpecialRenderer` (no patterns: the stack's
        // `banner_patterns` never reach an item model) and
        // `TridentSpecialRenderer`, each its model with no extra transform;
        // the definitions' `condition` carries the flip.
        "minecraft:shield" => crate::block_entity_models::SHIELD_MODEL.0.into(),
        "minecraft:trident" => crate::block_entity_models::TRIDENT_MODEL.0.into(),
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
fn special_item(
    node: &Value,
    outer: &crate::be_transform::Affine,
    read_model: &mut dyn FnMut(&str) -> Option<Value>,
) -> ItemModel {
    let Some(spec) = node.get("model") else {
        return ItemModel::Unsupported(blocked(node, None));
    };
    let kind = spec.get("type").and_then(|t| t.as_str());
    // The parts in the renderer's own model space. The decorated pot is the
    // one multi-part special: `DecoratedPotRenderer.submit` draws the base
    // (neck, top, bottom) and the four sides, each side posed by its
    // `PartPose` (`be_transform::pot_side`, written in px), and a stack with
    // no `pot_decorations` shows the plain side on all four.
    let parts: Vec<(PartSource, crate::be_transform::Affine)> =
        if kind == Some("minecraft:decorated_pot") {
            let side = crate::block_entity_models::pot_side_model(None);
            std::iter::once((
                PartSource::Entity(crate::block_entity_models::POT_BASE_MODEL.0.into()),
                crate::be_transform::IDENTITY,
            ))
            .chain((0..4).map(|i| {
                let mut m = crate::be_transform::pot_side(i);
                for row in &mut m {
                    row[3] /= 16.0; // px → block units; the rotation is unitless
                }
                (PartSource::Entity(side.clone()), m)
            }))
            .collect()
        } else if kind == Some("minecraft:banner") {
            // `BannerRenderer.submitSpecial`: the standing pole and bar, then
            // the cloth at `setupAnim(0)`'s rest tilt, then the base colour —
            // `Sheets.BANNER_PATTERN_BASE` on the same flag geometry, tinted
            // with the dye. The base mask is opaque wherever the bare cloth
            // is, so drawing it alone on the flag gives the cloth-then-base
            // result without two coplanar layers. Pattern layers come from
            // the stack's `banner_patterns`, which an item model never sees.
            use crate::block_entity_models as bem;
            let Some(colour) = spec
                .get("color")
                .and_then(|c| c.as_str())
                .and_then(|c| bem::DYE_COLORS.iter().position(|d| *d == strip_ns(c)))
            else {
                return ItemModel::Unsupported(blocked(node, None));
            };
            // The entity pass applies a part transform to `v - pivot` (the
            // flag is baked where it hangs), so the item does the same.
            let p = bem::BANNER_STANDING_FLAG_PIVOT;
            let mut flag = crate::be_transform::mul(
                &crate::be_transform::banner_flag_part(0.0, p),
                &crate::be_transform::translation(-p[0], -p[1], -p[2]),
            );
            for row in &mut flag {
                row[3] /= 16.0; // px → block units
            }
            vec![
                (
                    PartSource::Entity(bem::BANNER_STANDING_BODY_MODEL.into()),
                    crate::be_transform::IDENTITY,
                ),
                (
                    PartSource::TintedEntity(
                        "rewo:be/banner_pattern/base".into(),
                        bem::DYE_DIFFUSE_COLORS[colour],
                    ),
                    flag,
                ),
            ]
        } else {
            match special_block_entity(spec) {
                Some(key) => vec![(PartSource::Entity(key), crate::be_transform::IDENTITY)],
                None => return ItemModel::Unsupported(blocked(node, None)),
            }
        };
    // The special node's `transformation` (`Transformation`: translation ·
    // left rotation · scale · right rotation), in block units, before the
    // display transform — the item-side twin of the block-entity renderer's
    // own transform (the shulker box's flip, the heads' turn). The statue's
    // renderer adds none, but its model's `setupAnim` turns the root by π
    // about Z, which Rewo keeps in the block-entity transform, not the bake.
    // Under the branch transforms above it (the statue's `select` flips
    // y and z and lifts it 1.5 blocks: M244).
    let mut xf = crate::be_transform::mul(
        outer,
        &node
            .get("transformation")
            .map_or(crate::be_transform::IDENTITY, transformation_affine),
    );
    if kind == Some("minecraft:copper_golem_statue") {
        xf = crate::be_transform::mul(
            &xf,
            &crate::be_transform::rot_xyz(0.0, 0.0, std::f32::consts::PI),
        );
    }
    let Some(base) = node.get("base").and_then(|b| b.as_str()) else {
        return ItemModel::Unsupported("minecraft:special (no base)".into());
    };
    let ([r, l, ground, gui, fr, fl, fixed, head], side_lit) = chain_display(base, read_model);
    ItemModel::Resolved {
        geometry: ItemGeometry::Parts(
            parts
                .into_iter()
                .map(|(src, m)| (src, crate::be_transform::mul(&xf, &m)))
                .collect(),
            side_lit,
        ),
        third_person_right: r,
        third_person_left: l,
        ground,
        gui,
        first_person_right: fr,
        first_person_left: fl,
        fixed,
        head,
    }
}

/// M243 — a `minecraft:composite` whose models are all plain `block/...`
/// models (each optionally placed by its `transformation`): the beds, whose
/// foot is the head's model one block south. Vanilla resolves each layer's
/// display from its own model; a bed's two share `template_bed`'s, and this
/// takes the first layer's. Anything else in the list (the open bundle's
/// `bundle/selected_item`) suppresses the whole item.
fn composite_item(
    node: &Value,
    outer: &crate::be_transform::Affine,
    read_model: &mut dyn FnMut(&str) -> Option<Value>,
) -> ItemModel {
    let models = node
        .get("models")
        .and_then(|m| m.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut parts = Vec::new();
    for m in models {
        let reference = (m.get("type").and_then(|t| t.as_str()) == Some("minecraft:model"))
            .then(|| m.get("model").and_then(|r| r.as_str()))
            .flatten()
            .map(strip_ns);
        let Some(block) = reference.and_then(|r| r.strip_prefix("block/")) else {
            return ItemModel::Unsupported(blocked(node, None));
        };
        let own = node
            .get("transformation")
            .map_or(crate::be_transform::IDENTITY, transformation_affine);
        let xf = m
            .get("transformation")
            .map_or(crate::be_transform::IDENTITY, transformation_affine);
        let xf = crate::be_transform::mul(&crate::be_transform::mul(outer, &own), &xf);
        parts.push((PartSource::Block(block.to_string()), xf));
    }
    let Some((PartSource::Block(first), _)) = parts.first() else {
        return ItemModel::Unsupported(blocked(node, None));
    };
    let ([r, l, ground, gui, fr, fl, fixed, head], side_lit) =
        chain_display(&format!("block/{first}"), read_model);
    ItemModel::Resolved {
        geometry: ItemGeometry::Parts(parts, side_lit),
        third_person_right: r,
        third_person_left: l,
        ground,
        gui,
        first_person_right: fr,
        first_person_left: fl,
        fixed,
        head,
    }
}

/// M245 — the `Direction` ordinal (down 0, up 1, north 2, south 3, west 4,
/// east 5) a face of direction `dir` points along once `xf`'s linear part has
/// turned it: the nearest of the six. Exact for the part transforms items use
/// (quarter turns, flips, uniform scales); the banner's 0.45° cloth tilt
/// snaps back to the face it was.
pub fn turned_dir(xf: &crate::be_transform::Affine, dir: u8) -> u8 {
    const N: [[f32; 3]; 6] = [
        [0.0, -1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, -1.0],
        [0.0, 0.0, 1.0],
        [-1.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
    ];
    let Some(n) = N.get(dir as usize) else {
        return dir;
    };
    let r: [f32; 3] = std::array::from_fn(|i| xf[i][0] * n[0] + xf[i][1] * n[1] + xf[i][2] * n[2]);
    (0..6u8)
        .max_by(|&a, &b| {
            let d = |k: u8| N[k as usize].iter().zip(r).map(|(x, y)| x * y).sum::<f32>();
            d(a).total_cmp(&d(b))
        })
        .unwrap_or(dir)
}

/// The seven display contexts a model chain declares, each from the nearest
/// model that declares it (a missing left hand takes the right, as
/// `ItemTransforms.Deserializer` does per model); absent ones are the
/// identity. Also the nearest `gui_light` (M244), `true` unless it says
/// `front`: vanilla's `usesBlockLight`, which picks a slot's lighting.
fn chain_display(
    start: &str,
    read_model: &mut dyn FnMut(&str) -> Option<Value>,
) -> ([DisplayTransform; 8], bool) {
    let mut t: [Option<DisplayTransform>; 8] = [None; 8];
    let mut gui_light: Option<bool> = None;
    let mut cur = strip_ns(start).to_string();
    for _ in 0..16 {
        let Some(json) = read_model(&cur) else {
            break;
        };
        if gui_light.is_none() {
            gui_light = json
                .get("gui_light")
                .and_then(|g| g.as_str())
                .map(|g| g != "front");
        }
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
                read_transform(d, "head"),
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
    (t.map(Option::unwrap_or_default), gui_light.unwrap_or(true))
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
        assert_eq!(
            k(r#"{"type":"minecraft:conduit"}"#).as_deref(),
            Some("rewo:be/conduit")
        );
        assert_eq!(
            k(r#"{"type":"minecraft:shield"}"#).as_deref(),
            Some("rewo:be/shield")
        );
        assert_eq!(
            k(r#"{"type":"minecraft:trident"}"#).as_deref(),
            Some("rewo:be/trident")
        );
        assert_eq!(k(r#"{"type":"minecraft:banner","color":"white"}"#), None);
        assert_eq!(k(r#"{"type":"minecraft:bed"}"#), None);
    }

    /// M245 — a face's direction follows a part's turn: the statue's flip
    /// (`diag(-1, 1, -1)`) swaps north and south, east and west, and keeps
    /// up; a half-turn about X swaps up and down.
    #[test]
    fn a_turned_part_turns_its_faces() {
        let flip = [
            [-1.0, 0.0, 0.0, 0.5],
            [0.0, 1.0, 0.0, 1.5],
            [0.0, 0.0, -1.0, 0.5],
        ];
        assert_eq!(turned_dir(&flip, 2), 3);
        assert_eq!(turned_dir(&flip, 5), 4);
        assert_eq!(turned_dir(&flip, 1), 1);
        let rx = crate::be_transform::rot_xyz(std::f32::consts::PI, 0.0, 0.0);
        assert_eq!(turned_dir(&rx, 1), 0);
        assert_eq!(turned_dir(&crate::be_transform::IDENTITY, 4), 4);
    }

    /// M244 — a branch node's `transformation` reaches the special below it,
    /// composed parent first: the statue's `select` (flip y and z, lift 1.5)
    /// then the special's own (none), then the model's `setupAnim` turn.
    #[test]
    fn a_branch_transformation_composes_onto_the_special() {
        let def = json(
            r#"{"model":{"type":"minecraft:select","property":"minecraft:block_state",
                "block_state_property":"copper_golem_pose",
                "transformation":{"left_rotation":[0,0,0,1],"right_rotation":[0,0,0,1],
                  "scale":[1,-1,-1],"translation":[0.5,1.5,0.5]},
                "fallback":{"type":"minecraft:special","base":"x","model":{
                  "type":"minecraft:copper_golem_statue","pose":"standing",
                  "texture":"minecraft:textures/entity/copper_golem/copper_golem.png"}}}}"#,
        );
        match resolve_definition(&def, &mut reader(vec![]), SelectionContext::gui()) {
            ItemModel::Resolved {
                geometry: ItemGeometry::Parts(parts, _),
                ..
            } => {
                let m = parts[0].1;
                // scale(1,-1,-1) · rotZ(π) = diag(-1, 1, -1), then the lift.
                let want = [
                    [-1.0, 0.0, 0.0, 0.5],
                    [0.0, 1.0, 0.0, 1.5],
                    [0.0, 0.0, -1.0, 0.5],
                ];
                for i in 0..3 {
                    for j in 0..4 {
                        assert!((m[i][j] - want[i][j]).abs() < 1e-5, "{m:?}");
                    }
                }
            }
            other => panic!("{other:?}"),
        }
    }

    /// M244 — a banner item is its standing body and the dye-tinted base
    /// layer on the flag's rest pose.
    #[test]
    fn a_banner_item_is_its_body_and_its_tinted_base() {
        let def = json(
            r#"{"model":{"type":"minecraft:special","base":"minecraft:item/template_banner",
                "model":{"type":"minecraft:banner","color":"red"}}}"#,
        );
        // `template_banner` declares `gui_light: front` — the flat slot light.
        let mut read = reader(vec![(
            "item/template_banner",
            r#"{"gui_light":"front","display":{"gui":{"rotation":[30,20,0]}}}"#,
        )]);
        match resolve_definition(&def, &mut read, SelectionContext::gui()) {
            ItemModel::Resolved {
                geometry: ItemGeometry::Parts(parts, side_lit),
                ..
            } => {
                assert!(!side_lit, "a banner is lit flat in a slot");
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0].0, PartSource::Entity("rewo:be/banner_body".into()));
                assert_eq!(
                    parts[1].0,
                    PartSource::TintedEntity("rewo:be/banner_pattern/base".into(), 0xB02E26)
                );
                // At rest the flag only tilts about its pivot: a point AT the
                // pivot (-44 px up, block units) stays put.
                let m = parts[1].1;
                let y = -44.0 / 16.0;
                assert!((m[1][1] * y + m[1][3] - y).abs() < 1e-5, "{m:?}");
                assert!((m[2][1] * y + m[2][3]).abs() < 1e-5, "{m:?}");
            }
            other => panic!("{other:?}"),
        }
        let bad = json(
            r#"{"model":{"type":"minecraft:special","base":"x",
                "model":{"type":"minecraft:banner","color":"chartreuse"}}}"#,
        );
        assert_eq!(
            resolve_definition(&bad, &mut reader(vec![]), SelectionContext::gui()),
            ItemModel::Unsupported("minecraft:special".into())
        );
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
                geometry: ItemGeometry::Parts(parts, _),
                gui,
                third_person_left,
                ..
            } => {
                assert_eq!(parts.len(), 1);
                assert_eq!(parts[0].0, PartSource::Entity("rewo:be/chest".into()));
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
                head: DisplayTransform::IDENTITY,
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

    /// M243 — a bed: two block models, the foot one block south, with the
    /// head model's chain display; an open bundle's composite (a dynamic
    /// middle layer) and a decorated pot's five parts.
    #[test]
    fn composites_of_block_models_resolve_and_others_suppress() {
        let bed = json(
            r#"{"model":{"type":"minecraft:composite","models":[
                {"type":"minecraft:model","model":"minecraft:block/red_bed_head"},
                {"type":"minecraft:model","model":"minecraft:block/red_bed_foot",
                 "transformation":{"left_rotation":[0,0,0,1],"right_rotation":[0,0,0,1],
                   "scale":[1,1,1],"translation":[0,0,1]}}]}}"#,
        );
        let mut read = |p: &str| match p {
            "block/red_bed_head" => Some(json(r#"{"parent":"block/template_bed_head"}"#)),
            "block/template_bed_head" => Some(json(r#"{"parent":"block/template_bed"}"#)),
            "block/template_bed" => Some(json(
                r#"{"display":{"gui":{"rotation":[30,340,0],"translation":[2,3,0],"scale":[0.5325,0.5325,0.5325]}}}"#,
            )),
            _ => None,
        };
        match resolve_definition(&bed, &mut read, SelectionContext::gui()) {
            ItemModel::Resolved {
                geometry: ItemGeometry::Parts(parts, _),
                gui,
                ..
            } => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0].0, PartSource::Block("red_bed_head".into()));
                assert_eq!(parts[1].0, PartSource::Block("red_bed_foot".into()));
                assert_eq!(parts[1].1[2][3], 1.0, "the foot is one block south");
                assert_eq!(parts[0].1, crate::be_transform::IDENTITY);
                assert_eq!(gui.rotation, [30.0, 340.0, 0.0]);
            }
            other => panic!("{other:?}"),
        }
        let bundle = json(
            r#"{"model":{"type":"minecraft:composite","models":[
                {"type":"minecraft:model","model":"minecraft:item/red_bundle_open_back"},
                {"type":"minecraft:bundle/selected_item"}]}}"#,
        );
        assert_eq!(
            resolve_definition(&bundle, &mut reader(vec![]), SelectionContext::gui()),
            ItemModel::Unsupported("minecraft:composite".into())
        );
        let pot = json(
            r#"{"model":{"type":"minecraft:special","base":"minecraft:item/decorated_pot",
                "model":{"type":"minecraft:decorated_pot"}}}"#,
        );
        match resolve_definition(&pot, &mut reader(vec![]), SelectionContext::gui()) {
            ItemModel::Resolved {
                geometry: ItemGeometry::Parts(parts, _),
                ..
            } => {
                assert_eq!(parts.len(), 5);
                assert_eq!(
                    parts[0].0,
                    PartSource::Entity("rewo:be/decorated_pot".into())
                );
                // The back side's `PartPose` offset (15, 16, 1) px in block units.
                let back = parts[1].1;
                assert!((back[0][3] - 15.0 / 16.0).abs() < 1e-6, "{back:?}");
                assert!((back[1][3] - 1.0).abs() < 1e-6, "{back:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// M40: `composite` and `special` are the two types the reduction does not
    /// evaluate, and a branching type with no `property` at all is malformed —
    /// all three must suppress rather than guess. (M242/M243 resolve the
    /// specials and composites they can; an empty one still suppresses.)
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
            r#"{"model":{"type":"minecraft:select","property":"minecraft:context_entity_type",
                 "cases":[],"fallback":{"type":"minecraft:model","model":"minecraft:block/dirt"}}}"#,
        );
        let m = resolve_definition(&def, &mut reader(vec![]), SelectionContext::gui());
        assert_eq!(
            m,
            ItemModel::Unsupported("minecraft:select (minecraft:context_entity_type)".into()),
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

    /// A compass's `range_dispatch` over `minecraft:compass`, three entries
    /// out of 33: thresholds 0, 0.5 and 31.5 on a scale of 32.
    const COMPASS: &str = r#"{"model":{"type":"minecraft:condition","property":"minecraft:has_component",
        "component":"minecraft:lodestone_tracker",
        "on_true":{"type":"minecraft:model","model":"minecraft:item/lodestone"},
        "on_false":{"type":"minecraft:range_dispatch","property":"minecraft:compass",
            "target":"spawn","scale":32.0,"entries":[
              {"threshold":0.0,"model":{"type":"minecraft:model","model":"minecraft:item/compass_16"}},
              {"threshold":0.5,"model":{"type":"minecraft:model","model":"minecraft:item/compass_17"}},
              {"threshold":31.5,"model":{"type":"minecraft:model","model":"minecraft:item/compass_16"}}]}}}"#;

    fn leaf_of(def: &str, ctx: SelectionContext<'_>) -> String {
        let def = json(def);
        reduce_definition(&def["model"], ctx)
            .expect("reduces")
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap()
            .to_string()
    }

    /// A compass is no longer suppressed: with every needle at zero it is the
    /// first threshold's model, and the needle value moves it along the
    /// entries (`RangeSelectItemModel` takes the last threshold reached, and
    /// the scaled value wraps back to the first model past 31.5).
    #[test]
    fn a_compass_reduces_by_its_needle() {
        let at = |turns: f32| {
            leaf_of(
                COMPASS,
                SelectionContext::hand().with_needles(
                    Some("minecraft:overworld"),
                    Needles {
                        compass_spawn: turns,
                        ..Needles::default()
                    },
                ),
            )
        };
        assert_eq!(
            leaf_of(COMPASS, SelectionContext::hand()),
            "minecraft:item/compass_16"
        );
        assert_eq!(at(0.0), "minecraft:item/compass_16");
        assert_eq!(at(0.5 / 32.0), "minecraft:item/compass_17");
        assert_eq!(at(0.9), "minecraft:item/compass_17");
        assert_eq!(at(31.6 / 32.0), "minecraft:item/compass_16");
    }

    /// The recovery compass reads the recovery needle, not the spawn one.
    #[test]
    fn a_recovery_compass_reads_the_recovery_needle() {
        let def = COMPASS.replace("\"target\":\"spawn\"", "\"target\":\"recovery\"");
        let ctx = |spawn: f32, recovery: f32| {
            SelectionContext::hand().with_needles(
                Some("minecraft:overworld"),
                Needles {
                    compass_spawn: spawn,
                    compass_recovery: recovery,
                    clock_daytime: 0.0,
                },
            )
        };
        assert_eq!(leaf_of(&def, ctx(0.5, 0.0)), "minecraft:item/compass_16");
        assert_eq!(leaf_of(&def, ctx(0.0, 0.5)), "minecraft:item/compass_17");
    }

    /// An unknown compass target is suppressed, not guessed.
    #[test]
    fn an_unknown_compass_target_is_suppressed() {
        let def = json(&COMPASS.replace("\"target\":\"spawn\"", "\"target\":\"mars\""));
        assert!(reduce_definition(&def["model"], SelectionContext::hand()).is_err());
    }

    /// The clock: the overworld case dials the sun angle, any other dimension
    /// takes the fallback (a `random` needle, held still here).
    #[test]
    fn a_clock_dials_the_sun_in_the_overworld_only() {
        let clock = r#"{"model":{"type":"minecraft:select","property":"minecraft:context_dimension",
            "cases":[{"when":"minecraft:overworld","model":{"type":"minecraft:range_dispatch",
                "property":"minecraft:time","source":"daytime","scale":64.0,"entries":[
                {"threshold":0.0,"model":{"type":"minecraft:model","model":"minecraft:item/clock_00"}},
                {"threshold":32.0,"model":{"type":"minecraft:model","model":"minecraft:item/clock_32"}}]}}],
            "fallback":{"type":"minecraft:range_dispatch","property":"minecraft:time",
                "source":"random","scale":64.0,"entries":[
                {"threshold":0.0,"model":{"type":"minecraft:model","model":"minecraft:item/clock_00"}},
                {"threshold":32.0,"model":{"type":"minecraft:model","model":"minecraft:item/clock_32"}}]}}}"#;
        let ctx = |dim: Option<&'static str>, t: f32| {
            SelectionContext::hand().with_needles(
                dim,
                Needles {
                    clock_daytime: t,
                    ..Needles::default()
                },
            )
        };
        assert_eq!(
            leaf_of(clock, ctx(Some("minecraft:overworld"), 0.75)),
            "minecraft:item/clock_32"
        );
        assert_eq!(
            leaf_of(clock, ctx(Some("minecraft:the_nether"), 0.75)),
            "minecraft:item/clock_00"
        );
        assert_eq!(
            leaf_of(clock, ctx(None, 0.75)),
            "minecraft:item/clock_00",
            "no level matches no case"
        );
    }

    /// Every model a compass can reach is a variant the bake must make.
    #[test]
    fn a_compass_bakes_every_frame_as_a_variant() {
        let def = json(COMPASS);
        let leaves = use_variant_leaves(&def["model"], SelectionContext::hand());
        let models: Vec<&str> = leaves
            .iter()
            .filter_map(|l| l.get("model").and_then(|m| m.as_str()))
            .collect();
        assert!(models.contains(&"minecraft:item/compass_16"));
        assert!(models.contains(&"minecraft:item/compass_17"), "{models:?}");
    }

    /// Facing the target, the needle points straight ahead (turn 0), whichever
    /// way the owner faces. Derived from the compass directions, not from the
    /// formula: Minecraft yaw 0 faces +Z, 90 faces -X, -90 (270) faces +X.
    #[test]
    fn facing_the_target_points_the_needle_ahead() {
        for (yaw, target) in [
            (0.0f32, [0, 0, 10]),
            (90.0, [-10, 0, 0]),
            (-90.0, [10, 0, 0]),
            (180.0, [0, 0, -10]),
        ] {
            let aim = CompassAim::new([0.5, 0.0, 0.5], yaw, target).unwrap();
            let t = aim.turns();
            assert!(t < 1e-4 || t > 1.0 - 1e-4, "yaw {yaw}: {t}");
        }
        // Turning right (yaw decreases) with the target still south: the
        // needle swings the other way round the dial.
        let right = CompassAim::new([0.5, 0.0, 0.5], -90.0, [0, 0, 10]).unwrap();
        assert!((right.turns() - 0.75).abs() < 1e-4 || (right.turns() - 0.25).abs() < 1e-4);
    }

    /// A target on top of the owner has no direction.
    #[test]
    fn a_target_under_the_owner_has_no_aim() {
        assert!(CompassAim::new([0.5, 64.5, 0.5], 0.0, [0, 64, 0]).is_none());
        assert!(CompassAim::new([0.5, 64.0, 0.5], 0.0, [0, 64, 1]).is_some());
    }

    /// The wobbler moves once per game tick, and settles on a steady target.
    #[test]
    fn the_wobbler_updates_once_per_tick_and_settles() {
        let mut w = Wobbler::new(0.8);
        w.update(1, 0.25);
        let first = w.rotation();
        assert!(first > 0.0 && first < 0.25, "{first}");
        w.update(1, 0.25);
        assert_eq!(
            w.rotation(),
            first,
            "a second update in one tick is a no-op"
        );
        for tick in 2..400 {
            w.update(tick, 0.25);
        }
        assert!((w.rotation() - 0.25).abs() < 1e-3, "{}", w.rotation());
    }
}
