//! `TranslatableContents` — resolving a `translate` component against the
//! language table (M125).
//!
//! # What was wrong before this module
//!
//! Both of Rewo's component walkers — [`crate::component_wire::nbt_text`] for
//! plain text and [`crate::chat_style::parse_component`] for spans — met a
//! `translate` component by emitting **the key** and never looking at `with`
//! at all. So a real server's `multiplayer.player.joined` rendered as the
//! literal string `multiplayer.player.joined`, with the player's name dropped;
//! so did every death message and every command's feedback. That was a
//! deliberate, documented fallback ("visibly wrong beats invisibly wrong") and
//! it was the right call while there was no table to resolve against. There is
//! one now — `rewo_data::lang::Language`, loaded since M54.
//!
//! # The algorithm, and where each rule comes from
//!
//! `TranslatableContents.decompose` looks the key up and hands the template to
//! `decomposeTemplate`, which is transcribed once in
//! [`rewo_data::lang::decompose_template`] and shared by both walkers here.
//! What this module adds is the three things around it:
//!
//! 1. **Which template.** `fallback != null ? getOrDefault(key, fallback) :
//!    getOrDefault(key)`. The single-argument `getOrDefault` defaults to *the
//!    key itself*, so a missing key with no fallback still renders something —
//!    and that is exactly the pre-M125 behaviour, which is why passing no
//!    language table reproduces it rather than blanking the line.
//!
//! 2. **What an argument is.** `ARG_CODEC` is
//!    `Codec.either(PRIMITIVE_ARG_CODEC, ComponentSerialization.CODEC)` and
//!    `either` takes the **first** success, so the primitive reading wins
//!    whenever `isAllowedPrimitiveArgument` accepts it — `Number || Boolean ||
//!    String`. A string argument is therefore a *primitive*, not a literal
//!    component; the two render identically, because `FormattedText.of(String)`
//!    passes the caller's style through exactly as an unstyled literal would.
//!    A compound or a list is not a primitive and is parsed as a component.
//!
//! 3. **What a numeric argument's text is.** `getArgument` ends in
//!    `arg.toString()`, so the answer depends on the boxed Java type — and
//!    that is decided two layers down, by `NbtOps.convertTo` dispatching per
//!    tag (`ByteTag -> createByte`, `IntTag -> createInt`, …) into
//!    `JavaOps`, whose `createByte`/`createInt`/`createFloat`/… each return
//!    the boxed value unchanged. **The width survives**, so an `IntTag(3)`
//!    renders `3` and not `3.0`. That was read out of the decompiled
//!    `com.mojang.serialization.JavaOps` in `datafixerupper-10.0.21.jar` — the
//!    version 26.2's own manifest names — rather than assumed, because the
//!    plausible alternative (everything through `createNumeric`, which
//!    `NbtOps` implements as `DoubleTag.valueOf`) would put `.0` on every
//!    count in every command's feedback.
//!
//! # The two failure modes, which are not the same
//!
//! A **format** error — an unsupported specifier, a stray `%`, an argument
//! index past the end — is `TranslatableFormatException`, and `decompose`
//! catches it and answers with `[FormattedText.of(format)]`: the *looked-up
//! template*, unsubstituted, as one part. Not the key, and not a blank line.
//! Only when the key is missing do the template and the key coincide.
//!
//! A **decode** error is different and is a deliberate deviation here. An
//! argument that is neither a primitive nor a parseable component (an NBT
//! int-array, say) fails both halves of `Codec.either`, which in vanilla fails
//! the enclosing component's decode and therefore the packet. Rewo's walkers
//! are lenient throughout — `nbt_text` skips what it cannot read rather than
//! failing a packet — so such an argument contributes nothing and the rest of
//! the line still renders. Stated rather than hidden: it is the existing
//! convention of these two walkers, not a new one.
//!
//! # Ground truth
//!
//! - `net/minecraft/network/chat/contents/TranslatableContents.java` —
//!   `decompose`, `decomposeTemplate`, `getArgument`, `ARG_CODEC`,
//!   `isAllowedPrimitiveArgument`, `MAP_CODEC`
//! - `net/minecraft/network/chat/Component.java` — `visit(consumer, style)`,
//!   which is where an argument component's own style composes onto the
//!   translatable's
//! - `net/minecraft/locale/Language.java` — `getOrDefault`'s two arities
//! - `net/minecraft/nbt/NbtOps.java` — `convertTo`'s per-tag dispatch
//! - `com/mojang/serialization/JavaOps` (decompiled from
//!   `datafixerupper-10.0.21.jar`) — `createByte`/`createInt`/… returning the
//!   boxed value
//! - `java.lang.Double.toString` / `Float.toString` — in no decompile this
//!   project holds, so graded against a real JDK 25 by
//!   `tools/java_tostring_oracle/`

