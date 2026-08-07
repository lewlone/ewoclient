//! `blitNineSlicedSprite` as pure geometry (M104) — a sheet, a target rect and
//! a border in, a list of source→destination quads out.
//!
//! # Why this is not `rewo_gpu::screen::nine_slice`
//!
//! There is already a nine-slice in the repo, and it is deliberately left
//! alone. It belongs to the **screen** pass (M84/M85), writes `Vertex`
//! structs straight into that pass's buffer, and is graded only by
//! `statshot` and `serverlinkshot` — it has **no unit tests at all**. The
//! container pass this milestone needs it for speaks
//! [`rewo_gpu::container::PanelBlit`] instead, so there is nothing to share at
//! the emission end.
//!
//! What *is* shared is the arithmetic, and a second copy of arithmetic is a
//! second chance to drift (M90). So this is the arithmetic, on its own, with
//! tests — and moving the screen pass onto it is a follow-up that would move
//! pixels two pixel-gates currently pin, which is not this milestone's job.
//!
//! # The inner segments TILE
//!
//! `stretch_inner` defaults to `false` in `GuiSpriteScaling.NineSlice`'s codec,
//! so `blitNineSliceInnerSegment` falls through to `blitTiledSprite` and the
//! four edges and the centre repeat rather than stretching. 26.2 does that in
//! one quad with a shader-side tile size; Rewo's atlas is `CLAMP_TO_EDGE`, so a
//! tile is a quad and this emits the grid — the same choice `rewo_gpu::screen`
//! made, for the same reason.
//!
//! **On `recipe_book/overlay_recipe.png` the choice is unobservable**, which is
//! worth writing down so a green pixel gate is not read as having graded it:
//! that sprite's centre is one flat colour and each of its four edge bands is
//! constant along the axis it repeats on, so tiling and stretching produce
//! identical pixels. Tiled anyway, because it is what vanilla does and because
//! the next nine-sliced sprite need not be so obliging.
//!
//! # Ground truth (bundled 26.2 decompile)
//!
//! `net/minecraft/client/gui/GuiGraphicsExtractor.java` —
//! `blitNineSlicedSprite`, `blitNineSliceInnerSegment`, `blitTiledSprite`.

/// `GuiSpriteScaling.NineSlice.Border`, `{left, top, right, bottom}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Border {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Border {
    /// The one-number form an `.mcmeta` may use: `"border": 4`.
    pub const fn all(n: i32) -> Self {
        Self { left: n, top: n, right: n, bottom: n }
    }
}

/// One quad: where it goes, and which pixels of the sheet it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quad {
    pub dx: i32,
    pub dy: i32,
    pub w: i32,
    pub h: i32,
    pub sx: i32,
    pub sy: i32,
    pub sw: i32,
    pub sh: i32,
}

/// A tiled band: repeat `(sx, sy, tw, th)` across `(dx, dy, w, h)`, clipping
/// the last column and row.
fn tile(out: &mut Vec<Quad>, (dx, dy, w, h): (i32, i32, i32, i32), (sx, sy, tw, th): (i32, i32, i32, i32)) {
    // `blitTiledSprite`'s own guard. **Through [`quads`]' own five call sites it
    // is unreachable as a difference**, and a mutation deleting it survived the
    // whole suite — correctly. The `w <= 0` / `h <= 0` halves are already
    // enforced by the `while` conditions below, and the tile size is either the
    // band's own thickness (so `tw <= 0` implies `w <= 0`) or `isw`/`ish`, which
    // for a 32-px sheet with a border of at most 4 are never below 24. The one
    // thing it does buy is a hang: `tw == 0` with `w > 0` would spin on
    // `x += tw`. Kept because it is vanilla's, and because `tile` could gain a
    // caller that does not satisfy the argument above.
    if w <= 0 || h <= 0 || tw <= 0 || th <= 0 {
        return;
    }
    let mut y = 0;
    while y < h {
        let ch = th.min(h - y);
        let mut x = 0;
        while x < w {
            let cw = tw.min(w - x);
            out.push(Quad { dx: dx + x, dy: dy + y, w: cw, h: ch, sx, sy, sw: cw, sh: ch });
            x += tw;
        }
        y += th;
    }
}

