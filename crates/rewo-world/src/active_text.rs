//! `ActiveTextCollector` — which styled run is under the cursor (M128).
//!
//! # There is no `getClickedComponentStyleAt` in 26.2
//!
//! Older clients had one, and it walked a line accumulating `font.width` until
//! it passed the mouse. 26.2 does not: `ChatScreen.mouseClicked` builds an
//! `ActiveTextCollector.ClickableStyleFinder`, `ChatComponent
//! .captureClickableText` runs it through **the same private
//! `extractRenderState` the draw runs**, and
//! `ActiveTextCollector.findElementUnderCursor` inverse-transforms the mouse by
//! the text's pose and tests **each prepared glyph's own rectangle**. No x
//! accumulation, no `font.width` call anywhere in the lookup — the lookup is a
//! replay of the layout, which is why this module lays the line out rather than
//! measuring it.
//!
//! # The box is not the sprite cell, and it is not quite the advance either
//!
//! `TextRenderable.Styled` defaults its four `active*` edges to the *render*
//! box, and `BakedSheetGlyph.GlyphInstance` then **overrides exactly one of
//! them**:
//!
//! ```java
//! @Override
//! public float activeRight() {
//!    return this.x + this.glyph.info.getAdvance(this.style.isBold());
//! }
//! ```
//!
//! So the right edge is the advance — boxes tile rather than overlap — while
//! the other three come from the sprite:
//!
//! ```java
//! left  = x + glyph.left + (italic ? min(shearTop, shearBottom) : 0) - extraThickness(bold)
//! top   = y + glyph.up                                               - extraThickness(bold)
//! bottom= y + glyph.down + (hasShadow ? shadowOffset : 0)            + extraThickness(bold)
//! ```
//!
//! For the font Rewo actually draws that resolves to something very simple.
//! `ascii.png`'s provider declares `"ascent": 7` and no `"height"`, so the
//! codec's default 8 applies; `GlyphBitmap`'s defaults then give
//! `left = getBearingLeft() = 0`, `up = 7 - bearingTop = 7 - 7 = 0` and
//! `down = up + pixelHeight/oversample = 8`. With `shadowOffset = 1.0`
//! (`GlyphInfo`'s default, and the finder always prepares with
//! `dropShadow = true`), **a plain glyph's active box is exactly
//! `[x, x + advance) x [y, y + 9)`** — the same rectangle an `EmptyArea`
//! reports. The three style modifiers are transcribed anyway because they are
//! free and because `shearBottom` is *negative*:
//!
//! * bold widens left/top/bottom by `extraThickness = 0.1` and, through
//!   `getAdvance(bold)`, the right edge by the whole `boldOffset = 1.0`;
//! * italic moves the left edge by `min(shearTop, shearBottom)` =
//!   `min(1 - 0.25*0, 1 - 0.25*8)` = **-1**, and does not move the right edge
//!   at all, because that one is overridden;
//! * no shadow would drop the bottom edge to `y + 8`.
//!
//! # Two things a space does differently, and only one of them shows
//!
//! A whitespace codepoint is an `EmptyGlyph`, whose `createGlyph` returns
//! `null`, so `PreparedTextBuilder.accept` records an `EmptyArea` instead. Two
//! consequences:
//!
//! 1. **`addEmptyGlyph` does not call `markSize`.** Empty areas are absent
//!    from `bounds()`, and `findElementUnderCursor` early-outs on
//!    `bounds == null` — so a line of nothing but spaces is unreachable, while
//!    a space *between* two glyphs is clickable.
//! 2. **`visit` emits all glyphs, then all effects, then all empty areas**, so
//!    an empty area outranks any glyph box that overlaps it. That is only
//!    observable for italic text, where the left edge reaches a pixel back
//!    into the previous cell — recorded because it costs one line and the
//!    alternative is a rule that looks arbitrary later.
//!
//! # The scanner keeps the LAST match, and it is not unconditional
//!
//! ```java
//! private final Consumer<Style> styleScanner = style -> {
//!    if (style.getClickEvent() != null || this.includeInsertions && style.getInsertion() != null) {
//!       this.result = style;
//!    }
//! };
//! ```
//!
//! A style with no click event (and no insertion, when insertions are off)
//! **does not clear** an earlier find. It cannot matter within one line
//! because the boxes tile, but it matters across the `accept` calls of a whole
//! chat box: the finder is run over every visible row and keeps the last row
//! that answered. Rewo's rows are emitted top-first, so the *lowest* clickable
//! row under the cursor wins — which, since a point is inside at most one
//! row's band, is the row the cursor is in.