use rewo_data::lang::{decompose_template, Language, Part};
use rewo_proto::nbt::Nbt;

/// A `translate` component's inputs, with the template already looked up.
pub struct Translatable<'a> {
    /// The looked-up format string. Borrowed from the language table, from
    /// the component's own `fallback`, or from its key — whichever
    /// `getOrDefault` selected.
    pub template: &'a str,
    /// `with`, or empty. Absent and present-but-empty are the same to
    /// `adjustArgs`, which maps both to `NO_ARGS`.
    pub args: &'a [Nbt],
}

/// Read a component's `translate` / `fallback` / `with` and resolve the
/// template, or `None` if the tag is not a translatable component.
///
/// `lang` is `Option` because Rewo resolves components in two places and only
/// one of them has a table: the app owns `BakedAssets::lang`, while a packet
/// decode inside `PlaySession` has nothing. `None` selects the same template
/// the pre-M125 walkers emitted — `getOrDefault(key)`'s key-as-default — so a
/// call site with no table renders exactly what it rendered before.
pub fn translatable<'a>(tag: &'a Nbt, lang: Option<&'a Language>) -> Option<Translatable<'a>> {
    let key = tag.get("translate").and_then(Nbt::as_str)?;
    let fallback = tag.get("fallback").and_then(Nbt::as_str);
    let template = match (lang, fallback) {
        (Some(l), Some(f)) => l.get_or_default(key, f),
        (Some(l), None) => l.or_key(key),
        // With no table every key is missing, which is what both arities of
        // `getOrDefault` reduce to: the fallback if there is one, else the key.
        (None, Some(f)) => f,
        (None, None) => key,
    };
    let args: &[Nbt] = match tag.get("with") {
        Some(Nbt::List(items)) => items,
        _ => &[],
    };
    Some(Translatable { template, args })
}

impl Translatable<'_> {
    /// `decomposeTemplate`, or `None` for the `TranslatableFormatException`
    /// whose whole-line fallback is [`Self::template`].
    pub fn parts(&self) -> Option<Vec<Part<'_>>> {
        decompose_template(self.template, self.args.len())
    }
}

/// `getArgument`'s `arg.toString()`, for the arguments `Codec.either` resolves
/// to a **primitive**.
///
/// `None` means "not a primitive", which sends the caller to the component
/// branch — so this function is the `isAllowedPrimitiveArgument` test and the
/// stringification at once, and they cannot disagree.
pub fn primitive_arg_text(tag: &Nbt) -> Option<String> {
    Some(match tag {
        Nbt::String(s) => s.clone(),
        // Every integral width boxes to its own `Number` and stringifies as a
        // plain integer. There is no `Boolean` case: NBT has no boolean tag,
        // so a JSON `true` arrives as `ByteTag(1)` and renders `1`, not `true`.
        Nbt::Byte(v) => v.to_string(),
        Nbt::Short(v) => v.to_string(),
        Nbt::Int(v) => v.to_string(),
        Nbt::Long(v) => v.to_string(),
        Nbt::Float(v) => java_float_string(*v),
        Nbt::Double(v) => java_double_string(*v),
        _ => return None,
    })
}

/// `java.lang.Double.toString`.
///
/// Two rules, both graded against a real JDK by `tools/java_tostring_oracle/`:
///
/// * The plain form is used iff `1e-3 <= |v| < 1e7` — **inclusive** at the
///   bottom and **exclusive** at the top. `1.0E-3` prints `0.001` and `1.0E7`
///   prints `1.0E7`, so guessing the same strictness at both ends is wrong at
///   one of them whichever you guess.
/// * There is always at least one digit on each side of the point, in both
///   forms. That is what makes `1.0` print `1.0` where Rust's `{}` prints `1`,
///   and it applies to the mantissa too: `1.0E20`, not `1E20`.
///
/// The *digits* are delegated to Rust's shortest-round-trip formatting, which
/// agrees with Java 19+ (both emit the shortest decimal that round-trips).
/// A JVM older than that used a non-shortest algorithm and would disagree on
/// some values — the oracle exists to make a JDK bump say so.
pub fn java_double_string(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let mag = v.abs();
    // `v == 0.0` is true for -0.0 as well, and it takes the plain branch:
    // `Double.toString(-0.0)` is `-0.0`, sign preserved.
    if v == 0.0 || (1e-3..1e7).contains(&mag) {
        with_point(format!("{v}"))
    } else {
        java_scientific(format!("{v:e}"))
    }
}

