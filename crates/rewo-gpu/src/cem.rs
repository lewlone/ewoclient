//! OptiFine CEM (`.jem`) → [`mobs::Model`] parser (REWO_PLAN §12 M9a).
//!
//! Parses the static geometry of a resource-pack entity model into the same
//! `Model` IR the built-in mobs use, so a pack can override a mob's shape.
//! Animations (the `_animations.jpm` expression language) are M9c and not
//! read here; the `"model"` jpm reference is ignored for geometry (FA's are
//! pure animation containers).
//!
//! ## OptiFine coordinate conventions (the load-bearing part)
//!
//! A `.jem` is a tree of parts/submodels, each with a `translate`, an
//! `invertAxis` (default `"xy"`), and axis-aligned `boxes`. The authoring
//! frame is Y-up with X/Y inverted relative to Minecraft's model space;
//! `translate`s accumulate down the tree, boxes are relative to the
//! accumulated offset, and the whole thing maps into our Y-down model space
//! (feet near +24) by negating the inverted axes about a global baseline.
//! Calibrated against the vanilla creeper/pig/cow (Fresh Animations
//! replicates vanilla geometry): see `to_model` for the exact map.

use std::collections::HashMap;

use indexmap::IndexMap;
use serde_json::Value;

use crate::cem_anim::{AnimContext, AnimProgram, Channel, Program, Slots, Target};
use crate::mobs::{Fold, Model, ModelBuilder};

/// Baseline that maps the inverted Y back into our y-down model space (feet
/// ≈ +24). Tuned so a CEM model sits on the ground like the built-ins.
const BASE_Y: f32 = 24.0;

/// JEM space → our y-down model space, for a POINT (pivot): negate X/Y
/// (`invertAxis:"xy"`), fold Y about BASE_Y, Z through.
fn to_model_pt(v: [f32; 3]) -> [f32; 3] {
    [-v[0], BASE_Y - v[1], v[2]]
}

/// Parse a `.jem` JSON string into a `Model`. Every node of the part/submodel
/// tree becomes a **named bone** — top-level `part`s by their `part` name,
/// submodels by their `id` — so the animation program can drive nested detail
/// (FA blinks the eyes, turns the head, articulates the feet, all of which are
/// submodels). Bones are emitted in tree pre-order (parent before child), which
/// is exactly the order `part_transforms` composes them in. `jpms` resolves the
/// `_animations.jpm` referenced by the root part (M9c).
///
/// Pivots (the load-bearing asymmetry): a **top-level** part's `translate` is
/// the negated rotation pivot (`pivot = to_model(−translate)`, vanilla-exact).
/// A **submodel**'s `translate` is a *position* that accumulates, so its pivot
/// sits at that accumulated position (`pivot = to_model(boxOff)`) — e.g. the
/// creeper head submodel pivots at the neck. Box rest positions are identical
/// either way; only the per-bone rotation pivot differs.
pub fn model_from_jem(jem: &str, jpms: &HashMap<String, String>) -> Result<Model, String> {
    let root: Value = serde_json::from_str(jem).map_err(|e| format!("jem parse: {e}"))?;
    let models = root
        .get("models")
        .and_then(|m| m.as_array())
        .ok_or("jem: no models array")?;
    let mut b = ModelBuilder::new();
    let mut boxes = 0usize;
    // Bone name → part index, for animation-target resolution.
    let mut bones: HashMap<String, usize> = HashMap::new();
    // Per-bone own JEM translate (index 0 = the static root). Threaded to the
    // animation runtime so translation channels *replace* rather than add.
    let mut translates: Vec<[f32; 3]> = vec![[0.0; 3]];
    let mut anim_ref: Option<String> = None;
    for part in models {
        // The root part usually carries only the `"model"` animation ref.
        if let Some(m) = part.get("model").and_then(|v| v.as_str()) {
            anim_ref.get_or_insert_with(|| m.to_string());
        }
        let t = vec3(part.get("translate")).unwrap_or([0.0; 3]);
        add_node(&mut b, part, None, [0.0; 3], t, t, t, true, &mut bones, &mut translates, &mut boxes);
    }
    if boxes == 0 {
        return Err("jem: no boxes (geometry only in referenced .jpm?)".into());
    }
    let mut model = b.finish(1.0);
    debug_assert_eq!(translates.len(), model.parts.len(), "translate baseline per bone");
    model.cem_translate = translates;
    // Attach the animation program if the pack ships one for this mob.
    if let Some(name) = anim_ref {
        if let Some(src) = jpms.get(&name) {
            match parse_anim(src, &bones) {
                Ok(prog) => model.cem = Some(prog),
                Err(e) => log::warn!("cem: {name} animations skipped: {e}"),
            }
        }
    }
    Ok(model)
}