use crate::chat_style::{ChatSpan, ChatStyle};

/// `GlyphInfo.getShadowOffset()`.
pub const SHADOW_OFFSET: f32 = 1.0;
/// `BakedSheetGlyph.extraThickness(true)`.
pub const EXTRA_THICKNESS: f32 = 0.1;
/// `EmptyArea.DEFAULT_ASCENT`, and also the literal `7.0F` `PreparedTextBuilder
/// .accept` passes when it builds one.
pub const EMPTY_ASCENT: f32 = 7.0;
/// `EmptyArea.DEFAULT_HEIGHT`, likewise passed as `9.0F`.
pub const EMPTY_HEIGHT: f32 = 9.0;

/// `BitmapProvider.Definition.ascent` for `ascii.png` / `nonlatin_european.png`
/// — both declare `"ascent": 7`.
const FONT_ASCENT: f32 = 7.0;
/// `BitmapProvider.Definition.height`, which those providers do **not**
/// declare, so it is the codec's default 8.
const FONT_HEIGHT: f32 = 8.0;

/// `GlyphBitmap.getTop()` — `7.0F - getBearingTop()`, and `BitmapProvider
/// .Glyph`'s `getBearingTop` is the declared ascent.
const GLYPH_UP: f32 = 7.0 - FONT_ASCENT;
/// `GlyphBitmap.getBottom()` — `getTop() + pixelHeight / oversample`, which for
/// a `scale == 1` bitmap provider is the cell height.
const GLYPH_DOWN: f32 = GLYPH_UP + FONT_HEIGHT;
/// `GlyphBitmap.getLeft()` — `getBearingLeft()`, whose default is 0 and which
/// `BitmapProvider.Glyph` does not override.
const GLYPH_LEFT: f32 = 0.0;

/// `BakedSheetGlyph.shearTop()` — `1.0F - 0.25F * up`.
const SHEAR_TOP: f32 = 1.0 - 0.25 * GLYPH_UP;
/// `BakedSheetGlyph.shearBottom()` — `1.0F - 0.25F * down`. **Negative** for
/// the default font, which is why an italic glyph's left edge moves *left*.
const SHEAR_BOTTOM: f32 = 1.0 - 0.25 * GLYPH_DOWN;

/// The codepoints the default font's `space` provider supplies, and therefore
/// the ones that bake to an `EmptyGlyph`.
///
/// `assets/minecraft/font/include/space.json` declares exactly two:
/// `" ": 4` and `"‌": 0`. Everything else in the default font comes from a
/// bitmap provider and has a real sprite, including the missing-glyph box.
pub fn is_empty_glyph(c: char) -> bool {
    c == ' ' || c == '\u{200C}'
}

/// One laid-out `ActiveArea` — a glyph's or a space's clickable rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActiveArea {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    /// Which input span this character came from — the `style()` of the
    /// `ActiveArea`, by index rather than by clone.
    pub span: usize,
    /// Whether this is an `EmptyArea` rather than a `TextRenderable.Styled`.
    /// Decides both the `bounds` contribution and the visit order.
    pub empty: bool,
}

