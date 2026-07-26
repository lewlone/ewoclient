//! Sign text colour and line breaking — `AbstractSignRenderer` (M27).
//!
//! M25e drew sign text, but always black and always full-length: a dyed sign
//! rendered in the default colour, glowing text was decoded and ignored, and a
//! line longer than the board simply overhung it. All three are the same
//! method's business, so they are all here.
//!
//! ```text
//! int darkColor = getDarkColor(signText);
//! if (signText.hasGlowingText()) {
//!    textColor   = signText.getColor().getTextColor();
//!    drawOutline = textColor == DyeColor.BLACK.getTextColor() || state.drawOutline;
//!    lightVal    = 15728880;                       // full bright
//! } else {
//!    textColor   = darkColor;
//!    drawOutline = false;
//!    lightVal    = state.lightCoords;              // the block's own light
//! }
//! ```
//!
//! The asymmetry worth naming: **glowing text is not "the same colour, but
//! brighter"**. Unglowing text is the dye scaled to 40%; glowing text is the
//! dye at *full* strength, lit fullbright, with the 40% version used as its
//! outline. So the dark colour is not simply the dim case — it is a value both
//! branches need, which is why vanilla computes it before the branch and why
//! this module returns both.

/// `DyeColor`'s `textColor` field, in registry order.
///
/// The **last** constructor argument, not the first — `DyeColor` carries four
/// different colours per entry (`textureDiffuseColor`, two `MapColor`s, a
/// `fireworkColor`, and this), and picking the wrong one gives a plausible
/// sign in the wrong shade. Stored as `0xRRGGBB`; vanilla wraps each in
/// `ARGB.opaque` at construction.
pub const DYE_TEXT_COLORS: &[(&str, u32)] = &[
    ("white", 0xFFFFFF),
    ("orange", 0xFF681F),
    ("magenta", 0xFF00FF),
    ("light_blue", 0x9AC0CD),
    ("yellow", 0xFFFF00),
    ("lime", 0xBFFF00),
    ("pink", 0xFF69B4),
    ("gray", 0x808080),
    ("light_gray", 0xD3D3D3),
    ("cyan", 0x00FFFF),
    ("purple", 0xA020F0),
    ("blue", 0x0000FF),
    ("brown", 0x8B4513),
    ("green", 0x00FF00),
    ("red", 0xFF0000),
    ("black", 0x000000),
];

/// `DyeColor.BLACK.getTextColor()`, the value both special cases test against.
pub const BLACK_TEXT_COLOR: u32 = 0x000000;

/// `AbstractSignRenderer.BLACK_TEXT_OUTLINE_COLOR` — `-988212`, i.e.
/// `0xFFF0EBCC` opaque, so `0xF0EBCC`.
///
/// A near-white, because glowing *black* text needs a light outline to read at
/// all; every other dye outlines with its own dimmed self.
pub const BLACK_TEXT_OUTLINE_COLOR: u32 = 0xF0EBCC;

/// A sign face's resolved dye, defaulting to black for an absent or unknown
/// `color` tag — which is what vanilla's codec does.
pub fn dye_text_color(name: Option<&str>) -> u32 {
    let Some(name) = name else {
        return BLACK_TEXT_COLOR;
    };
    DYE_TEXT_COLORS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
        .unwrap_or(BLACK_TEXT_COLOR)
}

