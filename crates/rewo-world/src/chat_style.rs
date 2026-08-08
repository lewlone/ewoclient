//! Per-run chat styling — legacy `§` codes and text-component trees, resolved
//! into styled spans (M52d).
//!
//! # Why this crate and not `rewo-gpu`
//!
//! The input is a **network** text component: an `Nbt` tag off the wire, in
//! `ComponentSerialization.CODEC`'s shape. `Nbt` lives in `rewo-proto`, which
//! `rewo-gpu` does not depend on and should not — the GPU crate has no
//! business knowing the wire format. So the *parse* belongs here, beside
//! `rewo_net::component_wire::nbt_text`, which is the plain-text answer this
//! module
//! is the styled replacement for.
//!
//! The *output* is deliberately renderer-agnostic. [`ChatSpan`] names colours
//! and flags and nothing about typefaces, exactly as `rewo_gpu::tooltip::Span`
//! does one layer up, so `rewo-app` can hand a line to either the vanilla
//! bitmap pass or the Velvet type stack without the model changing.
//!
//! # What is parsed, and what a wrong implementation would look like
//!
//! Both halves of vanilla's styling reach the same span list:
//!
//! - **Legacy `§` codes** in a literal string, per `StringDecomposer.
//!   iterateFormatted` + `Style.applyLegacyFormat`.
//! - **Component trees** — `color`, the five boolean flags, and `extra`
//!   children inheriting through `Style.applyTo` + `Component.visit`.
//!
//! Every rule below is one a plausible-looking implementation gets wrong
//! *silently* — the output is still text in some colour, so nothing crashes
//! and nothing looks obviously broken. Each has a test pinning it:
//!
//! 1. **A colour code clears the five format flags.** `applyLegacyFormat`'s
//!    `default:` branch assigns `false` to all of them before setting the
//!    colour, so `§c§lX` is bold red and `§l§cX` is *plain* red. Treating a
//!    colour as "just the colour" makes the second case bold.
//! 2. **`§r` resets everything, and resets to the *enclosing* style** — not to
//!    white. `iterateFormatted` intercepts `RESET` and substitutes its
//!    `resetStyle` argument, which the four-argument overload seeds with the
//!    run's own resolved style. So `§r` inside a red component returns to red.
//! 3. **An unrecognised code consumes both characters.** `iterateFormatted`
//!    runs `i++` outside the `formatting != null` guard, so `§z` leaves no
//!    style change *and* no text.
//! 4. **An explicitly `false` field beats an inherited `true`.** `applyTo` is
//!    a null check, not a truth check — `"bold": false` on a child of a bold
//!    parent is un-bold. An `if bold { set }` implementation inherits `true`.
//! 5. **A `#` colour is hex, not CSS.** `TextColor.parseColor` does
//!    `Integer.parseInt(s.substring(1), 16)`, so `#f00` is `0x000F00`, a dark
//!    green — *not* `#ff0000`.
//! 6. **A top-level list makes its first element the parent of the rest.**
//!    `createFromList` copies element 0 and appends the others as its
//!    siblings, so they inherit element 0's style, not the caller's.
//!
//! # Deliberate deviations
//!
//! - **An unparseable `color` is treated as absent** (the field inherits).
//!   Vanilla's `optionalFieldOf` may instead fail the whole component decode
//!   depending on the DFU version's leniency; dropping one field is the
//!   non-fatal choice, and a chat line that loses a colour beats a chat line
//!   that vanishes.
//! - **A `§` before a supplementary character drops the whole character.**
//!   Vanilla indexes UTF-16, so it consumes only the high surrogate and then
//!   emits U+FFFD for the orphaned low one. Reproducing that would mean
//!   carrying UTF-16 semantics through a Rust `str` for a case no server
//!   produces on purpose.
//! - **Score / selector / NBT contents are not resolved.** All three need
//!   world state a component walker has no business reaching for — a
//!   scoreboard, an entity selector's matches, a block entity's NBT. Each
//!   falls back to emitting nothing, carrying the resolved style.
//!   **`translate` used to be on this list and no longer is**: it is resolved
//!   against the language table since M125 (see [`crate::chat_translate`]),
//!   and passing no table reproduces the old key-as-text behaviour exactly,
//!   because that is what `getOrDefault(key)` already does for a missing key.
//! - **The walk is bounded** by [`MAX_COMPONENT_STEPS`], which vanilla is not.
//!   See that constant for why a `translate` argument makes the difference
//!   between a linear tree and an exponential one.
//!

use rewo_data::lang::{Language, Part};
use rewo_proto::nbt::Nbt;

/// `ChatFormatting.PREFIX_CODE`.
pub const SECTION_SIGN: char = '\u{00A7}';

/// `TextColor`'s sixteen named colours, in `ChatFormatting` declaration order.
///
/// That order is load-bearing twice over: it is the order
/// `TextColor.fromLegacyFormat` maps, so index *is* the legacy code
/// (`0`..`9`, `a`..`f`), and the names are the strings `NAMED_COLORS` keys —
/// so one table serves both `§c` and `"color": "red"`.
///
/// The values are `TextColor`'s own decimal literals converted to hex, not
/// eyeballed: `dark_blue` is 170, `gold` is 16755200, `blue` is 5592575.
pub const NAMED_COLORS: [(&str, u32); 16] = [
    ("black", 0x00_0000),
    ("dark_blue", 0x00_00AA),
    ("dark_green", 0x00_AA00),
    ("dark_aqua", 0x00_AAAA),
    ("dark_red", 0xAA_0000),
    ("dark_purple", 0xAA_00AA),
    ("gold", 0xFF_AA00),
    ("gray", 0xAA_AAAA),
    ("dark_gray", 0x55_5555),
    ("blue", 0x55_55FF),
    ("green", 0x55_FF55),
    ("aqua", 0x55_FFFF),
    ("red", 0xFF_5555),
    ("light_purple", 0xFF_55FF),
    ("yellow", 0xFF_FF55),
    ("white", 0xFF_FFFF),
];