/// `blitNineSlicedSprite(pipeline, sprite, nineSlice, x, y, width, height)`.
///
/// `dst` is `(x, y, width, height)` and `sheet` is the `.mcmeta`'s declared
/// `(width, height)` — **not** the PNG's, though for every sprite in the jar
/// they agree.
///
/// Vanilla's four-way fork on `width == sheet.w` / `height == sheet.h` is not
/// reproduced branch for branch. Those branches exist so an exactly-sized axis
/// can skip its corner splits, and the general case already produces the same
/// geometry there: with `height == sheet.h` the left edge's band is exactly one
/// 1:1 tile, so the corner, the edge and the other corner stack into the single
/// full-height column that branch draws. The **whole-sprite** case is kept,
/// because a 1:1 blit has to stay one quad rather than nine.
pub fn quads(dst: (i32, i32, i32, i32), sheet: (i32, i32), border: Border) -> Vec<Quad> {
    let (x, y, w, h) = dst;
    let (sw, sh) = sheet;
    if w == sw && h == sh {
        return vec![Quad { dx: x, dy: y, w, h, sx: 0, sy: 0, sw, sh }];
    }
    // `Math.min(border.left(), width / 2)` and its three siblings — a sprite
    // drawn narrower than its own border must not draw its corners twice.
    let l = border.left.min(w / 2).max(0);
    let r = border.right.min(w / 2).max(0);
    let t = border.top.min(h / 2).max(0);
    let b = border.bottom.min(h / 2).max(0);
    // The SOURCE bands keep the unclamped border, because the clamp is about
    // the destination overlapping itself, not about which texels a corner is.
    // Only the inner source band shrinks with the clamped values.
    let (mw, mh) = (w - l - r, h - t - b);
    let (isw, ish) = (sw - l - r, sh - t - b);
    let mut out = Vec::new();
    // Corners, 1:1.
    push(&mut out, (x, y, l, t), (0, 0));
    push(&mut out, (x + w - r, y, r, t), (sw - r, 0));
    push(&mut out, (x, y + h - b, l, b), (0, sh - b));
    push(&mut out, (x + w - r, y + h - b, r, b), (sw - r, sh - b));
    // Edges and centre, tiled.
    tile(&mut out, (x + l, y, mw, t), (l, 0, isw, t));
    tile(&mut out, (x + l, y + h - b, mw, b), (l, sh - b, isw, b));
    tile(&mut out, (x, y + t, l, mh), (0, t, l, ish));
    tile(&mut out, (x + w - r, y + t, r, mh), (sw - r, t, r, ish));
    tile(&mut out, (x + l, y + t, mw, mh), (l, t, isw, ish));
    out
}