/// `ARGB.scaleRGB(color, scale)` — per channel, **truncating** to int and
/// clamping.
///
/// `(int)(red * 0.4F)` truncates; rounding instead would brighten every dyed
/// sign by up to one step per channel.
pub fn scale_rgb(color: u32, scale: f32) -> u32 {
    let ch = |shift: u32| -> u32 {
        let v = ((color >> shift) & 0xFF) as f32;
        ((v * scale) as i32).clamp(0, 255) as u32
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// `AbstractSignRenderer.getDarkColor`.
///
/// ```text
/// int color = signText.getColor().getTextColor();
/// return color == BLACK.getTextColor() && signText.hasGlowingText()
///     ? -988212 : ARGB.scaleRGB(color, 0.4F);
/// ```
pub fn dark_color(dye: u32, glowing: bool) -> u32 {
    if dye == BLACK_TEXT_COLOR && glowing {
        BLACK_TEXT_OUTLINE_COLOR
    } else {
        scale_rgb(dye, 0.4)
    }
}

/// How one sign face is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignTextStyle {
    /// The glyph colour, `0xRRGGBB`.
    pub color: u32,
    /// Whether to light the text fullbright rather than from the block.
    ///
    /// Vanilla passes `15728880`, which is `LightTexture.FULL_BRIGHT` — both
    /// nibbles at 15. Glowing text is legible in a dark room, which is the
    /// entire point of the glow ink.
    pub fullbright: bool,
    /// The outline colour, when one is drawn.
    pub outline: Option<u32>,
}

/// `submitSignText`'s colour branch.
///
/// `near` is `state.drawOutline` — vanilla's `isOutlineVisible`, true within
/// 16 blocks of the camera (or while scoping). Glowing **black** text outlines
/// regardless of distance, because without it there is nothing to see.
pub fn text_style(dye: u32, glowing: bool, near: bool) -> SignTextStyle {
    let dark = dark_color(dye, glowing);
    if glowing {
        SignTextStyle {
            color: dye,
            fullbright: true,
            outline: (dye == BLACK_TEXT_COLOR || near).then_some(dark),
        }
    } else {
        SignTextStyle {
            color: dark,
            fullbright: false,
            outline: None,
        }
    }
}

/// The width of a string in font px — `Font.width`, the sum of the advances.
pub fn width(text: &str, advance: &[u8; 256]) -> f32 {
    text.bytes().map(|b| advance[b as usize] as f32).sum()
}

/// `Font.split(text, maxWidth).get(0)` — the first line, which is all a sign
/// draws.
///
/// A sign does **not** wrap onto the next row: `getRenderMessages` splits, then
/// takes fragment 0 and throws the rest away. So an over-long line is
/// *truncated at a word boundary*, not overhung and not carried down.
///
/// `StringSplitter.LineBreakFinder`, for a single style and no newlines:
///
/// ```text
/// case 32: lastSpace = pos;          // falls through — the space's own width counts
/// default: width += charWidth;
///          if (!hadNonZeroWidthChar || width <= maxWidth) { accept; }
///          else break at (lastSpace != -1 ? lastSpace : pos);
/// ```
///
/// Two details that are easy to get backwards. The space that triggers a break
/// is **excluded** from the fragment (`splitLines` passes `lineBreak`, not the
/// adjusted one, when `includeAll` is false). And `hadNonZeroWidthChar` means a
/// single glyph wider than the whole board is still drawn — the result is never
/// empty, so a sign cannot silently lose a line to a wide character.
pub fn split_first(text: &str, max_width: f32, advance: &[u8; 256]) -> String {
    let max_width = max_width.max(1.0);
    let bytes = text.as_bytes();
    let mut width = 0.0f32;
    let mut had_non_zero = false;
    let mut last_space: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b' ' {
            last_space = Some(i);
        }
        let w = advance[b as usize] as f32;
        width += w;
        if !had_non_zero || width <= max_width {
            had_non_zero |= w != 0.0;
            continue;
        }
        let brk = last_space.unwrap_or(i);
        return String::from_utf8_lossy(&bytes[..brk]).into_owned();
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed-width font, so the split arithmetic is checkable by eye.
    fn uniform(w: u8) -> [u8; 256] {
        [w; 256]
    }

    #[test]
    fn the_dye_table_is_the_text_colour_not_the_texture_one() {
        // `red`'s texture diffuse is 11546150 (0xB02E26) and its *text* colour
        // is 0xFF0000. Picking the wrong constructor argument gives a sign
        // that looks plausible and is wrong.
        assert_eq!(dye_text_color(Some("red")), 0xFF0000);
        assert_eq!(dye_text_color(Some("black")), 0x000000);
        assert_eq!(dye_text_color(Some("white")), 0xFFFFFF);
        assert_eq!(DYE_TEXT_COLORS.len(), 16);
    }

    #[test]
    fn an_absent_or_unknown_colour_is_black() {
        assert_eq!(dye_text_color(None), BLACK_TEXT_COLOR);
        assert_eq!(dye_text_color(Some("chartreuse")), BLACK_TEXT_COLOR);
    }

    #[test]
    fn scale_rgb_truncates_rather_than_rounds() {
        // 0xFF * 0.4 = 102.0 exactly; 0xFE * 0.4 = 101.6 -> 101, where rounding
        // would give 102 and brighten every dyed sign by a step.
        assert_eq!(scale_rgb(0xFFFFFF, 0.4), 0x666666);
        assert_eq!(scale_rgb(0xFEFEFE, 0.4), 0x656565);
        assert_eq!(scale_rgb(0x000000, 0.4), 0x000000);
    }

    #[test]
    fn unglowing_text_is_the_dye_at_forty_percent() {
        let s = text_style(dye_text_color(Some("red")), false, false);
        assert_eq!(s.color, 0x660000);
        assert!(!s.fullbright);
        assert_eq!(s.outline, None);
    }

    #[test]
    fn glowing_text_is_the_dye_at_full_strength_not_the_dim_one() {
        // The asymmetry: glow does not brighten the 40% colour, it uses the
        // undimmed dye and demotes the 40% one to the outline.
        let s = text_style(dye_text_color(Some("red")), true, true);
        assert_eq!(s.color, 0xFF0000);
        assert!(s.fullbright);
        assert_eq!(s.outline, Some(0x660000));
    }

    #[test]
    fn glowing_black_outlines_at_any_distance() {
        // Otherwise there would be nothing to see: black glyphs, fullbright.
        let near = text_style(BLACK_TEXT_COLOR, true, true);
        let far = text_style(BLACK_TEXT_COLOR, true, false);
        assert_eq!(near.outline, Some(BLACK_TEXT_OUTLINE_COLOR));
        assert_eq!(far.outline, Some(BLACK_TEXT_OUTLINE_COLOR));
        // ...where a *coloured* glowing sign drops its outline at range.
        assert_eq!(text_style(0xFF0000, true, false).outline, None);
    }

    #[test]
    fn a_line_that_fits_is_untouched() {
        let a = uniform(6);
        assert_eq!(split_first("hello", 90.0, &a), "hello");
    }

    #[test]
    fn an_overlong_line_breaks_at_the_last_space_excluding_it() {
        // 6 px per glyph, board 30 px => 5 glyphs fit.
        let a = uniform(6);
        assert_eq!(split_first("ab cdefgh", 30.0, &a), "ab");
    }

    #[test]
    fn a_spaceless_overlong_line_breaks_mid_word() {
        let a = uniform(6);
        // 5 fit; the 6th overflows and there is no space to retreat to.
        assert_eq!(split_first("abcdefgh", 30.0, &a), "abcde");
    }

    #[test]
    fn a_single_glyph_wider_than_the_board_still_draws() {
        // `hadNonZeroWidthChar` guards the first glyph, so the result is never
        // empty — a sign cannot silently lose a line to one wide character.
        let a = uniform(200);
        assert_eq!(split_first("xy", 90.0, &a), "x");
    }

    #[test]
    fn a_leading_space_does_not_produce_an_empty_line() {
        let a = uniform(6);
        // lastSpace is 0, so the break would be at 0 — but only after the
        // first glyph has been accepted, which the guard ensures.
        assert_eq!(split_first(" abcdefgh", 30.0, &a), "");
    }
}
