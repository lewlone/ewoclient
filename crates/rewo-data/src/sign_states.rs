//! Sign block states → the text transform `AbstractSignRenderer` pushes (M25e).
//!
//! A sign's *board* is an ordinary block model, which Rewo has drawn since M2.
//! What was missing is the text, and vanilla places it with a transform per
//! face:
//!
//! ```text
//! StandingSignRenderer.textTransformation(attachment, angle, isFrontText):
//!   M = translate(0.5, 0.5, 0.5) · YP(-angle)
//!       [· translate(0, -0.3125, -0.4375)   if attachment == WALL]
//!       [· YP(180)                          if !isFrontText]
//!       · translate(0, 0.33333334, 0.046666667)
//!       · scale(0.010416667, -0.010416667, 0.010416667)
//! ```
//!
//! The negative y in that scale is the flip from the font's y-down layout to
//! the world's y-up — dropping it renders every sign's text upside down while
//! leaving it in exactly the right place, which is a hard bug to see.
//!
//! `0.010416667` is `1/96`: the font cell is 8 px and vanilla wants a line to
//! occupy 1/12 of a block, so a whole 4-line sign fits the board.

use std::collections::HashMap;
use std::path::Path;

use crate::be_transform::{mul, rot_y, scale, translation, Affine};
use crate::read_json_file;

/// `SignBlockEntity.getTextLineHeight()` — 10 for a plain sign.
pub const SIGN_LINE_HEIGHT: i32 = 10;
/// `HangingSignBlockEntity.getTextLineHeight()` — 9.
pub const HANGING_LINE_HEIGHT: i32 = 9;
/// `SignBlockEntity.getMaxTextLineWidth()`.
pub const SIGN_MAX_WIDTH: i32 = 90;

/// `StandingSignRenderer.TEXT_OFFSET`.
const TEXT_OFFSET: [f32; 3] = [0.0, 0.333_333_34, 0.046_666_667];
/// The font-pixel → block scale in that transform.
const TEXT_SCALE: f32 = 0.010_416_667;
/// The wall sign's extra drop, so the text sits on the plaque rather than
/// where a standing sign's would be.
const WALL_OFFSET: [f32; 3] = [0.0, -0.3125, -0.4375];

/// How a sign is attached, which decides whether the wall drop applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignAttachment {
    /// A standing / hanging sign, rotated by its 16-step `rotation`.
    Ground,
    /// A wall sign, rotated by its `facing`.
    Wall,
}

/// One sign block state, as the renderer needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SignState {
    pub attachment: SignAttachment,
    /// The Y angle in degrees — `rotation * 22.5` for a ground sign,
    /// `facing.toYRot()` for a wall one.
    pub angle: f32,
    /// `getTextLineHeight()`, in font pixels.
    pub line_height: i32,
}

impl SignState {
    /// The front or back text transform.
    pub fn text_transform(self, front: bool) -> Affine {
        let mut m = mul(&translation(0.5, 0.5, 0.5), &rot_y(-self.angle));
        if self.attachment == SignAttachment::Wall {
            m = mul(&m, &translation(WALL_OFFSET[0], WALL_OFFSET[1], WALL_OFFSET[2]));
        }
        if !front {
            m = mul(&m, &rot_y(180.0));
        }
        m = mul(
            &m,
            &translation(TEXT_OFFSET[0], TEXT_OFFSET[1], TEXT_OFFSET[2]),
        );
        mul(&m, &scale(TEXT_SCALE, -TEXT_SCALE, TEXT_SCALE))
    }

    /// The baseline y of line `i`, in font pixels.
    ///
    /// `AbstractSignRenderer`: `i * textLineHeight - signMidpoint`, where
    /// `signMidpoint = 4 * textLineHeight / 2` — **integer** arithmetic, so a
    /// line height of 9 gives 18, not 18.0 rounded from 4*9/2.
    pub fn line_y(self, i: i32) -> f32 {
        let midpoint = 4 * self.line_height / 2;
        (i * self.line_height - midpoint) as f32
    }
}

/// State id → sign state, for every sign block state.
#[derive(Default)]
pub struct SignStates {
    by_state: HashMap<u32, SignState>,
}