/// The largest value `TextColor.parseColor` accepts — it masks with
/// `& 16777215` on construction but *rejects* anything above it first.
const MAX_RGB: u32 = 0xFF_FFFF;

/// Unpack a packed RGB into the `[f32; 3]` the tooltip and HUD paths already
/// use (a plain `/ 255.0`, matching `rarity_color` in the app crate).
pub fn rgb_f32(rgb: u32) -> [f32; 3] {
    [
        ((rgb >> 16) & 0xFF) as f32 / 255.0,
        ((rgb >> 8) & 0xFF) as f32 / 255.0,
        (rgb & 0xFF) as f32 / 255.0,
    ]
}

/// A **resolved** style — every field decided, nothing left to inherit.
///
/// Vanilla's `Style` is all `@Nullable`, because a component's own style is a
/// *patch* over its parent's. That partial form is a decode detail and stays
/// private ([`StyleFields`]); what a renderer needs is the resolved answer,
/// which is what this is.
///
/// The colour is not an `Option` for the same reason vanilla's renderer takes
/// a colour argument: `Style.getColor()` returning null means "whatever the
/// call site defaults to", and that default differs by surface (chat is
/// white, lore is dark purple). Rewo pushes the decision to the *caller* of
/// [`parse_legacy`] / [`parse_component`] instead of into every span.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChatStyle {
    pub color: [f32; 3],
    pub bold: bool,
    pub italic: bool,
    pub underlined: bool,
    pub strikethrough: bool,
    pub obfuscated: bool,
}

impl ChatStyle {
    /// The chat HUD's baseline: white, no flags.
    pub const WHITE: ChatStyle = ChatStyle::plain([1.0, 1.0, 1.0]);

    pub const fn plain(color: [f32; 3]) -> Self {
        Self {
            color,
            bold: false,
            italic: false,
            underlined: false,
            strikethrough: false,
            obfuscated: false,
        }
    }

    /// Clear the five format flags but keep the colour — `applyLegacyFormat`'s
    /// `default:` branch, which is the whole of rule 1 in the module docs.
    const fn colored(color: [f32; 3]) -> Self {
        Self::plain(color)
    }

    pub fn span(&self, text: impl Into<String>) -> ChatSpan {
        ChatSpan {
            text: text.into(),
            color: self.color,
            bold: self.bold,
            italic: self.italic,
            underlined: self.underlined,
            strikethrough: self.strikethrough,
            obfuscated: self.obfuscated,
        }
    }
}

/// One styled run of a chat line.
///
/// Flat rather than `{ text, style }` so a renderer reads `span.italic`
/// directly, matching `rewo_gpu::tooltip::Span`'s shape one layer up.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatSpan {
    pub text: String,
    pub color: [f32; 3],
    pub bold: bool,
    pub italic: bool,
    pub underlined: bool,
    pub strikethrough: bool,
    pub obfuscated: bool,
}

impl ChatSpan {
    /// The span's styling, without its text — for comparing two runs.
    pub fn style(&self) -> ChatStyle {
        ChatStyle {
            color: self.color,
            bold: self.bold,
            italic: self.italic,
            underlined: self.underlined,
            strikethrough: self.strikethrough,
            obfuscated: self.obfuscated,
        }
    }
}

/// One chat line: spans laid end to end on a shared baseline.
pub type ChatLine = Vec<ChatSpan>;

/// The plain text of a line, spans concatenated.
///
/// Equivalent to `StringDecomposer.getPlainText` for what this module parses:
/// the `§` codes are already gone, because they never became text.
pub fn plain_text(line: &ChatLine) -> String {
    line.iter().map(|s| s.text.as_str()).collect()
}

/// `ChatFormatting.getByCode` for the sixteen colours — the index into
/// [`NAMED_COLORS`], which is also the enum's ordinal.
fn legacy_color_index(code: char) -> Option<usize> {
    match code {
        '0'..='9' => Some(code as usize - '0' as usize),
        'a'..='f' => Some(10 + (code as usize - 'a' as usize)),
        _ => None,
    }
}

/// `Style.applyLegacyFormat`, with `iterateFormatted`'s `RESET` interception
/// folded in.
///
/// `None` is `getByCode` returning null — the caller has still consumed both
/// characters by then (module rule 3).
fn apply_legacy(style: ChatStyle, reset: ChatStyle, code: char) -> Option<ChatStyle> {
    // `getByCode` lowercases before matching, so `§C` is `§c`.
    Some(match code.to_ascii_lowercase() {
        'k' => ChatStyle { obfuscated: true, ..style },
        'l' => ChatStyle { bold: true, ..style },
        'm' => ChatStyle { strikethrough: true, ..style },
        'n' => ChatStyle { underlined: true, ..style },
        'o' => ChatStyle { italic: true, ..style },
        // Not `Style.EMPTY`: `iterateFormatted` substitutes `resetStyle`
        // before `applyLegacyFormat` ever sees RESET.
        'r' => reset,
        lower => ChatStyle::colored(rgb_f32(NAMED_COLORS[legacy_color_index(lower)?].1)),
    })
}

/// Parse legacy `§` codes in a literal string.
///
/// `base` is both the starting style and what `§r` returns to — vanilla's
/// two-argument `iterateFormatted(string, style, sink)`, which forwards
/// `style` as `resetStyle` as well.
pub fn parse_legacy(text: &str, base: ChatStyle) -> ChatLine {
    let mut out = Vec::new();
    push_legacy(text, base, base, &mut out);
    out
}

/// The four-argument `iterateFormatted`: a current style and a separate reset
/// target, appending onto an existing line.
fn push_legacy(text: &str, current: ChatStyle, reset: ChatStyle, out: &mut ChatLine) {
    let mut style = current;
    let mut buf = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != SECTION_SIGN {
            buf.push(ch);
            continue;
        }
        // `if (i + 1 >= size) break;` — a dangling `§` is dropped, not drawn.
        let Some(code) = chars.next() else { break };
        let Some(next) = apply_legacy(style, reset, code) else {
            // Unrecognised: `i++` already ran, so both characters are gone and
            // the style is untouched.
            continue;
        };
        if next != style {
            flush(&mut buf, style, out);
            style = next;
        }
    }
    flush(&mut buf, style, out);
}