impl ActiveArea {
    /// `ActiveTextCollector.isPointInRectangle` — **half-open on the right and
    /// the bottom**, which is what makes tiled boxes partition the line rather
    /// than double-count its seams.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// `Font.PreparedText` — the areas of one laid-out line, in `visit` order.
#[derive(Clone, Debug, Default)]
pub struct Prepared {
    /// Glyph areas first, then empty areas: `visit` drains `glyphs`, then
    /// `effects` (which carry no style and are not `ActiveArea`s), then
    /// `emptyAreas`.
    pub areas: Vec<ActiveArea>,
    /// `PreparedTextBuilder.bounds()` — `null` when nothing called `markSize`.
    ///
    /// Built from the **render** box (`markSize(instance.left(), .top(),
    /// .right(), .bottom())`), which is wider than the active box, and from
    /// glyphs only.
    pub bounds: Option<(f32, f32, f32, f32)>,
}

/// `Font.prepareText` for one styled line at `(x, y)`, in the space the text is
/// drawn in.
///
/// `width_of` is the same provider [`crate::string_splitter`] takes —
/// `getGlyph(cp).info().getAdvance(style.isBold())` — so a hit test and a wrap
/// cannot disagree about where a character sits.
pub fn prepare(
    spans: &[ChatSpan],
    x: f32,
    y: f32,
    width_of: &dyn Fn(&str, ChatStyle) -> i32,
) -> Prepared {
    let mut glyphs: Vec<ActiveArea> = Vec::new();
    let mut empties: Vec<ActiveArea> = Vec::new();
    let mut bounds: Option<(f32, f32, f32, f32)> = None;
    let mut pen = x;
    for (index, span) in spans.iter().enumerate() {
        let style = span.style();
        let thick = if span.bold { EXTRA_THICKNESS } else { 0.0 };
        for c in span.text.chars() {
            // `float advance = glyphInfo.getAdvance(bold)` — the bold offset is
            // already in the provider (M126b).
            let advance = width_of(&c.to_string(), style.clone()) as f32;
            if is_empty_glyph(c) {
                // `new EmptyArea(this.x, this.y, advance, 7.0F, 9.0F, style)`,
                // whose `activeTop` is `y + 7 - ascent` — zero here — and whose
                // `activeBottom` is that plus 9. It contributes nothing to
                // `bounds`.
                empties.push(ActiveArea {
                    left: pen,
                    top: y + EMPTY_ASCENT - EMPTY_ASCENT,
                    right: pen + advance,
                    bottom: y + EMPTY_ASCENT - EMPTY_ASCENT + EMPTY_HEIGHT,
                    span: index,
                    empty: true,
                });
            } else {
                let shear_left = if span.italic {
                    SHEAR_TOP.min(SHEAR_BOTTOM)
                } else {
                    0.0
                };
                let shear_right = if span.italic {
                    SHEAR_TOP.max(SHEAR_BOTTOM)
                } else {
                    0.0
                };
                let left = pen + GLYPH_LEFT + shear_left - thick;
                let top = y + GLYPH_UP - thick;
                let bottom = y + GLYPH_DOWN + SHADOW_OFFSET + thick;
                glyphs.push(ActiveArea {
                    left,
                    top,
                    // The one override: the ADVANCE, not the sprite's right.
                    right: pen + advance,
                    bottom,
                    span: index,
                    empty: false,
                });
                // `markSize(instance.left(), .top(), .right(), .bottom())` —
                // the render box, whose right edge is the sprite's plus the
                // shadow and the shear.
                let render_right =
                    pen + GLYPH_LEFT + FONT_HEIGHT + SHADOW_OFFSET + shear_right + thick;
                bounds = Some(match bounds {
                    None => (left, top, render_right, bottom),
                    Some((l, t, r, b)) => {
                        (l.min(left), t.min(top), r.max(render_right), b.max(bottom))
                    }
                });
            }
            pen += advance;
        }
    }
    glyphs.extend(empties);
    Prepared { areas: glyphs, bounds }
}

/// `ActiveTextCollector.findElementUnderCursor` — the index of the span whose
/// area the point is in, or `None`.
///
/// The `bounds` early-out is transcribed rather than optimised away: it is what
/// makes a line of only spaces unreachable, since nothing contributed to it.
/// **Last match wins**, because the visitor's `output.accept(glyph.style())`
/// runs for every hit and the caller's scanner assigns.
pub fn find_area_under_cursor(prepared: &Prepared, x: f32, y: f32) -> Option<&ActiveArea> {
    let (l, t, r, b) = prepared.bounds?;
    // `bounds.containsPoint((int)testX, (int)testY)` — `ScreenRectangle`'s
    // containment is half-open the same way `isPointInRectangle` is.
    if !(x >= l && x < r && y >= t && y < b) {
        return None;
    }
    prepared.areas.iter().filter(|a| a.contains(x, y)).next_back()
}

/// `ActiveTextCollector.ClickableStyleFinder` — accumulates the last style
/// under the cursor that is worth reporting.
///
/// One finder is run over **every** line the chat box draws, exactly as
/// `captureClickableText` runs one over `extractRenderState`'s whole walk.
#[derive(Clone, Debug, Default)]
pub struct ClickableStyleFinder {
    /// `includeInsertions(this.insertionClickMode())` — shift held.
    pub include_insertions: bool,
    result: Option<ChatStyle>,
}

impl ClickableStyleFinder {
    pub fn new(include_insertions: bool) -> Self {
        Self { include_insertions, result: None }
    }

    /// `ClickableStyleFinder.accept(alignment, anchorX, y, parameters, text)`
    /// for one left-aligned line at `(x, y)`.
    pub fn accept(
        &mut self,
        spans: &[ChatSpan],
        x: f32,
        y: f32,
        test: (f32, f32),
        width_of: &dyn Fn(&str, ChatStyle) -> i32,
    ) {
        let prepared = prepare(spans, x, y, width_of);
        if let Some(area) = find_area_under_cursor(&prepared, test.0, test.1) {
            self.offer(spans[area.span].style());
        }
    }