/// Recurse the part/submodel tree, creating one bone per node. `acc_t` is the
/// accumulated JEM translate down to (and including) this node; `top_t` is the
/// top-level ancestor's translate (excluded from box placement — it's the
/// top-level pivot). `parent_pivot` is the parent bone's *absolute* pivot, so a
/// bone stores its pivot relative to its parent for the compose step.
#[allow(clippy::too_many_arguments)]
fn add_node(
    b: &mut ModelBuilder,
    node: &Value,
    parent_bone: Option<usize>,
    parent_pivot: [f32; 3],
    acc_t: [f32; 3],
    own_t: [f32; 3],
    top_t: [f32; 3],
    is_top: bool,
    bones: &mut HashMap<String, usize>,
    translates: &mut Vec<[f32; 3]>,
    boxes: &mut usize,
) {
    // Box offset: submodel translates accumulate; the top-level translate is
    // the pivot, so it's removed (`acc_t − top_t` == 0 for the top level).
    let box_off = [acc_t[0] - top_t[0], acc_t[1] - top_t[1], acc_t[2] - top_t[2]];
    // Absolute pivot: top-level = −translate; submodel = its accumulated
    // position (`box_off`). Both mapped to model space by `to_model_pt`.
    let pivot_abs = if is_top {
        to_model_pt([-acc_t[0], -acc_t[1], -acc_t[2]])
    } else {
        to_model_pt(box_off)
    };
    let rel_pivot = [
        pivot_abs[0] - parent_pivot[0],
        pivot_abs[1] - parent_pivot[1],
        pivot_abs[2] - parent_pivot[2],
    ];
    let bone = b.cem_part(rel_pivot, parent_bone);
    debug_assert_eq!(translates.len(), bone, "translate vec tracks bone index");
    // Rest baseline for the replace-translation semantics: FA re-specifies a
    // top-level part's *world pivot* (so baseline = invertAxis of the pivot)
    // but a submodel's *own relative translate*. `eval_program` subtracts this
    // so a rest re-spec nets zero; only the sway remains.
    let baseline = if is_top {
        [-pivot_abs[0], -pivot_abs[1], pivot_abs[2]]
    } else {
        own_t
    };
    translates.push(baseline);
    // Register by name: top-level `part`, submodel `id`.
    let key = if is_top { "part" } else { "id" };
    if let Some(name) = node.get(key).and_then(|v| v.as_str()) {
        bones.entry(name.to_string()).or_insert(bone);
    }
    if let Some(arr) = node.get("boxes").and_then(|v| v.as_array()) {
        for bx in arr {
            emit_box(b, bx, box_off, bone, pivot_abs);
            *boxes += 1;
        }
    }
    if let Some(subs) = node.get("submodels").and_then(|v| v.as_array()) {
        for sub in subs {
            let st = vec3(sub.get("translate")).unwrap_or([0.0; 3]);
            let child_acc = [acc_t[0] + st[0], acc_t[1] + st[1], acc_t[2] + st[2]];
            add_node(b, sub, Some(bone), pivot_abs, child_acc, st, top_t, false, bones, translates, boxes);
        }
    }
}

