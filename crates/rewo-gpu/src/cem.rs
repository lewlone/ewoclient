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

/// Parse a `.jem` JSON string into a `Model`. Each top-level part becomes a
/// named bone (pivot = `to_model(−translate)`, since OptiFine's `translate`
/// IS the negated rotation pivot); its boxes + submodel boxes attach to it,
/// pivot-relative, so the animation program can rotate the bone. `jpms`
/// resolves the `_animations.jpm` referenced by the root part (M9c).
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
    let mut anim_ref: Option<String> = None;
    for part in models {
        // The root part usually carries only the `"model"` animation ref.
        if let Some(m) = part.get("model").and_then(|v| v.as_str()) {
            anim_ref.get_or_insert_with(|| m.to_string());
        }
        // Pivot from the top-level translate (= −pivot in JEM).
        let t = vec3(part.get("translate")).unwrap_or([0.0; 3]);
        let pivot = to_model_pt([-t[0], -t[1], -t[2]]);
        let idx = b.cem_part(pivot);
        if let Some(name) = part.get("part").and_then(|v| v.as_str()) {
            bones.insert(name.to_string(), idx);
        }
        // Emit the part's own + submodel boxes on this bone, pivot-relative.
        walk(&mut b, part, [0.0; 3], true, idx, pivot, &mut boxes);
    }
    if boxes == 0 {
        return Err("jem: no boxes (geometry only in referenced .jpm?)".into());
    }
    let mut model = b.finish(1.0);
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

/// Recurse a part/submodel emitting boxes onto bone `part` (pivot-relative).
/// Submodel `translate`s accumulate; the top-level part's translate is the
/// pivot (handled by the caller) so it's skipped here.
#[allow(clippy::too_many_arguments)]
fn walk(
    b: &mut ModelBuilder,
    node: &Value,
    parent_off: [f32; 3],
    top_level: bool,
    part: usize,
    pivot: [f32; 3],
    boxes: &mut usize,
) {
    let off = if top_level {
        parent_off
    } else {
        let t = vec3(node.get("translate")).unwrap_or([0.0; 3]);
        [parent_off[0] + t[0], parent_off[1] + t[1], parent_off[2] + t[2]]
    };
    if let Some(arr) = node.get("boxes").and_then(|v| v.as_array()) {
        for bx in arr {
            emit_box(b, bx, off, part, pivot);
            *boxes += 1;
        }
    }
    if let Some(subs) = node.get("submodels").and_then(|v| v.as_array()) {
        for sub in subs {
            walk(b, sub, off, false, part, pivot, boxes);
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
        // Per-face uvNorth/… (creeper eyes) not yet handled → box-UV at 0.
        .unwrap_or((0.0, 0.0));
    let jmin = [min[0] + off[0], min[1] + off[1], min[2] + off[2]];
    let to_model = Fold::rot([0.0, 0.0, std::f32::consts::PI], [0.0, BASE_Y, 0.0]);
    let unpivot = Fold::at([-pivot[0], -pivot[1], -pivot[2]]);
    b.cube_f(part, 0, uv, jmin, dims, grow, false, &[to_model, unpivot]);
}

/// Parse an `_animations.jpm`'s `animations` array into an ordered program.
/// Each entry is a map `{target: expr}`; keys are `var.x`/`varb.x` (a user
/// slot) or `bone.channel` (e.g. `head.rx`). Bones not in the model + non-
/// rotation/translation channels are skipped.
fn parse_anim(src: &str, bones: &HashMap<String, usize>) -> Result<AnimProgram, String> {
    let root: Value = serde_json::from_str(src).map_err(|e| format!("jpm parse: {e}"))?;
    let arr = root
        .get("animations")
        .and_then(|v| v.as_array())
        .ok_or("jpm: no animations array")?;
    let mut slots = Slots::default();
    let mut steps: Vec<(Target, Program)> = Vec::new();
    for block in arr {
        let Some(map) = block.as_object() else { continue };
        for (key, val) in map {
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
                    (Some(&part), Some(channel)) => Target::Bone { part: part as u16, channel },
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

/// Evaluate a mob's animation program for one frame, returning per-part
/// channel deltas `[rx, ry, rz, tx, ty, tz]` (radians / model px). `ctx`
/// must already carry this frame's built-in inputs; user slots are sized +
/// filled here in file order.
pub fn eval_program(prog: &AnimProgram, ctx: &mut AnimContext, part_count: usize) -> Vec<[f32; 6]> {
    ctx.user.clear();
    ctx.user.resize(prog.slot_count, 0.0);
    let mut out = vec![[0.0f32; 6]; part_count];
    for (target, program) in &prog.steps {
        let v = program.eval(ctx);
        match target {
            Target::Var(slot) => {
                if let Some(u) = ctx.user.get_mut(*slot) {
                    *u = v;
                }
            }
            Target::Bone { part, channel } => {
                if let Some(d) = out.get_mut(*part as usize) {
                    // The model is baked through a 180° Z-rotation
                    // (invertAxis:"xy" → x→−x, y→−y). Conjugating the
                    // animation by it negates the X and Y rotation angles +
                    // translations; Z passes through.
                    let (i, s) = match channel {
                        Channel::Rx => (0, -1.0), Channel::Ry => (1, -1.0), Channel::Rz => (2, 1.0),
                        Channel::Tx => (3, -1.0), Channel::Ty => (4, -1.0), Channel::Tz => (5, 1.0),
                        // Scale channels not applied yet (M9c follow-up).
                        Channel::Sx | Channel::Sy | Channel::Sz => continue,
                    };
                    d[i] += v * s;
                }
            }
        }
    }
    out
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

    /// Absolute model-space y span (quads are stored pivot-relative; the
    /// pivot is added back at render, so add it here).
    fn y_span(m: &Model) -> (f32, f32) {
        m.quads
            .iter()
            .flat_map(|q| {
                let pv = m.parts[q.part as usize].pivot[1];
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
        assert_eq!(prog.slot_count, 1);
    }

    #[test]
    fn empty_or_boxless_model_errors() {
        assert!(model_from_jem("{}", &no_jpms()).is_err());
        assert!(model_from_jem(r#"{"models":[{"part":"root","model":"x.jpm"}]}"#, &no_jpms()).is_err());
    }
}