    /// The `styleScanner` lambda. **A style with nothing to report does not
    /// clear an earlier find** — the assignment is inside the guard.
    pub fn offer(&mut self, style: ChatStyle) {
        let worth_reporting =
            style.click().is_some() || (self.include_insertions && style.insertion().is_some());
        if worth_reporting {
            self.result = Some(style);
        }
    }

    /// `result()`.
    pub fn result(self) -> Option<ChatStyle> {
        self.result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_events::{ChatEvents, ClickEvent};
    use std::sync::Arc;

    /// A six-pixel font with vanilla's one-pixel bold offset — the same
    /// fixture `string_splitter`'s tests use.
    fn w6s(s: &str, style: ChatStyle) -> i32 {
        s.chars().count() as i32 * (6 + i32::from(style.bold))
    }

    fn link(text: &str, cmd: &str) -> ChatSpan {
        ChatStyle {
            events: Some(Arc::new(ChatEvents {
                click: Some(ClickEvent::RunCommand(cmd.into())),
                ..Default::default()
            })),
            ..ChatStyle::WHITE
        }
        .span(text)
    }

    fn plain(text: &str) -> ChatSpan {
        ChatStyle::WHITE.span(text)
    }

    fn cmd_at(spans: &[ChatSpan], x: f32, y: f32, test: (f32, f32)) -> Option<String> {
        let mut f = ClickableStyleFinder::new(false);
        f.accept(spans, x, y, test, &w6s);
        match f.result()?.click()? {
            ClickEvent::RunCommand(c) => Some(c.clone()),
            other => panic!("{other:?}"),
        }
    }

    /// **The right edge is the advance, and the boxes tile.** A six-pixel
    /// glyph at x=0 owns `[0, 6)`; the next owns `[6, 12)`. The sprite CELL is
    /// eight wide, so a cell-based box would have them overlapping by two and
    /// the second glyph would win at x=6..8 for the wrong reason.
    #[test]
    fn a_plain_glyphs_box_is_its_advance_by_nine() {
        let p = prepare(&[plain("ab")], 0.0, 0.0, &w6s);
        assert_eq!(p.areas.len(), 2);
        assert_eq!(
            (p.areas[0].left, p.areas[0].right),
            (0.0, 6.0)
        );
        assert_eq!((p.areas[1].left, p.areas[1].right), (6.0, 12.0));
        assert_eq!((p.areas[0].top, p.areas[0].bottom), (0.0, 9.0));
    }

    /// `isPointInRectangle` is `x >= left && x < right`, so the seam belongs to
    /// the character on its right.
    #[test]
    fn the_seam_belongs_to_the_right_hand_character() {
        let spans = [link("a", "/a"), link("b", "/b")];
        assert_eq!(cmd_at(&spans, 0.0, 0.0, (5.9, 4.0)).as_deref(), Some("/a"));
        assert_eq!(cmd_at(&spans, 0.0, 0.0, (6.0, 4.0)).as_deref(), Some("/b"));
    }

    /// Outside the row's nine pixels, nothing.
    #[test]
    fn the_row_is_nine_pixels_tall() {
        let spans = [link("a", "/a")];
        assert_eq!(cmd_at(&spans, 0.0, 10.0, (2.0, 10.0)).as_deref(), Some("/a"));
        assert_eq!(cmd_at(&spans, 0.0, 10.0, (2.0, 18.9)).as_deref(), Some("/a"));
        assert_eq!(cmd_at(&spans, 0.0, 10.0, (2.0, 19.0)), None);
        assert_eq!(cmd_at(&spans, 0.0, 10.0, (2.0, 9.9)), None);
    }

    /// **A line of nothing but spaces is unreachable**, because
    /// `addEmptyGlyph` never calls `markSize` and `findElementUnderCursor`
    /// early-outs on a null `bounds`.
    #[test]
    fn a_line_of_only_spaces_has_no_bounds_and_so_no_hit() {
        let p = prepare(&[link("   ", "/x")], 0.0, 0.0, &w6s);
        assert_eq!(p.areas.len(), 3);
        assert!(p.areas.iter().all(|a| a.empty));
        assert_eq!(p.bounds, None);
        assert_eq!(cmd_at(&[link("   ", "/x")], 0.0, 0.0, (3.0, 4.0)), None);
    }

    /// A space BETWEEN glyphs is clickable, because the glyphs supplied the
    /// bounds.
    #[test]
    fn a_space_between_glyphs_is_clickable() {
        assert_eq!(
            cmd_at(&[link("a b", "/x")], 0.0, 0.0, (8.0, 4.0)).as_deref(),
            Some("/x")
        );
    }

    /// `visit` drains glyphs before empty areas, so where the two overlap the
    /// space wins. Italic is the only way to make them overlap: `shearBottom`
    /// is `1 - 0.25 * 8 = -1`, so an italic glyph's left edge reaches one pixel
    /// back into the preceding cell.
    #[test]
    fn an_empty_area_outranks_a_glyph_that_overlaps_it() {
        let italic = |text: &str, cmd: &str| ChatSpan {
            italic: true,
            ..link(text, cmd)
        };
        // "a" is a space-carrying link, "b" is an italic glyph link that
        // reaches back over the space's last pixel.
        let spans = [link("x ", "/space"), italic("b", "/glyph")];
        let p = prepare(&spans, 0.0, 0.0, &w6s);
        // The italic glyph starts at 12 and its box opens at 11.
        let glyph = p.areas.iter().find(|a| !a.empty && a.span == 1).unwrap();
        assert_eq!(glyph.left, 11.0);
        // 11.5 is inside both the space's [6, 12) and the glyph's [11, 18).
        assert_eq!(cmd_at(&spans, 0.0, 0.0, (11.5, 4.0)).as_deref(), Some("/space"));
    }

    /// Bold moves three edges by `extraThickness` and the fourth by the whole
    /// `boldOffset`, because the right edge is the advance.
    #[test]
    fn bold_widens_the_box_by_the_bold_offset_on_the_right() {
        let bold = ChatSpan { bold: true, ..plain("a") };
        let p = prepare(&[bold], 0.0, 0.0, &w6s);
        let a = p.areas[0];
        assert_eq!(a.right, 7.0);
        assert!((a.left - -0.1).abs() < 1e-6);
        assert!((a.top - -0.1).abs() < 1e-6);
        assert!((a.bottom - 9.1).abs() < 1e-6);
    }

    /// A style with no click event does not clear an earlier find — the
    /// assignment is inside the guard, not beside it.
    #[test]
    fn an_unclickable_style_does_not_clear_the_result() {
        let mut f = ClickableStyleFinder::new(false);
        f.offer(link("a", "/a").style());
        f.offer(ChatStyle::WHITE);
        assert!(f.result().is_some());
    }

    /// An insertion is only worth reporting with shift held.
    #[test]
    fn an_insertion_is_reported_only_when_insertions_are_included() {
        let style = ChatStyle {
            events: Some(Arc::new(ChatEvents {
                insertion: Some("Steve".into()),
                ..Default::default()
            })),
            ..ChatStyle::WHITE
        };
        let mut off = ClickableStyleFinder::new(false);
        off.offer(style.clone());
        assert!(off.result().is_none());
        let mut on = ClickableStyleFinder::new(true);
        on.offer(style);
        assert!(on.result().is_some());
    }

    /// The finder accumulates across `accept` calls, and the last row that
    /// answered wins.
    #[test]
    fn the_finder_accumulates_across_lines() {
        let mut f = ClickableStyleFinder::new(false);
        f.accept(&[link("a", "/top")], 0.0, 0.0, (2.0, 12.0), &w6s);
        f.accept(&[link("a", "/bottom")], 0.0, 9.0, (2.0, 12.0), &w6s);
        match f.result().unwrap().click().unwrap() {
            ClickEvent::RunCommand(c) => assert_eq!(c, "/bottom"),
            other => panic!("{other:?}"),
        }
    }

    /// The pen advances across spans, so a link's second span is where its
    /// first span's width puts it — the same sum the renderer makes.
    #[test]
    fn the_pen_carries_across_spans() {
        let spans = [plain("aaa"), link("bbb", "/x")];
        assert_eq!(cmd_at(&spans, 0.0, 0.0, (17.0, 4.0)), None);
        assert_eq!(cmd_at(&spans, 0.0, 0.0, (18.0, 4.0)).as_deref(), Some("/x"));
        assert_eq!(cmd_at(&spans, 0.0, 0.0, (35.9, 4.0)).as_deref(), Some("/x"));
        assert_eq!(cmd_at(&spans, 0.0, 0.0, (36.0, 4.0)), None);
    }
}