/// Emit one CEM box onto bone `part`, pivot-relative. The JEM→model map
/// (180° Z-rotation + BASE_Y fold) then a −pivot shift are two `Fold`s the
/// `cube_f` box-UV emitter applies per-vertex.
fn emit_box(b: &mut ModelBuilder, bx: &Value, off: [f32; 3], part: usize, pivot: [f32; 3]) {
    let Some(c) = bx.get("coordinates").and_then(|v| v.as_array()) else { return };
    if c.len() < 6 {
        return;
    }
    let f = |i: usize| c[i].as_f64().unwrap_or(0.0) as f32;
    let min = [f(0), f(1), f(2)];
    let dims = [f(3), f(4), f(5)];
    let grow = bx.get("sizeAdd").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    let uv = bx
        .get("textureOffset")
        .and_then(|v| v.as_array())
        .map(|a| (a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32))
        // No box-UV offset → 0; per-face `uv*` rects (below) override anyway.
        .unwrap_or((0.0, 0.0));
    // Per-face UV rects (OptiFine `uvNorth`/… — texture-pixel [u0,v0,u1,v1]).
    // FA uses these for hand-placed detail: creeper/zombie/pig eyes (uvNorth
    // on flat plane boxes) and the pig snout (all sides). Order must match
    // `mobs::face_slot`: [North, South, East, West, Up, Down].
    let face_uv = [
        face_rect(bx, "uvNorth"),
        face_rect(bx, "uvSouth"),
        face_rect(bx, "uvEast"),
        face_rect(bx, "uvWest"),
        face_rect(bx, "uvUp"),
        face_rect(bx, "uvDown"),
    ];
    let jmin = [min[0] + off[0], min[1] + off[1], min[2] + off[2]];
    let to_model = Fold::rot([0.0, 0.0, std::f32::consts::PI], [0.0, BASE_Y, 0.0]);
    let unpivot = Fold::at([-pivot[0], -pivot[1], -pivot[2]]);
    b.cube_f_faceuv(part, 0, uv, jmin, dims, grow, false, &[to_model, unpivot], &face_uv);
}

/// Read a `uv<Face>` key as a texture-pixel rect `[u0,v0,u1,v1]`, or `None`
/// if absent/malformed (→ box-UV fallback for that face).
fn face_rect(bx: &Value, key: &str) -> Option<[f32; 4]> {
    let a = bx.get(key)?.as_array()?;
    if a.len() < 4 {
        return None;
    }
    Some([
        a[0].as_f64()? as f32,
        a[1].as_f64()? as f32,
        a[2].as_f64()? as f32,
        a[3].as_f64()? as f32,
    ])
}

/// Parse an `_animations.jpm`'s `animations` array into an ordered program.
/// Each entry is a map `{target: expr}`; keys are `var.x`/`varb.x` (a user
/// slot) or `bone.channel` (e.g. `head.rx`). Bones not in the model + non-
/// rotation/translation channels are skipped.
///
/// The assignment maps are deserialized into [`IndexMap`]s so their keys keep
/// **file order** — OptiFine evaluates a block top-to-bottom and a later
/// expression may read a bone channel an earlier one assigned (FA mirrors the
/// left eye off the right: `"l_eye_white.sy": "r_eye_white.sy"`). Going through
/// `serde_json::Value` would sort the keys (its `Map` is a `BTreeMap`) and
/// silently break those reads.
fn parse_anim(src: &str, bones: &HashMap<String, usize>) -> Result<AnimProgram, String> {
    #[derive(serde::Deserialize)]
    struct JpmAnims {
        #[serde(default)]
        animations: Vec<IndexMap<String, Value>>,
    }
    let file: JpmAnims = serde_json::from_str(src).map_err(|e| format!("jpm parse: {e}"))?;
    let mut slots = Slots::default();
    let mut steps: Vec<(Target, Program)> = Vec::new();
    for block in &file.animations {
        for (key, val) in block {
            let Some(expr) = val.as_str() else { continue };
            let prog = match Program::compile(expr, &mut slots) {
                Ok(p) => p,
                Err(e) => {
                    log::debug!("cem: skip {key}: {e}");
                    continue;
                }
            };
            let target = if key.starts_with("var.") || key.starts_with("varb.") {
                Target::Var(slots.get_or_add(key))
            } else if let Some((bone, chan)) = key.rsplit_once('.') {
                match (bones.get(bone), Channel::parse(chan)) {
                    (Some(&part), Some(channel)) => {
                        // Intern a slot for `bone.channel` so its value is
                        // readable by later expressions (mirror/derive-scale).
                        let slot = slots.get_or_add(key);
                        Target::Bone { part: part as u16, channel, slot }
                    }
                    _ => continue, // unknown bone or channel → skip
                }
            } else {
                continue;
            };
            steps.push((target, prog));
        }
    }
    Ok(AnimProgram { steps, slot_count: slots.len() })
}

