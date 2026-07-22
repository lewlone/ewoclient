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

use serde_json::Value;

use crate::mobs::{Fold, Model, ModelBuilder, STATIC_PART};

/// Baseline that maps the inverted Y back into our y-down model space (feet
/// ≈ +24). Tuned so a CEM model sits on the ground like the built-ins.
const BASE_Y: f32 = 24.0;

/// Parse a `.jem` JSON string into a `Model`. `tex_w/tex_h` come from the
/// `.jem`'s `textureSize` (box-UVs are in those texels; the atlas
/// normalization in `entities` expects the mob's own texture pixels).
pub fn model_from_jem(jem: &str) -> Result<Model, String> {
    let root: Value = serde_json::from_str(jem).map_err(|e| format!("jem parse: {e}"))?;
    let models = root
        .get("models")
        .and_then(|m| m.as_array())
        .ok_or("jem: no models array")?;
    let mut b = ModelBuilder::new();
    let mut boxes = 0usize;
    for part in models {
        // A top-level part's `translate` is its rotation *pivot* (used by
        // animations), NOT static positioning — its boxes sit at their raw
        // coordinates. Only submodel translates accumulate. So each
        // top-level part starts from a zero offset regardless of its own
        // translate.
        walk(&mut b, part, [0.0; 3], true, &mut boxes);
    }
    if boxes == 0 {
        return Err("jem: no boxes (geometry only in referenced .jpm?)".into());
    }
    // CEM boxes are already in mob texture px; scale is the standard 1/16.
    Ok(b.finish(1.0))
}

/// Recurse a part/submodel: submodel `translate`s accumulate down the tree;
/// a top-level part's translate is skipped (it's the rotation pivot, not
/// static position — see `model_from_jem`). Emit each box, then descend.
fn walk(b: &mut ModelBuilder, node: &Value, parent_off: [f32; 3], top_level: bool, boxes: &mut usize) {
    let off = if top_level {
        parent_off // ignore a top-level part's own translate (= pivot)
    } else {
        let t = vec3(node.get("translate")).unwrap_or([0.0; 3]);
        [parent_off[0] + t[0], parent_off[1] + t[1], parent_off[2] + t[2]]
    };
    if let Some(arr) = node.get("boxes").and_then(|v| v.as_array()) {
        for bx in arr {
            emit_box(b, bx, off);
            *boxes += 1;
        }
    }
    if let Some(subs) = node.get("submodels").and_then(|v| v.as_array()) {
        for sub in subs {
            walk(b, sub, off, false, boxes);
        }
    }
}

/// Emit one CEM box. `coordinates` = [x,y,z, dx,dy,dz]; `textureOffset` =
/// box-UV origin; `sizeAdd` = uniform inflate. Vertices land in our y-down
/// model space via `to_model` (a `Fold` carries the per-vertex map into the
/// existing `cube_f` box-UV emitter).
fn emit_box(b: &mut ModelBuilder, bx: &Value, off: [f32; 3]) {
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
    // The box is built in JEM local space, min shifted by the accumulated
    // `translate`. The JEM→model map — negate X and Y (`invertAxis:"xy"`),
    // fold Y about BASE_Y, Z through — is exactly a 180° Z-rotation plus a
    // +BASE_Y translate, which `cube_f`'s Fold applies per-vertex (and
    // rotates the normal / re-derives shade for free).
    let jmin = [min[0] + off[0], min[1] + off[1], min[2] + off[2]];
    let to_model = Fold::rot([0.0, 0.0, std::f32::consts::PI], [0.0, BASE_Y, 0.0]);
    b.cube_f(STATIC_PART, 0, uv, jmin, dims, grow, false, &[to_model]);
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

    fn y_span(m: &Model) -> (f32, f32) {
        m.quads
            .iter()
            .flat_map(|q| q.pos.iter().map(|p| p[1]))
            .fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(y), b.max(y)))
    }

    /// The vanilla creeper body: a top-level part whose `translate` is the
    /// rotation pivot (IGNORED for static geometry). Box y[6,18] folds about
    /// BASE_Y(24) → model y[6,18], reproducing the vanilla creeper body
    /// exactly — proves invertAxis + the top-level-translate rule.
    #[test]
    fn top_level_translate_is_the_pivot_not_position() {
        let jem = r#"{"textureSize":[64,32],"models":[
            {"part":"body","translate":[0,-7,0],"invertAxis":"xy",
             "boxes":[{"coordinates":[-4,6,-2,8,12,4],"textureOffset":[16,16]}]}]}"#;
        let m = model_from_jem(jem).unwrap();
        assert_eq!(m.quads.len(), 6, "one box → six faces");
        let (lo, hi) = y_span(&m);
        assert!((lo - 6.0).abs() < 0.01 && (hi - 18.0).abs() < 0.01, "y span {lo}..{hi}");
    }

    /// A submodel's `translate` DOES accumulate (positions nested geometry).
    /// The creeper head cube (head2 under body, translate +18, box y[0,8])
    /// lands at model y[-2,6] — the vanilla creeper head.
    #[test]
    fn submodel_translate_accumulates() {
        let jem = r#"{"models":[
            {"part":"body","translate":[0,-7,0],"invertAxis":"xy","submodels":[
              {"id":"head2","translate":[0,18,0],"invertAxis":"xy",
               "boxes":[{"coordinates":[-4,0,-4,8,8,8],"textureOffset":[0,0]}]}]}]}"#;
        let m = model_from_jem(jem).unwrap();
        let (lo, hi) = y_span(&m);
        assert!((lo + 2.0).abs() < 0.01 && (hi - 6.0).abs() < 0.01, "head y span {lo}..{hi}");
    }

    #[test]
    fn empty_or_boxless_model_errors() {
        assert!(model_from_jem("{}").is_err());
        assert!(model_from_jem(r#"{"models":[{"part":"root","model":"x.jpm"}]}"#).is_err());
    }
}