/// `java.lang.Float.toString`. The thresholds are the same two numbers; the
/// digits are the shortest that round-trip **as an `f32`**, which is why this
/// formats the `f32` rather than widening it (`0.1f32 as f64` is
/// `0.100000001490116…`, and printing that would be sixteen wrong digits).
pub fn java_float_string(v: f32) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let mag = v.abs();
    if v == 0.0 || (1e-3..1e7).contains(&mag) {
        with_point(format!("{v}"))
    } else {
        java_scientific(format!("{v:e}"))
    }
}

/// Give a plain decimal its mandatory fractional digit: `1` -> `1.0`.
fn with_point(s: String) -> String {
    if s.contains('.') {
        s
    } else {
        s + ".0"
    }
}

/// Rust's `1e20` / `9.999e-4` into Java's `1.0E20` / `9.999E-4`.
///
/// Rust normalises the mantissa to one digit before the point exactly as Java
/// does, so the only differences are the mandatory fractional digit and the
/// capital `E`. Neither language writes a `+` on a positive exponent.
fn java_scientific(s: String) -> String {
    match s.split_once('e') {
        Some((mantissa, exp)) => format!("{}E{}", with_point(mantissa.to_string()), exp),
        None => with_point(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compound(pairs: &[(&str, Nbt)]) -> Nbt {
        Nbt::Compound(pairs.iter().map(|(k, v)| ((*k).into(), v.clone())).collect())
    }

    fn lang(pairs: &[(&str, &str)]) -> Language {
        Language::from_map(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        )
    }

    #[test]
    fn a_non_translatable_tag_is_not_one() {
        assert!(translatable(&Nbt::String("hi".into()), None).is_none());
        assert!(translatable(&compound(&[("text", Nbt::String("hi".into()))]), None).is_none());
    }

    /// `getOrDefault(key)` — the arity whose default is the key. This is the
    /// pre-M125 rendering, and it has to survive so that a call site with no
    /// table is unchanged rather than blank.
    #[test]
    fn a_missing_key_resolves_to_the_key_itself() {
        let tag = compound(&[("translate", Nbt::String("a.b".into()))]);
        assert_eq!(translatable(&tag, None).unwrap().template, "a.b");
        assert_eq!(
            translatable(&tag, Some(&lang(&[("x", "y")]))).unwrap().template,
            "a.b"
        );
    }

    /// `getOrDefault(key, fallback)` — and note the order: a key that IS in
    /// the table beats the fallback. Reading the field name as "prefer this"
    /// inverts it.
    #[test]
    fn a_fallback_loses_to_a_present_key_and_wins_over_a_missing_one() {
        let tag = compound(&[
            ("translate", Nbt::String("a.b".into())),
            ("fallback", Nbt::String("Fallback".into())),
        ]);
        let l = lang(&[("a.b", "Translated")]);
        assert_eq!(translatable(&tag, Some(&l)).unwrap().template, "Translated");
        assert_eq!(
            translatable(&tag, Some(&lang(&[]))).unwrap().template,
            "Fallback"
        );
        // No table at all is the same as a table without the key.
        assert_eq!(translatable(&tag, None).unwrap().template, "Fallback");
    }

    /// `adjustArgs` maps an absent `with` and an empty one to the same
    /// `NO_ARGS`, so a template with a specifier fails identically either way.
    #[test]
    fn an_absent_with_and_an_empty_with_are_the_same() {
        let absent = compound(&[("translate", Nbt::String("k".into()))]);
        let empty = compound(&[
            ("translate", Nbt::String("k".into())),
            ("with", Nbt::List(vec![])),
        ]);
        assert!(translatable(&absent, None).unwrap().args.is_empty());
        assert!(translatable(&empty, None).unwrap().args.is_empty());
    }

    /// A `with` that is not a list is not an argument list. Vanilla's
    /// `listOf()` would fail the decode; the lenient reading here is no args,
    /// which makes the template's own specifiers collapse it to raw text
    /// rather than substituting something invented.
    #[test]
    fn a_with_that_is_not_a_list_yields_no_arguments() {
        let tag = compound(&[
            ("translate", Nbt::String("k".into())),
            ("with", Nbt::String("Steve".into())),
        ]);
        assert!(translatable(&tag, None).unwrap().args.is_empty());
    }

    #[test]
    fn the_real_join_message_resolves_and_decomposes() {
        let tag = compound(&[
            ("translate", Nbt::String("multiplayer.player.joined".into())),
            ("with", Nbt::List(vec![Nbt::String("Steve".into())])),
        ]);
        let l = lang(&[("multiplayer.player.joined", "%s joined the game")]);
        let t = translatable(&tag, Some(&l)).unwrap();
        assert_eq!(t.template, "%s joined the game");
        assert_eq!(
            t.parts(),
            Some(vec![Part::Arg(0), Part::Literal(" joined the game")])
        );
    }

    /// A format error keeps the **template**, not the key — and the two differ
    /// exactly when the key resolved.
    #[test]
    fn a_format_error_falls_back_to_the_template_not_the_key() {
        let tag = compound(&[
            ("translate", Nbt::String("k".into())),
            ("with", Nbt::List(vec![Nbt::String("one".into())])),
        ]);
        let l = lang(&[("k", "%s and %s")]);
        let t = translatable(&tag, Some(&l)).unwrap();
        assert_eq!(t.parts(), None);
        assert_eq!(t.template, "%s and %s");
        assert_ne!(t.template, "k");
    }

    // -- `getArgument`'s `toString`, against `tools/java_tostring_oracle/` ---

    #[test]
    fn a_string_argument_is_a_primitive() {
        assert_eq!(
            primitive_arg_text(&Nbt::String("Steve".into())).as_deref(),
            Some("Steve")
        );
    }

    /// A compound or a list is not a primitive, so `Codec.either` falls to the
    /// component branch. `None` here IS that dispatch.
    #[test]
    fn a_component_argument_is_not_a_primitive() {
        assert_eq!(primitive_arg_text(&compound(&[("text", Nbt::String("x".into()))])), None);
        assert_eq!(primitive_arg_text(&Nbt::List(vec![])), None);
        assert_eq!(primitive_arg_text(&Nbt::IntArray(vec![1])), None);
    }

    /// The integral widths, and the fact that they stay integral. If
    /// `NbtOps.convertTo` had gone through `createNumeric` these would all be
    /// `Double`s and every count in every command's feedback would carry a
    /// `.0`.
    #[test]
    fn integral_arguments_keep_their_width_and_render_as_integers() {
        assert_eq!(primitive_arg_text(&Nbt::Byte(1)).as_deref(), Some("1"));
        assert_eq!(primitive_arg_text(&Nbt::Byte(-128)).as_deref(), Some("-128"));
        assert_eq!(primitive_arg_text(&Nbt::Short(32767)).as_deref(), Some("32767"));
        assert_eq!(primitive_arg_text(&Nbt::Int(64)).as_deref(), Some("64"));
        assert_eq!(
            primitive_arg_text(&Nbt::Int(-2147483648)).as_deref(),
            Some("-2147483648")
        );
        assert_eq!(
            primitive_arg_text(&Nbt::Long(9223372036854775807)).as_deref(),
            Some("9223372036854775807")
        );
    }

    /// Every row is a line of `tools/java_tostring_oracle/`'s output, run on
    /// JDK 25.0.3. Nothing here is derived from the Rust implementation.
    #[test]
    fn double_arguments_match_the_jdk() {
        for (v, want) in [
            (0.0f64, "0.0"),
            (-0.0f64, "-0.0"),
            (1.0, "1.0"),
            (-1.0, "-1.0"),
            (3.0, "3.0"),
            (0.5, "0.5"),
            (100.0, "100.0"),
            // The low threshold is inclusive: 1e-3 is plain.
            (1.0e-3, "0.001"),
            // One ulp of magnitude below it is not.
            (9.999e-4, "9.999E-4"),
            // The high threshold is exclusive: 1e7 is scientific.
            (1.0e7, "1.0E7"),
            (9999999.0, "9999999.0"),
            (1.0e-4, "1.0E-4"),
            (1.0e8, "1.0E8"),
            (1.23e-5, "1.23E-5"),
            (1.0e20, "1.0E20"),
            (0.1, "0.1"),
            (1.0 / 3.0, "0.3333333333333333"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
        ] {
            assert_eq!(java_double_string(v), want, "Double.toString({v:?})");
            assert_eq!(
                primitive_arg_text(&Nbt::Double(v)).as_deref(),
                Some(want),
                "as an argument"
            );
        }
    }

    /// Same, for `Float.toString`. `0.1f` is the row that proves the digits
    /// come from the `f32` and not from a widened `f64`.
    #[test]
    fn float_arguments_match_the_jdk() {
        for (v, want) in [
            (0.0f32, "0.0"),
            (1.0, "1.0"),
            (1.5, "1.5"),
            (0.1, "0.1"),
            (1.0e7, "1.0E7"),
            (9999999.0, "9999999.0"),
            (1.0e-3, "0.001"),
            (9.999e-4, "9.999E-4"),
            (3.4028235e38, "3.4028235E38"),
        ] {
            assert_eq!(java_float_string(v), want, "Float.toString({v:?})");
        }
    }
}