/// Emit the buffered run, if there is one.
///
/// Empty spans are dropped rather than emitted. Vanilla's sink is per
/// codepoint and never sees a run at all, so a zero-length span is an
/// artefact of batching — and `§a§b§cx` would otherwise produce two invisible
/// spans before the visible one.
fn flush(buf: &mut String, style: ChatStyle, out: &mut ChatLine) {
    if !buf.is_empty() {
        out.push(style.span(std::mem::take(buf)));
    }
}

/// How many component nodes one line may visit before the walk gives up.
///
/// **This is a bound on work, not on nesting, and the difference is the whole
/// point.** `extra` recursion is self-limiting: every level costs bytes, so a
/// packet of size *n* yields O(*n*) nodes. A `translate` argument is not, and
/// the reason is that a template may name the same argument more than once —
/// `%1$s%1$s` *duplicates* its subtree, so output is exponential in nesting
/// depth. A depth cap alone does not fix it either, because the fan-out is
/// server-controlled: the `fallback` field is a string the server chooses, so
/// it can hold as many `%1$s`s as its packet budget allows, and eight levels
/// of a hundred-way fan-out is 10^16 nodes.
///
/// Vanilla has the same shape (`TranslatableContents.visit` recurses into a
/// component argument with no budget) and Rewo deliberately does not inherit
/// it. A step counter bounds the work absolutely, where a text-length budget
/// would not: a tree whose leaves emit *nothing* costs no text and still costs
/// exponential time. Sixty-five thousand nodes is far past any legitimate
/// message and far short of anything that stalls a frame.
pub const MAX_COMPONENT_STEPS: u32 = 65_536;

/// Parse a network text component into styled spans.
///
/// `base` is the enclosing style — the `parentStyle` argument of
/// `Component.visit`, and the colour an unstyled component ends up with.
///
/// `lang` resolves `translate` components (M125). It is `Option` because Rewo
/// parses components in two places and only one of them has a table: `None`
/// selects `getOrDefault(key)`'s key-as-default, which is exactly what this
/// function emitted before there was any resolution at all.
pub fn parse_component(tag: &Nbt, base: ChatStyle, lang: Option<&Language>) -> ChatLine {
    let mut out = Vec::new();
    let mut steps = MAX_COMPONENT_STEPS;
    walk(tag, base, lang, &mut steps, &mut out);
    out
}

fn walk(
    tag: &Nbt,
    parent: ChatStyle,
    lang: Option<&Language>,
    steps: &mut u32,
    out: &mut ChatLine,
) {
    if *steps == 0 {
        return;
    }
    *steps -= 1;
    match tag {
        // `Codec.STRING` -> `Component.literal`, whose style is EMPTY, so it
        // resolves to the parent's. Legacy codes inside it still parse.
        Nbt::String(s) => push_legacy(s, parent, parent, out),

        // `createFromList`: element 0 is copied as the root and the rest are
        // *appended to it*, so they are its siblings and inherit its style —
        // not the caller's (module rule 6).
        Nbt::List(items) => {
            let Some((first, rest)) = items.split_first() else { return };
            walk(first, parent, lang, steps, out);
            let root = resolved_style(first, parent);
            for item in rest {
                walk(item, root, lang, steps, out);
            }
        }

        Nbt::Compound(_) => {
            let style = resolve_style(tag, parent);
            // `Component.visit` runs the contents before the siblings.
            if let Some(Nbt::String(s)) = tag.get("text") {
                // `resetStyle` is this run's own resolved style, so a `§r`
                // here returns to the component's colour, not to white.
                push_legacy(s, style, style, out);
            } else if let Some(t) = crate::chat_translate::translatable(tag, lang) {
                walk_translatable(&t, style, lang, steps, out);
            }
            if let Some(Nbt::List(children)) = tag.get("extra") {
                for child in children {
                    walk(child, style, lang, steps, out);
                }
            }
        }

        _ => {}
    }
}

/// `TranslatableContents.visit(output, currentStyle)` — the decomposed parts,
/// each visited with the translatable's own resolved style.
///
/// `style` is that resolved style, so a literal run of the template takes it
/// verbatim (`FormattedText.of(prefix).visit` forwards the style it is given)
/// while a component argument applies its own **on top** of it, because
/// `Component.visit` opens with `this.getStyle().applyTo(parentStyle)`. That
/// is the same composition an `extra` child gets, and it is what makes a
/// team-coloured sender name keep its colour inside a grey `/msg` line.
fn walk_translatable(
    t: &crate::chat_translate::Translatable<'_>,
    style: ChatStyle,
    lang: Option<&Language>,
    steps: &mut u32,
    out: &mut ChatLine,
) {
    let Some(parts) = t.parts() else {
        // `TranslatableFormatException` -> `[FormattedText.of(format)]`: the
        // looked-up template, unsubstituted, as one part. Not the key.
        push_legacy(t.template, style, style, out);
        return;
    };
    for part in parts {
        match part {
            Part::Literal(s) => push_legacy(s, style, style, out),
            Part::Arg(i) => match crate::chat_translate::primitive_arg_text(&t.args[i]) {
                // `FormattedText.of(arg.toString())` — the caller's style,
                // with no style of its own to apply.
                Some(text) => push_legacy(&text, style, style, out),
                None => walk(&t.args[i], style, lang, steps, out),
            },
        }
    }
}

/// The style a component resolves to, for use as its children's parent.
///
/// A list's is its first element's, because that element became the root.
fn resolved_style(tag: &Nbt, parent: ChatStyle) -> ChatStyle {
    match tag {
        Nbt::Compound(_) => resolve_style(tag, parent),
        Nbt::List(items) => items
            .first()
            .map(|first| resolved_style(first, parent))
            .unwrap_or(parent),
        _ => parent,
    }
}