impl SignStates {
    /// Read the sign states out of `blocks.json`.
    ///
    /// Every block whose name ends in `_sign` is one of the four shapes: a
    /// standing sign (`rotation`), a wall sign (`facing`), a hanging sign
    /// (`rotation`) or a wall hanging sign (`facing`). The property present
    /// says which, so no name list is needed and a new wood type is picked up
    /// for free.
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let obj = json.as_object().ok_or("blocks.json: root is not an object")?;
        let mut by_state = HashMap::new();
        for (block, def) in obj {
            let short = block.trim_start_matches("minecraft:");
            if !short.ends_with("_sign") {
                continue;
            }
            let hanging = short.contains("hanging_sign");
            let line_height = if hanging {
                HANGING_LINE_HEIGHT
            } else {
                SIGN_LINE_HEIGHT
            };
            let Some(states) = def.get("states").and_then(|s| s.as_array()) else {
                continue;
            };
            for st in states {
                let Some(id) = st.get("id").and_then(|i| i.as_u64()) else {
                    continue;
                };
                let props = st.get("properties").and_then(|p| p.as_object());
                let (attachment, angle) = if let Some(rot) = props
                    .and_then(|p| p.get("rotation"))
                    .and_then(|r| r.as_str())
                    .and_then(|r| r.parse::<i32>().ok())
                {
                    // 16 steps around the circle.
                    (SignAttachment::Ground, rot as f32 * 360.0 / 16.0)
                } else if let Some(facing) = props
                    .and_then(|p| p.get("facing"))
                    .and_then(|f| f.as_str())
                    .and_then(crate::chest_states::ChestFacing::from_name)
                {
                    (SignAttachment::Wall, facing.to_y_rot())
                } else {
                    return Err(format!(
                        "blocks.json: sign state {id} of {block} has neither a \
                         rotation nor a horizontal facing"
                    ));
                };
                by_state.insert(
                    id as u32,
                    SignState {
                        attachment,
                        angle,
                        line_height,
                    },
                );
            }
        }
        if by_state.is_empty() {
            return Err("blocks.json: no sign states found".into());
        }
        log::info!("rewo-data: {} sign block state(s)", by_state.len());
        Ok(Self { by_state })
    }

    pub fn get(&self, state_id: u32) -> Option<SignState> {
        self.by_state.get(&state_id).copied()
    }

    pub fn len(&self) -> usize {
        self.by_state.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_state.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: &Affine, p: [f32; 3]) -> [f32; 3] {
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
        ]
    }

    const GROUND: SignState = SignState {
        attachment: SignAttachment::Ground,
        angle: 0.0,
        line_height: SIGN_LINE_HEIGHT,
    };

    #[test]
    fn the_y_scale_is_negative() {
        // Font space runs y-down; the world does not. A positive y here would
        // put the text in exactly the right place, upside down.
        let m = GROUND.text_transform(true);
        assert!(m[1][1] < 0.0, "y scale {} must be negative", m[1][1]);
    }

    #[test]
    fn the_four_lines_straddle_the_board_centre() {
        // `i * 10 - 20` for i in 0..4 → -20, -10, 0, 10 font px, which the
        // negative y scale turns into descending world heights.
        let ys: Vec<f32> = (0..4).map(|i| GROUND.line_y(i)).collect();
        assert_eq!(ys, vec![-20.0, -10.0, 0.0, 10.0]);
        let m = GROUND.text_transform(true);
        let world: Vec<f32> = ys.iter().map(|&y| apply(&m, [0.0, y, 0.0])[1]).collect();
        assert!(
            world.windows(2).all(|w| w[0] > w[1]),
            "line 0 must sit highest: {world:?}"
        );
    }

    #[test]
    fn the_text_sits_just_off_the_board_face() {
        // TEXT_OFFSET's z is 0.0466 — a hair proud of the board, so the glyphs
        // do not z-fight the plank they sit on.
        let m = GROUND.text_transform(true);
        let p = apply(&m, [0.0, 0.0, 0.0]);
        assert!((p[2] - (0.5 + 0.046_666_667)).abs() < 1e-5, "{p:?}");
    }

    #[test]
    fn the_back_text_faces_the_other_way() {
        let f = apply(&GROUND.text_transform(true), [0.0, 0.0, 0.0]);
        let b = apply(&GROUND.text_transform(false), [0.0, 0.0, 0.0]);
        assert!(f[2] > 0.5 && b[2] < 0.5, "front {f:?} back {b:?}");
    }

    #[test]
    fn a_wall_sign_drops_and_pulls_back() {
        let wall = SignState {
            attachment: SignAttachment::Wall,
            ..GROUND
        };
        let g = apply(&GROUND.text_transform(true), [0.0, 0.0, 0.0]);
        let w = apply(&wall.text_transform(true), [0.0, 0.0, 0.0]);
        assert!(w[1] < g[1], "wall text sits lower: {w:?} vs {g:?}");
        assert!(w[2] < g[2], "and further back");
    }

    #[test]
    fn a_hanging_sign_uses_the_shorter_line_height() {
        let h = SignState {
            line_height: HANGING_LINE_HEIGHT,
            ..GROUND
        };
        // `4 * 9 / 2` is 18 by integer division, so the lines are -18..9.
        assert_eq!(
            (0..4).map(|i| h.line_y(i)).collect::<Vec<_>>(),
            vec![-18.0, -9.0, 0.0, 9.0]
        );
    }
}