fn push(out: &mut Vec<Quad>, (dx, dy, w, h): (i32, i32, i32, i32), (sx, sy): (i32, i32)) {
    if w > 0 && h > 0 {
        out.push(Quad { dx, dy, w, h, sx, sy, sw: w, sh: h });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The overlay's own sheet.
    const SHEET: (i32, i32) = (32, 32);
    const B: Border = Border::all(4);

    fn covered(qs: &[Quad], dst: (i32, i32, i32, i32)) {
        // Every quad is inside the target…
        for q in qs {
            assert!(q.w > 0 && q.h > 0, "no empty quads: {q:?}");
            assert!(q.dx >= dst.0 && q.dy >= dst.1, "{q:?} starts before {dst:?}");
            assert!(
                q.dx + q.w <= dst.0 + dst.2 && q.dy + q.h <= dst.1 + dst.3,
                "{q:?} spills out of {dst:?}"
            );
            assert_eq!((q.w, q.h), (q.sw, q.sh), "every quad is 1:1: {q:?}");
            assert!(
                q.sx >= 0 && q.sy >= 0 && q.sx + q.sw <= SHEET.0 && q.sy + q.sh <= SHEET.1,
                "{q:?} samples outside the sheet"
            );
        }
        // …and every destination pixel is covered exactly once.
        let mut hits = vec![0u32; (dst.2 * dst.3) as usize];
        for q in qs {
            for yy in 0..q.h {
                for xx in 0..q.w {
                    let px = q.dx - dst.0 + xx;
                    let py = q.dy - dst.1 + yy;
                    hits[(py * dst.2 + px) as usize] += 1;
                }
            }
        }
        assert!(
            hits.iter().all(|&n| n == 1),
            "every pixel covered exactly once (min {}, max {})",
            hits.iter().min().unwrap(),
            hits.iter().max().unwrap()
        );
    }

    #[test]
    fn a_one_to_one_blit_stays_a_single_quad() {
        let qs = quads((7, 9, 32, 32), SHEET, B);
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0], Quad { dx: 7, dy: 9, w: 32, h: 32, sx: 0, sy: 0, sw: 32, sh: 32 });
    }

    /// The overlay's smallest real panel — one button, 33x33.
    #[test]
    fn a_thirty_three_square_panel_tiles_its_bands_exactly_once() {
        let dst = (0, 0, 33, 33);
        let qs = quads(dst, SHEET, B);
        covered(&qs, dst);
        // All FOUR corners, each named individually. An earlier cut checked two
        // of them and pinned the other pair with an `any(|q| q.sx == 29)` — but
        // there are two right-hand corners, so breaking one left the other
        // satisfying the existential and the mutation survived. An `any` over a
        // set whose members you meant to check one by one can only detect total
        // failure.
        assert!(qs.contains(&Quad { dx: 0, dy: 0, w: 4, h: 4, sx: 0, sy: 0, sw: 4, sh: 4 }));
        assert!(qs.contains(&Quad { dx: 29, dy: 0, w: 4, h: 4, sx: 28, sy: 0, sw: 4, sh: 4 }));
        assert!(qs.contains(&Quad { dx: 0, dy: 29, w: 4, h: 4, sx: 0, sy: 28, sw: 4, sh: 4 }));
        assert!(qs.contains(&Quad { dx: 29, dy: 29, w: 4, h: 4, sx: 28, sy: 28, sw: 4, sh: 4 }));
        // The centre is 25 wide from a 24-wide band, so it takes TWO tiles and
        // the second is one pixel of the band's left edge.
        let centre: Vec<_> = qs.iter().filter(|q| q.sx == 4 && q.sy == 4).collect();
        assert_eq!(centre.len(), 4, "2x2 tiles for a 25x25 centre from 24x24");
        assert!(centre.iter().any(|q| q.w == 1 && q.h == 1), "the clipped corner tile");
    }

    /// The widest the overlay gets: five buttons across.
    #[test]
    fn a_five_wide_panel_repeats_its_top_band() {
        let dst = (0, 0, 5 * 25 + 8, 33);
        let qs = quads(dst, SHEET, B);
        covered(&qs, dst);
        let top: Vec<_> = qs.iter().filter(|q| q.sy == 0 && q.sx == 4).collect();
        // 125 px of destination from a 24 px band: five whole tiles and a 5 px
        // remainder.
        assert_eq!(top.len(), 6);
        assert_eq!(top.iter().map(|q| q.w).sum::<i32>(), 125);
        assert_eq!(top.last().unwrap().w, 5, "the last tile is CLIPPED, not stretched");
    }

    /// The claim the general case rests on: with one axis exactly the sheet's,
    /// the corner-edge-corner stack is the single full-length blit vanilla's
    /// dedicated branch draws.
    #[test]
    fn an_exactly_tall_target_stacks_into_a_full_height_column() {
        let dst = (0, 0, 100, 32);
        let qs = quads(dst, SHEET, B);
        covered(&qs, dst);
        let left: Vec<_> = qs.iter().filter(|q| q.dx == 0).collect();
        assert_eq!(left.len(), 3, "corner, edge, corner");
        assert_eq!(left.iter().map(|q| q.h).sum::<i32>(), 32);
        // And each is 1:1 against the sheet's own left column.
        for q in &left {
            assert_eq!((q.sx, q.sw), (0, 4));
            assert_eq!(q.sy, q.dy, "1:1 vertically, so the column is uncut");
        }
    }

    /// Every band's source origin, named one by one.
    ///
    /// Two mutations survived before this existed, and both were the same
    /// mistake: the witnesses pinned the *left* edge and the top-left corner
    /// and left their opposite numbers free. A sprite's four sides are four
    /// separate claims, and checking two of them is checking two of them.
    ///
    /// Every tile of a band shares the band's source origin — only `sw`/`sh`
    /// shrink on the clipped last row and column — so one `(sx, sy)` per band
    /// is the whole claim.
    #[test]
    fn every_edge_band_samples_its_own_side_of_the_sheet() {
        let dst = (0, 0, 108, 58);
        let qs = quads(dst, SHEET, B);
        covered(&qs, dst);
        // (label, destination band, expected source origin)
        let bands = [
            ("top", (4, 0, 100, 4), (4, 0)),
            ("bottom", (4, 54, 100, 4), (4, 28)),
            ("left", (0, 4, 4, 50), (0, 4)),
            ("right", (104, 4, 4, 50), (28, 4)),
            ("centre", (4, 4, 100, 50), (4, 4)),
        ];
        for (name, (bx, by, bw, bh), origin) in bands {
            let inside: Vec<_> = qs
                .iter()
                .filter(|q| q.dx >= bx && q.dy >= by && q.dx < bx + bw && q.dy < by + bh)
                .collect();
            assert!(!inside.is_empty(), "{name} band is empty");
            for q in inside {
                assert_eq!((q.sx, q.sy), origin, "{name} band quad {q:?}");
            }
        }
    }

    /// `Math.min(border, width / 2)` — a target narrower than its own border.
    #[test]
    fn a_target_narrower_than_its_border_does_not_draw_its_corners_twice() {
        let dst = (0, 0, 6, 6);
        let qs = quads(dst, SHEET, B);
        covered(&qs, dst);
        // Border 4 clamped to 3 each side: four 3x3 corners and nothing else.
        assert_eq!(qs.len(), 4);
        assert!(qs.iter().all(|q| q.w == 3 && q.h == 3));
        // Both right-hand corners still sample the sheet's RIGHT edge — the
        // clamp is about the destination overlapping itself, not about which
        // texels a corner is made of.
        assert_eq!(
            qs.iter().filter(|q| q.sx == 29).count(),
            2,
            "both right-hand corners, not just one"
        );
    }

    /// An odd target, where the clamp is not symmetric.
    #[test]
    fn an_odd_narrow_target_still_covers_every_pixel_once() {
        for w in 1..=12 {
            for h in 1..=12 {
                let dst = (3, 5, w, h);
                covered(&quads(dst, SHEET, B), dst);
            }
        }
    }

    /// Every panel size the overlay can actually ask for.
    #[test]
    fn every_reachable_overlay_panel_is_covered_exactly_once() {
        for total in 1..=64usize {
            let (w, h) = crate::recipe_overlay::panel_size(total);
            let dst = (11, 31, w, h);
            covered(&quads(dst, SHEET, B), dst);
        }
    }
}