/// Evaluate a mob's animation program for one frame. Returns, per part:
/// - rotation/translation deltas `[rx, ry, rz, tx, ty, tz]` (radians / model
///   px), summed onto the base pose in `part_transforms`;
/// - a scale factor `[sx, sy, sz]` (default `1.0`), applied about the pivot.
///
/// `ctx` must already carry this frame's built-in inputs; user + bone-channel
/// slots are sized here and filled in file order, so a later expression reads
/// the value an earlier one assigned (`ctx.user` doubles as the read store).
pub fn eval_program(
    prog: &AnimProgram,
    ctx: &mut AnimContext,
    part_count: usize,
    translates: &[[f32; 3]],
) -> (Vec<[f32; 6]>, Vec<[f32; 3]>) {
    ctx.user.clear();
    ctx.user.resize(prog.slot_count, 0.0);
    let mut out = vec![[0.0f32; 6]; part_count];
    let mut scale = vec![[1.0f32; 3]; part_count];
    for (target, program) in &prog.steps {
        let v = program.eval(ctx);
        match target {
            Target::Var(slot) => {
                if let Some(u) = ctx.user.get_mut(*slot) {
                    *u = v;
                }
            }
            Target::Bone { part, channel, slot } => {
                // Publish the raw (OptiFine-convention) value for later reads,
                // BEFORE the render-space sign flip below.
                if let Some(u) = ctx.user.get_mut(*slot) {
                    *u = v;
                }
                // The model is baked through a 180° Z-rotation
                // (invertAxis:"xy" → x→−x, y→−y). Conjugating the animation by
                // it negates the X and Y rotation angles + translations; Z
                // passes through. Scale magnitudes are rotation-invariant.
                let base = translates.get(*part as usize).copied().unwrap_or([0.0; 3]);
                match channel {
                    // Rotation is additive-from-zero (CEM base pose is
                    // identity), so `+=` == assign for the common single-write.
                    Channel::Rx => add6(&mut out, *part, 0, -v),
                    Channel::Ry => add6(&mut out, *part, 1, -v),
                    Channel::Rz => add6(&mut out, *part, 2, v),
                    // Translation *replaces* the bone's translate (FA authors
                    // rest + sway). Subtract the own-translate baseline so a
                    // rest re-specification nets to zero — leaving only sway.
                    Channel::Tx => set_t(&mut out, *part, 3, -v - base[0]),
                    Channel::Ty => set_t(&mut out, *part, 4, -v - base[1]),
                    Channel::Tz => set_t(&mut out, *part, 5, v - base[2]),
                    // Scale is an assignment (last write wins), not additive.
                    Channel::Sx => set_scale(&mut scale, *part, 0, v),
                    Channel::Sy => set_scale(&mut scale, *part, 1, v),
                    Channel::Sz => set_scale(&mut scale, *part, 2, v),
                }
            }
        }
    }
    (out, scale)
}

/// Add `v` into the `i`-th slot of `part` (rotation deltas accumulate onto the
/// identity base pose).
fn add6(out: &mut [[f32; 6]], part: u16, i: usize, v: f32) {
    if let Some(d) = out.get_mut(part as usize) {
        d[i] += v;
    }
}

/// Set the `i`-th slot of `part` (translation is an assignment — last write
/// wins — with the rest baseline already removed by the caller).
fn set_t(out: &mut [[f32; 6]], part: u16, i: usize, v: f32) {
    if let Some(d) = out.get_mut(part as usize) {
        d[i] = v;
    }
}

/// Set the `i`-th scale axis of `part` (assignment — a doubly-driven axis
/// takes the last write, matching OptiFine).
fn set_scale(scale: &mut [[f32; 3]], part: u16, i: usize, v: f32) {
    if let Some(s) = scale.get_mut(part as usize) {
        s[i] = v;
    }
}