/// `Style.applyTo(parent)` — a field present on this component wins, absent
/// inherits.
///
/// Presence, not truth: `"italic": false` under an italic parent is upright
/// (module rule 4).
fn resolve_style(tag: &Nbt, parent: ChatStyle) -> ChatStyle {
    let mut style = parent;
    if let Some(color) = tag.get("color").and_then(Nbt::as_str).and_then(parse_color) {
        style.color = color;
    }
    if let Some(v) = nbt_bool(tag.get("bold")) {
        style.bold = v;
    }
    if let Some(v) = nbt_bool(tag.get("italic")) {
        style.italic = v;
    }
    if let Some(v) = nbt_bool(tag.get("underlined")) {
        style.underlined = v;
    }
    if let Some(v) = nbt_bool(tag.get("strikethrough")) {
        style.strikethrough = v;
    }
    if let Some(v) = nbt_bool(tag.get("obfuscated")) {
        style.obfuscated = v;
    }
    style
}

/// `Codec.BOOL` through `NbtOps`.
///
/// `NbtOps` **overrides** `getBooleanValue` to `doubleValue() != 0.0`, so any
/// numeric tag works and there is no byte truncation — a `256` int is `true`,
/// where `DynamicOps`' default `byteValue() != 0` would have made it `false`.
fn nbt_bool(tag: Option<&Nbt>) -> Option<bool> {
    Some(match tag? {
        Nbt::Byte(v) => *v != 0,
        Nbt::Short(v) => *v != 0,
        Nbt::Int(v) => *v != 0,
        Nbt::Long(v) => *v != 0,
        Nbt::Float(v) => *v != 0.0,
        Nbt::Double(v) => *v != 0.0,
        _ => return None,
    })
}