fn vec3(v: Option<&Value>) -> Option<[f32; 3]> {
    let a = v?.as_array()?;
    if a.len() < 3 {
        return None;
    }
    Some([
        a[0].as_f64()? as f32,
        a[1].as_f64()? as f32,
        a[2].as_f64()? as f32,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Absolute model-space pivot-y of a bone (pivots are stored relative to
    /// the parent now that submodels are their own bones — sum up the chain).
    fn abs_pivot_y(m: &Model, part: usize) -> f32 {
        let p = &m.parts[part];
        let base = p.parent.map(|par| abs_pivot_y(m, par as usize)).unwrap_or(0.0);
        base + p.pivot[1]
    }

    /// Absolute model-space y span (quads are stored pivot-relative; the
    /// absolute pivot is added back at render, so add it here).
    fn y_span(m: &Model) -> (f32, f32) {
        m.quads
            .iter()
            .flat_map(|q| {
                let pv = abs_pivot_y(m, q.part as usize);
                q.pos.iter().map(move |p| p[1] + pv)
            })
            .fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(y), b.max(y)))
    }

    fn no_jpms() -> HashMap<String, String> {
        HashMap::new()
    }

    /// The vanilla creeper body. A top-level part's `translate` [0,-7,0] is
    /// its rotation pivot → pivot [0,31,0] (to_model(-t)); box y[6,18] lands
    /// at absolute model y[6,18], reproducing the vanilla creeper body.
    #[test]
    fn top_level_translate_is_the_pivot_not_position() {
        let jem = r#"{"textureSize":[64,32],"models":[
            {"part":"body","translate":[0,-7,0],"invertAxis":"xy",
             "boxes":[{"coordinates":[-4,6,-2,8,12,4],"textureOffset":[16,16]}]}]}"#;
        let m = model_from_jem(jem, &no_jpms()).unwrap();
        assert_eq!(m.quads.len(), 6, "one box → six faces");
        let (lo, hi) = y_span(&m);
        assert!((lo - 6.0).abs() < 0.01 && (hi - 18.0).abs() < 0.01, "y span {lo}..{hi}");
    }

    /// A submodel's `translate` accumulates (positions nested geometry). The
    /// creeper head cube (head2 under body, translate +18, box y[0,8]) lands
    /// at absolute model y[-2,6] — the vanilla creeper head.
    #[test]
    fn submodel_translate_accumulates() {
        let jem = r#"{"models":[
            {"part":"body","translate":[0,-7,0],"invertAxis":"xy","submodels":[
              {"id":"head2","translate":[0,18,0],"invertAxis":"xy",
               "boxes":[{"coordinates":[-4,0,-4,8,8,8],"textureOffset":[0,0]}]}]}]}"#;
        let m = model_from_jem(jem, &no_jpms()).unwrap();
        let (lo, hi) = y_span(&m);
        assert!((lo + 2.0).abs() < 0.01 && (hi - 6.0).abs() < 0.01, "head y span {lo}..{hi}");
    }

    /// The pivot equals `to_model(−translate)` — the vanilla zombie leg
    /// (translate [1.9,-12,0]) pivots at [1.9,12,0]; the body [0,-24,0] at
    /// [0,0,0]. This is what makes the leg swing about the hip.
    #[test]
    fn pivot_is_negated_translate() {
        let jem = r#"{"models":[
            {"part":"left_leg","translate":[1.9,-12,0],"invertAxis":"xy",
             "boxes":[{"coordinates":[-2,0,-2,4,12,4],"textureOffset":[0,16]}]},
            {"part":"body","translate":[0,-24,0],"invertAxis":"xy",
             "boxes":[{"coordinates":[-4,12,-2,8,12,4],"textureOffset":[16,16]}]}]}"#;
        let m = model_from_jem(jem, &no_jpms()).unwrap();
        let leg = m.parts[1].pivot; // part 0 is the static root
        let body = m.parts[2].pivot;
        assert!((leg[0] - 1.9).abs() < 0.01 && (leg[1] - 12.0).abs() < 0.01, "leg pivot {leg:?}");
        assert!(body[1].abs() < 0.01, "body pivot {body:?}");
    }

    /// A submodel is its own bone, parented to its part, and pivots at its
    /// *accumulated position* (not the top-level −translate rule). The creeper
    /// head is a submodel of `body`; it must pivot at the neck (model y=6) so
    /// head-look rotates about the neck, and box rest positions are unchanged.
    #[test]
    fn submodel_is_a_child_bone_pivoting_at_its_position() {
        let jem = r#"{"models":[
            {"part":"body","translate":[0,-7,0],"invertAxis":"xy","submodels":[
              {"id":"head2","translate":[0,18,0],
               "boxes":[{"coordinates":[-4,0,-4,8,8,8],"textureOffset":[0,0]}]}]}]}"#;
        let m = model_from_jem(jem, &no_jpms()).unwrap();
        // parts: 0 = static root, 1 = body (top-level), 2 = head2 (submodel).
        let body = m.parts[1].pivot;
        assert!((body[1] - 17.0).abs() < 0.01, "body pivot = to_model(−translate) = 17, got {body:?}");
        assert_eq!(m.parts[2].parent, Some(1), "head2 is a child of body");
        // head2 stores its pivot relative to body; absolute must be the neck.
        let head2_abs = [
            body[0] + m.parts[2].pivot[0],
            body[1] + m.parts[2].pivot[1],
            body[2] + m.parts[2].pivot[2],
        ];
        assert!((head2_abs[1] - 6.0).abs() < 0.01, "head submodel pivots at the neck y=6, got {head2_abs:?}");
        // Static geometry preserved: head box still spans model y[-2,6].
        let (lo, hi) = y_span(&m);
        assert!((lo + 2.0).abs() < 0.01 && (hi - 6.0).abs() < 0.01, "head y span {lo}..{hi}");
    }

    /// A referenced `_animations.jpm` parses into a bone-targeted program.
    #[test]
    fn parses_animation_program() {
        let jem = r#"{"models":[
            {"part":"root","model":"z_anim.jpm"},
            {"part":"left_leg","translate":[1.9,-12,0],
             "boxes":[{"coordinates":[-2,0,-2,4,12,4],"textureOffset":[0,16]}]}]}"#;
        let mut jpms = HashMap::new();
        jpms.insert(
            "z_anim.jpm".to_string(),
            r#"{"animations":[{"var.x":"limb_swing*2","left_leg.rx":"cos(var.x)"}]}"#.to_string(),
        );
        let m = model_from_jem(jem, &jpms).unwrap();
        let prog = m.cem.expect("animation attached");
        assert_eq!(prog.steps.len(), 2, "one var + one bone channel");
        // Two slots: `var.x` and the bone channel `left_leg.rx` (bone channels
        // now intern a slot so later expressions can read them back).
        assert_eq!(prog.slot_count, 2);
    }

    /// A per-face `uvNorth` rect overrides the box-UV unwrap for that face
    /// only, wound like the box-UV North face. The zombie/creeper eyes are
    /// flat plane boxes (a 0 dim) whose front (North) plane carries the eye
    /// texels — this is what makes them sample the right pixels.
    #[test]
    fn per_face_uv_north_overrides_box_uv() {
        let jem = r#"{"models":[
            {"part":"head","invertAxis":"xy","translate":[0,0,0],
             "boxes":[{"coordinates":[0,0,0,1,1,0],"uvNorth":[14,17,15,18]}]}]}"#;
        let m = model_from_jem(jem, &no_jpms()).unwrap();
        // Flat box (dim z=0): only the North + South planes have area.
        let north = m
            .quads
            .iter()
            .find(|q| q.facing == crate::mobs::Facing::North)
            .expect("north face emitted");
        // rect (a,b,c,e)=(14,17,15,18) → verts [(c,b),(a,b),(a,e),(c,e)].
        assert_eq!(north.uv, [[15.0, 17.0], [14.0, 17.0], [14.0, 18.0], [15.0, 18.0]]);
        // The South plane (no explicit uv) falls back to the box-UV unwrap
        // near the texture origin — proving the override is per-face, not
        // whole-box.
        let south = m
            .quads
            .iter()
            .find(|q| q.facing == crate::mobs::Facing::South)
            .expect("south face emitted");
        assert!(south.uv.iter().all(|c| c[0] < 5.0 && c[1] < 5.0), "box-UV fallback, got {:?}", south.uv);
        assert_ne!(south.uv, north.uv, "override must not leak to other faces");
    }

    /// Scale channels apply, bone channels are readable as variables, and the
    /// program keeps file order. FA blinks/dilates the eyes by scaling the eye
    /// boxes and mirrors the left eye off the right (`l.sy: "r.sy"`).
    #[test]
    fn scale_channels_bone_reads_and_file_order() {
        let jem = r#"{"models":[
            {"part":"root","model":"a.jpm"},
            {"part":"r_eye_white","translate":[0,-24,0],
             "boxes":[{"coordinates":[0,0,0,1,1,0],"uvNorth":[0,0,1,1]}]},
            {"part":"l_eye_white","translate":[0,-24,0],
             "boxes":[{"coordinates":[0,0,0,1,1,0],"uvNorth":[0,0,1,1]}]}]}"#;
        let mut jpms = HashMap::new();
        // File order: r assigned first, then l mirrors it. Alphabetically
        // `l_eye_white.sy` < `r_eye_white.sy`, so a sorted map would evaluate l
        // first and read r's slot as 0 — this pins the file-order fix.
        jpms.insert(
            "a.jpm".to_string(),
            r#"{"animations":[{
                "r_eye_white.sy":"3",
                "l_eye_white.sy":"r_eye_white.sy",
                "r_eye_white.rx":"0.25"
            }]}"#
                .to_string(),
        );
        let m = model_from_jem(jem, &jpms).unwrap();
        let prog = m.cem.as_ref().expect("animation attached");
        let mut ctx = crate::cem_anim::AnimContext::default();
        let (deltas, scale) = eval_program(prog, &mut ctx, m.parts.len(), &m.cem_translate);
        // parts: 0 = static root, 1 = "root" (anim holder), 2 = r_eye_white, 3 = l_eye_white.
        assert_eq!(scale[2][1], 3.0, "r_eye_white.sy = 3");
        assert_eq!(scale[3][1], 3.0, "l_eye_white.sy mirrors r — requires file order");
        assert_eq!(scale[2][0], 1.0, "unassigned scale axis stays 1");
        assert!((deltas[2][0] + 0.25).abs() < 1e-6, "rx negated by the 180°Z fold");
    }

    /// OptiFine translation channels *replace* a bone's translate; FA
    /// re-specifies rest (+ sway). A submodel whose anim re-specifies exactly
    /// its rest position must net to ZERO translation delta — adding it (the
    /// bug) flung the pig's head ~12 units off the body.
    #[test]
    fn translation_channels_replace_not_add() {
        let jem = r#"{"models":[
            {"part":"root","model":"a.jpm"},
            {"part":"body","translate":[0,-8,0],"submodels":[
              {"id":"head2","translate":[0,12,-6],
               "boxes":[{"coordinates":[-4,-4,-8,8,8,8],"textureOffset":[0,0]}]}]}]}"#;
        let mut jpms = HashMap::new();
        // ty=-12, tz=-6 == invertAxis of the jem translate [0,12,-6] (rest).
        jpms.insert(
            "a.jpm".to_string(),
            r#"{"animations":[{"head2.ty":"-12","head2.tz":"-6","head2.tx":"0"}]}"#.to_string(),
        );
        let m = model_from_jem(jem, &jpms).unwrap();
        let prog = m.cem.as_ref().expect("animation attached");
        let mut ctx = crate::cem_anim::AnimContext::default();
        let (deltas, _) = eval_program(prog, &mut ctx, m.parts.len(), &m.cem_translate);
        // parts: 0 static root, 1 "root" anim-holder, 2 body, 3 head2.
        let d = &deltas[3];
        assert!(
            d[3].abs() < 1e-4 && d[4].abs() < 1e-4 && d[5].abs() < 1e-4,
            "rest re-spec must net zero translation delta, got tx/ty/tz {:?}",
            &d[3..6]
        );
    }

    #[test]
    fn empty_or_boxless_model_errors() {
        assert!(model_from_jem("{}", &no_jpms()).is_err());
        assert!(model_from_jem(r#"{"models":[{"part":"root","model":"x.jpm"}]}"#, &no_jpms()).is_err());
    }
}