/// `TextColor.parseColor` — a `#` hex literal or one of the sixteen names.
///
/// The hex branch is `Integer.parseInt(_, 16)` with a `0..=0xFFFFFF` range
/// check, which means the digit count is *not* fixed at six: `#f00` is
/// `0x000F00` and `#00FF0000` is a legal `0xFF0000`. Eight digits is the cap
/// because past that `parseInt` overflows an `int` and throws.
///
/// The name branch is a `HashMap` lookup and so is case-**sensitive**.
pub fn parse_color(spec: &str) -> Option<[f32; 3]> {
    if let Some(hex) = spec.strip_prefix('#') {
        if hex.is_empty() || hex.len() > 8 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(hex, 16).ok()?;
        if value > MAX_RGB {
            return None;
        }
        return Some(rgb_f32(value));
    }
    NAMED_COLORS
        .iter()
        .find(|(name, _)| *name == spec)
        .map(|(_, rgb)| rgb_f32(*rgb))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compound(fields: &[(&str, Nbt)]) -> Nbt {
        Nbt::Compound(fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    fn text(s: &str) -> Nbt {
        compound(&[("text", Nbt::String(s.into()))])
    }

    /// Repack a resolved colour so an assertion can name `0xFF_5555` rather
    /// than three float literals.
    fn rgb_to_u32(c: [f32; 3]) -> u32 {
        ((c[0] * 255.0).round() as u32) << 16
            | ((c[1] * 255.0).round() as u32) << 8
            | (c[2] * 255.0).round() as u32
    }

    /// `(text, colour, [bold, italic, underlined, strikethrough, obfuscated])`
    /// for every span — one readable shape for the assertions below.
    fn shape(line: &ChatLine) -> Vec<(&str, u32, [bool; 5])> {
        line.iter()
            .map(|s| {
                (
                    s.text.as_str(),
                    rgb_to_u32(s.color),
                    [s.bold, s.italic, s.underlined, s.strikethrough, s.obfuscated],
                )
            })
            .collect()
    }

    const NONE: [bool; 5] = [false; 5];
    const BOLD: [bool; 5] = [true, false, false, false, false];
    const ITALIC: [bool; 5] = [false, true, false, false, false];
    const WHITE: u32 = 0xFF_FFFF;
    const RED: u32 = 0xFF_5555;
    const GREEN: u32 = 0x55_FF55;

    // ── the colour table ──────────────────────────────────────────────────

    #[test]
    fn the_sixteen_legacy_colours_are_text_colors_exact_values() {
        // `TextColor`'s decimal literals, spot-checked at the values a
        // hand-written table is most likely to fudge.
        assert_eq!(parse_color("dark_blue"), Some(rgb_f32(170)));
        assert_eq!(parse_color("gold"), Some(rgb_f32(16_755_200)));
        assert_eq!(parse_color("gray"), Some(rgb_f32(11_184_810)));
        assert_eq!(parse_color("dark_gray"), Some(rgb_f32(5_592_405)));
        assert_eq!(parse_color("blue"), Some(rgb_f32(5_592_575)));
        assert_eq!(parse_color("light_purple"), Some(rgb_f32(16_733_695)));
        assert_eq!(parse_color("white"), Some(rgb_f32(16_777_215)));
    }

    #[test]
    fn a_legacy_code_and_its_colour_name_resolve_to_the_same_value() {
        // `TextColor.fromLegacyFormat` is an identity mapping over the enum's
        // declaration order, which is why one table can serve both.
        for (i, (name, _)) in NAMED_COLORS.iter().enumerate() {
            let code = char::from_digit(i as u32, 16).unwrap();
            let via_code = shape(&parse_legacy(&format!("§{code}x"), ChatStyle::WHITE))[0].1;
            let via_name = parse_color(name).unwrap();
            assert_eq!(via_code, rgb_to_u32(via_name), "colour {i} ({name})");
        }
    }

    #[test]
    fn a_hash_colour_is_plain_hex_and_not_css_shorthand() {
        assert_eq!(parse_color("#FF0000"), Some(rgb_f32(0xFF_0000)));
        // The trap: `Integer.parseInt("f00", 16)` is 3840, a dark green.
        assert_eq!(parse_color("#f00"), Some(rgb_f32(0x00_0F00)));
        // Leading zeros past six digits are legal so long as the value fits.
        assert_eq!(parse_color("#00FF0000"), Some(rgb_f32(0xFF_0000)));
    }

    #[test]
    fn a_colour_out_of_range_or_not_hex_is_rejected() {
        assert_eq!(parse_color("#1000000"), None); // > 0xFFFFFF
        assert_eq!(parse_color("#FFFFFFFFF"), None); // overflows an int
        assert_eq!(parse_color("#"), None);
        assert_eq!(parse_color("#gg0000"), None);
        assert_eq!(parse_color("puce"), None);
        // `NAMED_COLORS` is a HashMap lookup, so the name is case-sensitive.
        assert_eq!(parse_color("RED"), None);
    }

    // ── legacy `§` codes ──────────────────────────────────────────────────

    #[test]
    fn text_before_any_code_carries_the_base_style() {
        let line = parse_legacy("plain", ChatStyle::plain(rgb_f32(GREEN)));
        assert_eq!(shape(&line), vec![("plain", GREEN, NONE)]);
    }

    #[test]
    fn a_colour_code_starts_a_new_span_in_that_colour() {
        let line = parse_legacy("a§cb", ChatStyle::WHITE);
        assert_eq!(shape(&line), vec![("a", WHITE, NONE), ("b", RED, NONE)]);
    }

    #[test]
    fn a_colour_code_clears_the_format_flags_so_the_order_decides_boldness() {
        // The classic trap. `applyLegacyFormat`'s colour branch assigns
        // `false` to all five flags before setting the colour.
        assert_eq!(shape(&parse_legacy("§c§lX", ChatStyle::WHITE)), vec![("X", RED, BOLD)]);
        assert_eq!(shape(&parse_legacy("§l§cX", ChatStyle::WHITE)), vec![("X", RED, NONE)]);
    }

    #[test]
    fn a_colour_code_clears_a_flag_that_was_already_running() {
        // Same rule, but where the flag is carried across visible text rather
        // than set immediately before — an implementation that special-cased
        // only adjacent codes would pass the test above and fail this one.
        let line = parse_legacy("§la§cb", ChatStyle::WHITE);
        assert_eq!(shape(&line), vec![("a", WHITE, BOLD), ("b", RED, NONE)]);
    }

    #[test]
    fn each_format_code_sets_exactly_its_own_flag() {
        let cases = [
            ('k', [false, false, false, false, true]),
            ('l', BOLD),
            ('m', [false, false, false, true, false]),
            ('n', [false, false, true, false, false]),
            ('o', ITALIC),
        ];
        for (code, flags) in cases {
            let line = parse_legacy(&format!("§{code}x"), ChatStyle::WHITE);
            assert_eq!(shape(&line), vec![("x", WHITE, flags)], "code §{code}");
        }
    }

    #[test]
    fn format_flags_accumulate_until_something_clears_them() {
        let line = parse_legacy("§l§o§nx", ChatStyle::WHITE);
        assert_eq!(shape(&line), vec![("x", WHITE, [true, true, true, false, false])]);
    }

    #[test]
    fn reset_returns_every_field_not_only_the_colour() {
        let line = parse_legacy("§c§lbold§rplain", ChatStyle::WHITE);
        assert_eq!(shape(&line), vec![("bold", RED, BOLD), ("plain", WHITE, NONE)]);
    }

    #[test]
    fn reset_returns_to_the_enclosing_style_rather_than_to_white() {
        // `iterateFormatted` substitutes its `resetStyle` argument, which is
        // seeded with the run's own style — not `Style.EMPTY`.
        let base = ChatStyle { italic: true, ..ChatStyle::plain(rgb_f32(GREEN)) };
        let line = parse_legacy("§ca§rb", base);
        assert_eq!(shape(&line), vec![("a", RED, NONE), ("b", GREEN, ITALIC)]);
    }

    #[test]
    fn a_code_is_matched_case_insensitively() {
        assert_eq!(shape(&parse_legacy("§Cx", ChatStyle::WHITE)), vec![("x", RED, NONE)]);
        assert_eq!(shape(&parse_legacy("§Lx", ChatStyle::WHITE)), vec![("x", WHITE, BOLD)]);
        assert_eq!(
            shape(&parse_legacy("§c§lb§Rp", ChatStyle::WHITE)),
            vec![("b", RED, BOLD), ("p", WHITE, NONE)]
        );
    }

    #[test]
    fn an_unrecognised_code_consumes_both_characters_and_changes_nothing() {
        // `i++` sits outside the `formatting != null` guard, so `§z` is eaten
        // whole. Emitting the letter would be the natural wrong answer.
        assert_eq!(shape(&parse_legacy("a§zb", ChatStyle::WHITE)), vec![("ab", WHITE, NONE)]);
    }

    #[test]
    fn a_doubled_section_sign_is_not_an_escape() {
        // There is no escaping in `iterateFormatted`: `§§` is a `§` followed
        // by the unrecognised code `§`, so the pair is eaten and what looked
        // like the escaped code becomes literal text. This assertion was
        // written the other way round first, expecting `§§c` to yield a
        // literal `§c` or a red run; it does neither.
        assert_eq!(shape(&parse_legacy("§§cx", ChatStyle::WHITE)), vec![("cx", WHITE, NONE)]);
    }

    #[test]
    fn a_trailing_section_sign_is_dropped() {
        assert_eq!(shape(&parse_legacy("hi§", ChatStyle::WHITE)), vec![("hi", WHITE, NONE)]);
    }

    #[test]
    fn no_empty_spans_are_emitted() {
        // Three style changes with no text between them, and a leading code.
        assert_eq!(shape(&parse_legacy("§a§b§cx", ChatStyle::WHITE)), vec![("x", RED, NONE)]);
        assert!(parse_legacy("", ChatStyle::WHITE).is_empty());
        assert!(parse_legacy("§c", ChatStyle::WHITE).is_empty());
    }

    #[test]
    fn a_repeated_code_does_not_split_the_run() {
        // The style is unchanged, so there is nothing to flush.
        assert_eq!(shape(&parse_legacy("§ca§cb", ChatStyle::WHITE)), vec![("ab", RED, NONE)]);
    }

    // ── component trees ───────────────────────────────────────────────────

    #[test]
    fn a_bare_string_component_is_one_span_of_the_base_style() {
        let line = parse_component(&Nbt::String("hi".into()), ChatStyle::plain(rgb_f32(GREEN)), None);
        assert_eq!(shape(&line), vec![("hi", GREEN, NONE)]);
    }

    #[test]
    fn a_component_applies_its_own_colour_and_flags() {
        let tag = compound(&[
            ("text", Nbt::String("x".into())),
            ("color", Nbt::String("red".into())),
            ("bold", Nbt::Byte(1)),
        ]);
        assert_eq!(shape(&parse_component(&tag, ChatStyle::WHITE, None)), vec![("x", RED, BOLD)]);
    }

    #[test]
    fn an_extra_child_inherits_every_unset_field_of_its_parent() {
        let tag = compound(&[
            ("text", Nbt::String("a".into())),
            ("color", Nbt::String("red".into())),
            ("italic", Nbt::Byte(1)),
            ("extra", Nbt::List(vec![text("b")])),
        ]);
        assert_eq!(
            shape(&parse_component(&tag, ChatStyle::WHITE, None)),
            vec![("a", RED, ITALIC), ("b", RED, ITALIC)]
        );
    }

    #[test]
    fn an_extra_child_overrides_only_the_fields_it_sets() {
        let child = compound(&[
            ("text", Nbt::String("b".into())),
            ("color", Nbt::String("green".into())),
        ]);
        let tag = compound(&[
            ("text", Nbt::String("a".into())),
            ("color", Nbt::String("red".into())),
            ("bold", Nbt::Byte(1)),
            ("extra", Nbt::List(vec![child])),
        ]);
        assert_eq!(
            shape(&parse_component(&tag, ChatStyle::WHITE, None)),
            vec![("a", RED, BOLD), ("b", GREEN, BOLD)]
        );
    }

    #[test]
    fn an_explicit_false_beats_an_inherited_true() {
        // `applyTo` is a null check, not a truth check. An implementation
        // written as `if child.bold { style.bold = true }` inherits the
        // parent's `true` here and looks entirely plausible doing it.
        let child = compound(&[("text", Nbt::String("b".into())), ("bold", Nbt::Byte(0))]);
        let tag = compound(&[
            ("text", Nbt::String("a".into())),
            ("bold", Nbt::Byte(1)),
            ("extra", Nbt::List(vec![child])),
        ]);
        assert_eq!(
            shape(&parse_component(&tag, ChatStyle::WHITE, None)),
            vec![("a", WHITE, BOLD), ("b", WHITE, NONE)]
        );
    }

    #[test]
    fn the_parent_text_is_emitted_before_its_children() {
        let tag = compound(&[
            ("text", Nbt::String("head".into())),
            ("extra", Nbt::List(vec![text("tail")])),
        ]);
        assert_eq!(plain_text(&parse_component(&tag, ChatStyle::WHITE, None)), "headtail");
    }

    #[test]
    fn nesting_is_recursive_so_a_grandchild_inherits_through_its_parent() {
        let grandchild = text("c");
        let child = compound(&[
            ("text", Nbt::String("b".into())),
            ("color", Nbt::String("green".into())),
            ("extra", Nbt::List(vec![grandchild])),
        ]);
        let tag = compound(&[
            ("text", Nbt::String("a".into())),
            ("bold", Nbt::Byte(1)),
            ("extra", Nbt::List(vec![child])),
        ]);
        assert_eq!(
            shape(&parse_component(&tag, ChatStyle::WHITE, None)),
            vec![("a", WHITE, BOLD), ("b", GREEN, BOLD), ("c", GREEN, BOLD)]
        );
    }

    #[test]
    fn a_siblings_style_does_not_leak_to_the_sibling_after_it() {
        // Both children resolve against the *parent*, not against each other.
        let red = compound(&[
            ("text", Nbt::String("a".into())),
            ("color", Nbt::String("red".into())),
        ]);
        let tag = compound(&[
            ("text", Nbt::String("".into())),
            ("extra", Nbt::List(vec![red, text("b")])),
        ]);
        assert_eq!(
            shape(&parse_component(&tag, ChatStyle::WHITE, None)),
            vec![("a", RED, NONE), ("b", WHITE, NONE)]
        );
    }

    #[test]
    fn a_top_level_list_makes_the_first_element_the_parent_of_the_rest() {
        // `createFromList` copies element 0 and appends the others to it, so
        // they are siblings of element 0 and inherit its style. Treating the
        // list as a flat sequence loses the red on "b".
        let first = compound(&[
            ("text", Nbt::String("a".into())),
            ("color", Nbt::String("red".into())),
        ]);
        let list = Nbt::List(vec![first, text("b")]);
        assert_eq!(
            shape(&parse_component(&list, ChatStyle::WHITE, None)),
            vec![("a", RED, NONE), ("b", RED, NONE)]
        );
    }

    #[test]
    fn legacy_codes_inside_component_text_are_parsed_too() {
        // `StringDecomposer.iterateFormatted(FormattedText, ...)` runs the
        // literal scan over each fragment's contents, which is how a plugin's
        // `§c` in a component still colours.
        let tag = compound(&[
            ("text", Nbt::String("a§lb".into())),
            ("color", Nbt::String("red".into())),
        ]);
        assert_eq!(
            shape(&parse_component(&tag, ChatStyle::WHITE, None)),
            vec![("a", RED, NONE), ("b", RED, BOLD)]
        );
    }

    #[test]
    fn a_reset_inside_component_text_returns_to_that_components_own_style() {
        // The four-argument overload is called with the fragment's resolved
        // style as *both* arguments, so `§r` lands on green — not on white,
        // and not on `Style.EMPTY`.
        let tag = compound(&[
            ("text", Nbt::String("a§cb§rc".into())),
            ("color", Nbt::String("green".into())),
        ]);
        assert_eq!(
            shape(&parse_component(&tag, ChatStyle::WHITE, None)),
            vec![("a", GREEN, NONE), ("b", RED, NONE), ("c", GREEN, NONE)]
        );
    }

    #[test]
    fn a_translate_component_falls_back_to_its_key_and_keeps_the_style() {
        let tag = compound(&[
            ("translate", Nbt::String("chat.type.text".into())),
            ("color", Nbt::String("red".into())),
        ]);
        assert_eq!(
            shape(&parse_component(&tag, ChatStyle::WHITE, None)),
            vec![("chat.type.text", RED, NONE)]
        );
    }

    // ── translatable resolution (M125) ────────────────────────────────────

    fn lang(pairs: &[(&str, &str)]) -> Language {
        Language::from_map(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        )
    }

    #[test]
    fn a_translate_component_resolves_its_key_and_substitutes_its_arguments() {
        let tag = compound(&[
            ("translate", Nbt::String("multiplayer.player.joined".into())),
            ("with", Nbt::List(vec![Nbt::String("Steve".into())])),
        ]);
        let l = lang(&[("multiplayer.player.joined", "%s joined the game")]);
        assert_eq!(
            plain_text(&parse_component(&tag, ChatStyle::WHITE, Some(&l))),
            "Steve joined the game"
        );
    }

    /// The reason the styled path exists at all: a component argument applies
    /// its **own** style on top of the translatable's, because
    /// `Component.visit` opens with `getStyle().applyTo(parentStyle)`. A
    /// resolution that substituted plain strings would paint the whole line
    /// one colour, and the line would still read correctly — which is what
    /// makes this the rule to pin rather than the substitution.
    #[test]
    fn a_component_argument_keeps_its_own_style_inside_the_template() {
        let sender = compound(&[
            ("text", Nbt::String("Steve".into())),
            ("color", Nbt::String("green".into())),
        ]);
        let tag = compound(&[
            ("translate", Nbt::String("k".into())),
            ("color", Nbt::String("red".into())),
            ("with", Nbt::List(vec![sender])),
        ]);
        let l = lang(&[("k", "<%s> hi")]);
        assert_eq!(
            shape(&parse_component(&tag, ChatStyle::WHITE, Some(&l))),
            vec![("<", RED, NONE), ("Steve", GREEN, NONE), ("> hi", RED, NONE)]
        );
    }

    /// The other half, and the test above cannot see it: `applyTo` is a patch,
    /// so an argument that sets `color` **overrides** whatever parent it is
    /// handed and looks identical whether the template's style reached it or
    /// not. An argument that sets only `bold` inherits the colour, and that is
    /// what makes the composition observable.
    ///
    /// MUTATION: pass `ChatStyle::WHITE` instead of the translatable's style
    /// to a component argument. It survives the test above and dies here. It
    /// is not academic — a `/msg` decoration is GRAY + italic and every
    /// argument in it inherits that.
    #[test]
    fn a_component_argument_inherits_the_templates_style_for_what_it_leaves_unset() {
        let sender = compound(&[
            ("text", Nbt::String("Steve".into())),
            ("bold", Nbt::Byte(1)),
        ]);
        let tag = compound(&[
            ("translate", Nbt::String("k".into())),
            ("color", Nbt::String("red".into())),
            ("with", Nbt::List(vec![sender])),
        ]);
        let l = lang(&[("k", "<%s>")]);
        assert_eq!(
            shape(&parse_component(&tag, ChatStyle::WHITE, Some(&l))),
            vec![("<", RED, NONE), ("Steve", RED, BOLD), (">", RED, NONE)]
        );
    }

    /// A **primitive** argument has no style of its own, so it takes the
    /// template's — `FormattedText.of(String)` forwards the style it is given.
    /// The literal runs on either side must come out identical to it.
    #[test]
    fn a_primitive_argument_takes_the_templates_style() {
        let tag = compound(&[
            ("translate", Nbt::String("k".into())),
            ("color", Nbt::String("red".into())),
            ("with", Nbt::List(vec![Nbt::String("Steve".into())])),
        ]);
        let l = lang(&[("k", "<%s>")]);
        assert_eq!(
            shape(&parse_component(&tag, ChatStyle::WHITE, Some(&l))),
            vec![("<", RED, NONE), ("Steve", RED, NONE), (">", RED, NONE)]
        );
    }

    /// An integral argument is an integer, not a double — the `JavaOps` width
    /// rule, seen from the renderer rather than from `primitive_arg_text`.
    #[test]
    fn an_integer_argument_renders_without_a_decimal_point() {
        let tag = compound(&[
            ("translate", Nbt::String("k".into())),
            (
                "with",
                Nbt::List(vec![Nbt::String("Dirt".into()), Nbt::Int(64)]),
            ),
        ]);
        let l = lang(&[("k", "Gave %s x%s")]);
        assert_eq!(
            plain_text(&parse_component(&tag, ChatStyle::WHITE, Some(&l))),
            "Gave Dirt x64"
        );
    }

    /// A format error renders the resolved **template**, not the key, and the
    /// two are different strings whenever the key resolved. Vanilla's
    /// `decompose` catch replaces the whole part list with one
    /// `FormattedText.of(format)`, so no prefix leaks out before it.
    #[test]
    fn a_format_error_renders_the_template_and_no_partial_prefix() {
        let tag = compound(&[
            ("translate", Nbt::String("k".into())),
            ("with", Nbt::List(vec![Nbt::String("one".into())])),
        ]);
        let l = lang(&[("k", "first %s then %s")]);
        assert_eq!(
            plain_text(&parse_component(&tag, ChatStyle::WHITE, Some(&l))),
            "first %s then %s"
        );
    }

    /// A translatable argument of a translatable resolves too, and the styles
    /// compose through both levels.
    #[test]
    fn a_nested_translatable_argument_resolves() {
        let inner = compound(&[
            ("translate", Nbt::String("inner".into())),
            ("color", Nbt::String("green".into())),
        ]);
        let tag = compound(&[
            ("translate", Nbt::String("outer".into())),
            ("with", Nbt::List(vec![inner])),
        ]);
        let l = lang(&[("outer", "[%s]"), ("inner", "deep")]);
        assert_eq!(
            shape(&parse_component(&tag, ChatStyle::WHITE, Some(&l))),
            vec![("[", WHITE, NONE), ("deep", GREEN, NONE), ("]", WHITE, NONE)]
        );
    }

    /// The `extra` children of a translatable still run, and still run
    /// **after** its contents — `Component.visit` does the contents first.
    #[test]
    fn a_translatables_extra_children_follow_its_resolved_text() {
        let tag = compound(&[
            ("translate", Nbt::String("k".into())),
            ("with", Nbt::List(vec![Nbt::String("X".into())])),
            ("extra", Nbt::List(vec![text("!")])),
        ]);
        let l = lang(&[("k", "<%s>")]);
        assert_eq!(
            plain_text(&parse_component(&tag, ChatStyle::WHITE, Some(&l))),
            "<X>!"
        );
    }

    /// The work bound. A template that names the same argument twice doubles
    /// its subtree, so twenty levels of it is a million nodes and forty is a
    /// trillion — and the fan-out is server-chosen, because `fallback` is a
    /// string the server sends. Vanilla has no bound here; this one returns a
    /// truncated line in bounded time instead of hanging the frame.
    ///
    /// The assertion is on the **step budget being spent**, not on the text:
    /// a test that only checked "it returned" would pass just as well against
    /// an implementation that took an hour to do it.
    #[test]
    fn a_self_duplicating_translatable_is_bounded() {
        // Forty nested levels of `%1$s%1$s`. Unbounded, this is 2^40 spans.
        let mut node = text("x");
        for _ in 0..40 {
            node = compound(&[
                ("translate", Nbt::String("dup".into())),
                ("with", Nbt::List(vec![node])),
            ]);
        }
        let l = lang(&[("dup", "%1$s%1$s")]);
        let line = parse_component(&node, ChatStyle::WHITE, Some(&l));
        assert!(
            line.len() < MAX_COMPONENT_STEPS as usize,
            "bounded by the step budget, got {} spans",
            line.len()
        );
        // And it really did hit the wall rather than terminating early for
        // some other reason — an unbounded walk would owe 2^40 leaves.
        assert!(line.len() > 1000, "the walk did do the work it could afford");
    }

    #[test]
    fn text_wins_over_translate_when_a_component_carries_both() {
        // The contents codec is an either, so this cannot happen from vanilla;
        // matching `component_wire::nbt_text`'s preference keeps the two
        // readers from disagreeing on a malformed input.
        let tag = compound(&[
            ("text", Nbt::String("literal".into())),
            ("translate", Nbt::String("key".into())),
        ]);
        assert_eq!(plain_text(&parse_component(&tag, ChatStyle::WHITE, None)), "literal");
    }

    #[test]
    fn a_boolean_flag_is_any_nonzero_numeric_tag() {
        // `NbtOps.getBooleanValue` is `doubleValue() != 0.0`, so there is no
        // byte truncation: 256 is `true`, where the `DynamicOps` default of
        // `byteValue() != 0` would have made it `false`.
        for tag in [Nbt::Byte(1), Nbt::Short(1), Nbt::Int(256), Nbt::Float(0.5)] {
            let c = compound(&[("text", Nbt::String("x".into())), ("bold", tag.clone())]);
            assert_eq!(shape(&parse_component(&c, ChatStyle::WHITE, None)), vec![("x", WHITE, BOLD)]);
        }
        for tag in [Nbt::Byte(0), Nbt::Int(0), Nbt::Double(0.0)] {
            let c = compound(&[("text", Nbt::String("x".into())), ("bold", tag.clone())]);
            assert_eq!(shape(&parse_component(&c, ChatStyle::WHITE, None)), vec![("x", WHITE, NONE)]);
        }
    }

    #[test]
    fn an_unparseable_colour_is_treated_as_absent_so_the_parent_colour_survives() {
        let child = compound(&[
            ("text", Nbt::String("b".into())),
            ("color", Nbt::String("chartreuse".into())),
        ]);
        let tag = compound(&[
            ("text", Nbt::String("a".into())),
            ("color", Nbt::String("red".into())),
            ("extra", Nbt::List(vec![child])),
        ]);
        assert_eq!(
            shape(&parse_component(&tag, ChatStyle::WHITE, None)),
            vec![("a", RED, NONE), ("b", RED, NONE)]
        );
    }

    #[test]
    fn a_component_with_no_contents_yields_only_its_children() {
        let tag = compound(&[("extra", Nbt::List(vec![text("only")]))]);
        assert_eq!(plain_text(&parse_component(&tag, ChatStyle::WHITE, None)), "only");
    }

    #[test]
    fn an_unhandled_contents_kind_yields_nothing_rather_than_a_placeholder() {
        // `score` / `selector` / `nbt` contents need a resolver this crate
        // does not have. An empty line is recoverable; an invented one is not.
        let tag = compound(&[("selector", Nbt::String("@p".into()))]);
        assert!(parse_component(&tag, ChatStyle::WHITE, None).is_empty());
        assert!(parse_component(&Nbt::Int(7), ChatStyle::WHITE, None).is_empty());
        assert!(parse_component(&Nbt::List(vec![]), ChatStyle::WHITE, None).is_empty());
    }

    #[test]
    fn a_span_round_trips_through_its_own_style() {
        let style = ChatStyle { obfuscated: true, ..ChatStyle::plain(rgb_f32(RED)) };
        assert_eq!(style.span("x").style(), style);
    }
}
