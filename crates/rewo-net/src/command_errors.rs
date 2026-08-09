//! `BuiltInExceptions`' literals and `getExceptionMessage` — the red-handed
//! half of the command field (M134).
//!
//! M117 shipped the usage box and modelled this as a **gate**: where vanilla
//! prints "Unknown command at position 1: /<--[HERE]" it printed nothing, on
//! the reasoning that listing what may come next under a command that is
//! already wrong is worse than silence. That was right about the gate and
//! wrong about the silence — the message is the most visible thing the box
//! ever shows, and typing a command that does not exist is how most players
//! meet it.
//!
//! # It is not a third panel — it is a member of the usage box
//!
//! `updateUsageInfo` writes both into the **same** `commandUsage` list, and
//! `extractUsage` draws that list with one loop, one fill and one geometry.
//! The exclusivity is `if (this.commandUsage.isEmpty())` guarding the usage
//! entries, so it runs one way only: an exception suppresses the usage lines,
//! and the usage lines never suppress an exception. `extractRenderState` then
//! hides the whole box behind the suggestion popup when there is one, exactly
//! as it did before this milestone. So there are two exclusions, not three,
//! and they are at different levels.
//!
//! # The usage lines are GREY and the message is WHITE
//!
//! ```java
//! graphics.text(this.font, line, this.commandUsagePosition, lineY + 2, -1);
//! ```
//!
//! One colour argument for every line — and `Font`'s `getTextColor` uses it
//! only `if (textColor == null)`, i.e. as a *default* the style overrides.
//! `fillNodeUsage` bakes `USAGE_FORMAT` (`ChatFormatting.GRAY`) into its
//! lines, so they draw grey; `ComponentUtils.fromMessage(e.getRawMessage())`
//! is `Component.literal(...)` with **no style at all** — a brigadier
//! `LiteralMessage` is not a `Component` — so the message takes the `-1` and
//! draws white. A reader who colours the message to match the box gets grey,
//! and one who matches the unparsed tail gets red; both are wrong, and the
//! difference is only observable once both kinds of line exist.
//!
//! # `getContext` never returns null here, so the wrapper always applies
//!
//! ```java
//! public String getContext() {
//!    if (this.input != null && this.cursor >= 0) { ... } else return null;
//! }
//! ```
//!
//! Every exception a client parse produces went through `createWithContext`,
//! which supplies both — so `getExceptionMessage`'s null branch is
//! unreachable from this screen and every message is wrapped in
//! `command.context.parse_error`. The excerpt is the **ten code units before
//! the cursor**, `...`-prefixed when there are more, then the literal
//! `<--[HERE]`. Note `command.context.here` exists in `en_us.json` and is
//! **not** what brigadier appends: brigadier is a library and has no
//! translation table, so the marker is a hard-coded string and the lang key
//! belongs to a different (server-side) message.
//!
//! # Which of the twenty-seven literals a client can actually produce
//!
//! `BuiltInExceptions` declares 27. From `CommandDispatcher.parse` driven by
//! [`crate::dispatcher`]:
//!
//! * **Reachable**: the eight numeric range errors, the four `Invalid …`
//!   and five `Expected …` reader errors, `Unclosed quoted string`,
//!   `Invalid escape sequence`, `Expected whitespace to end one argument`,
//!   `Unknown command` and `Incorrect argument for command`.
//! * **Unreachable, and each for its own reason**:
//!   * `Expected quote to start a string` — thrown by `readQuotedString`,
//!     which nothing on the parse path calls; `readString` falls back to the
//!     unquoted reader instead.
//!   * `Expected '<symbol>'` — `expect()` is used by Minecraft's own argument
//!     types, whose Rewo equivalents ([`crate::selector`],
//!     [`crate::snbt_grammar`], [`crate::block_item`]) report their own
//!     refusal instead.
//!   * `Could not parse command: …` — wraps a `RuntimeException` escaping
//!     `child.parse`. Rust has no such escape.
//!   * **`Expected literal …`** — the interesting one. `getRelevantNodes`
//!     returns a `LiteralCommandNode` only when the next word **equals its
//!     name**, and `LiteralCommandNode.parse` on that word cannot then fail.
//!     So `literalIncorrect` is never recorded, which makes `updateUsageInfo`'s
//!     `literals++` counter and the `dispatcherUnknownArgument` line it
//!     substitutes **dead code in vanilla**. Both are transcribed anyway —
//!     they are cheap, and a tree with a literal whose name contains a space
//!     would be the only way to disturb the argument — but no test here
//!     claims to have exercised them, because exercising them would require
//!     building a parse vanilla cannot build.
//!
//! # Ground truth
//!
//! - `com/mojang/brigadier/exceptions/BuiltInExceptions.java` — the 27
//!   literals, verbatim
//! - `com/mojang/brigadier/exceptions/CommandSyntaxException.java` —
//!   `getContext`, `CONTEXT_AMOUNT`
//! - `com/mojang/brigadier/tree/CommandNode.java` — `getRelevantNodes`
//! - `net/minecraft/client/gui/components/CommandSuggestions.java` —
//!   `getExceptionMessage`, `updateUsageInfo`, `extractUsage`
//! - `net/minecraft/commands/Commands.java` — `getParseException`
//! - `net/minecraft/client/gui/Font.java` — `getTextColor`, which is why the
//!   `-1` is a default and not an override
//! - `assets/minecraft/lang/en_us.json` — `command.context.parse_error` is
//!   `"%s at position %s: %s"`, and survives `deprecated.json` untouched

use crate::dispatcher::{ParseResults, ReaderError};
use rewo_data::lang::{decompose_template, Language, Part};

/// `CommandSyntaxException.CONTEXT_AMOUNT`.
pub const CONTEXT_AMOUNT: usize = 10;

/// The marker `getContext` appends. A brigadier literal, not a lang key.
pub const HERE: &str = "<--[HERE]";

/// The key `getExceptionMessage` wraps every contextual message in.
pub const PARSE_ERROR_KEY: &str = "command.context.parse_error";

/// One of `BuiltInExceptions`' types, as far as a client parse can reach
/// them.
///
/// The two dispatcher-level ones are separate variants rather than
/// [`ReaderError`] members because [`crate::dispatcher`] cannot raise them:
/// they are minted by `Commands.getParseException` and by `updateUsageInfo`'s
/// literal-count branch, both of which sit above the parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltIn<'a> {
    Reader(&'a ReaderError),
    /// `dispatcherUnknownCommand` — "Unknown command".
    UnknownCommand,
    /// `dispatcherUnknownArgument` — "Incorrect argument for command".
    UnknownArgument,
}

/// `CommandExceptionType`'s message, with its arguments already substituted.
///
/// `None` means Rewo has no vanilla literal to print: only
/// [`ReaderError::UnknownArgumentType`], which is this client's own
/// "no reader for that type / the reader refused" and has no counterpart in
/// `BuiltInExceptions`, and [`ReaderError::LiteralIncorrect`], which vanilla
/// counts rather than prints (see the module docs) and which Rewo's variant
/// does not carry the literal name for.
pub fn message(e: BuiltIn<'_>) -> Option<String> {
    let r = match e {
        BuiltIn::UnknownCommand => return Some("Unknown command".to_string()),
        BuiltIn::UnknownArgument => return Some("Incorrect argument for command".to_string()),
        BuiltIn::Reader(r) => r,
    };
    Some(match r {
        ReaderError::ExpectedInt => "Expected integer".to_string(),
        ReaderError::InvalidInt(v) => format!("Invalid integer '{v}'"),
        ReaderError::ExpectedLong => "Expected long".to_string(),
        ReaderError::InvalidLong(v) => format!("Invalid long '{v}'"),
        ReaderError::ExpectedDouble => "Expected double".to_string(),
        ReaderError::InvalidDouble(v) => format!("Invalid double '{v}'"),
        ReaderError::ExpectedFloat => "Expected float".to_string(),
        ReaderError::InvalidFloat(v) => format!("Invalid float '{v}'"),
        ReaderError::ExpectedBool => "Expected bool".to_string(),
        ReaderError::InvalidBool(v) => {
            format!("Invalid bool, expected true or false but found '{v}'")
        }
        ReaderError::InvalidEscape(c) => {
            format!("Invalid escape sequence '{c}' in quoted string")
        }
        ReaderError::ExpectedEndOfQuote => "Unclosed quoted string".to_string(),
        ReaderError::OutOfRange {
            kind,
            too_high,
            found,
            bound,
        } => {
            // `"Integer must not be less than " + min + ", found " + found` —
            // the BOUND is printed first and the offending value second,
            // which is the reverse of the order they are handed in.
            let cmp = if *too_high { "more" } else { "less" };
            format!("{} must not be {cmp} than {bound}, found {found}", kind.name())
        }
        ReaderError::ExpectedArgumentSeparator => {
            "Expected whitespace to end one argument, but found trailing data".to_string()
        }
        ReaderError::LiteralIncorrect | ReaderError::UnknownArgumentType => return None,
    })
}

/// `CommandSyntaxException.getContext`.
///
/// ```java
/// int cursor = Math.min(this.input.length(), this.cursor);
/// if (cursor > 10) builder.append("...");
/// builder.append(this.input.substring(Math.max(0, cursor - 10), cursor));
/// builder.append("<--[HERE]");
/// ```
///
/// `input` is UTF-16 because every cursor in this subsystem is a Java string
/// index. A ten-unit window can therefore cut a surrogate pair in half;
/// `from_utf16_lossy` yields U+FFFD, which is what an unpaired surrogate
/// renders as in vanilla too.
///
/// Always `Some` for a client parse — the `None` is `getContext`'s
/// `input == null || cursor < 0` branch, which `createWithContext` cannot
/// produce and which only the two-argument `CommandSyntaxException`
/// constructor reaches.
pub fn context(input: &[u16], cursor: usize) -> String {
    let cursor = cursor.min(input.len());
    let mut out = String::new();
    if cursor > CONTEXT_AMOUNT {
        out.push_str("...");
    }
    out.push_str(&String::from_utf16_lossy(
        &input[cursor.saturating_sub(CONTEXT_AMOUNT)..cursor],
    ));
    out.push_str(HERE);
    out
}

/// `CommandSuggestions.getExceptionMessage`.
///
/// ```java
/// Component message = ComponentUtils.fromMessage(e.getRawMessage());
/// String context = e.getContext();
/// return context == null ? message.getVisualOrderText()
///    : Component.translatable("command.context.parse_error", message, e.getCursor(), context)
///               .getVisualOrderText();
/// ```
///
/// A plain `String` rather than a span list, and that is faithful rather than
/// a simplification: every part of it is unstyled. The template is unstyled,
/// the message is an unstyled literal (see the module docs), the cursor is an
/// `Integer` whose `toString` is a bare decimal, and the excerpt is a raw
/// `String`. A Minecraft-defined exception *could* carry a styled `Component`
/// as its `Message`, but none of them is reachable from a client-side parse —
/// `ArgKind` never runs one.
///
/// `None` when the type has no printable literal — see [`message`].
pub fn exception_message(
    e: BuiltIn<'_>,
    input: &[u16],
    cursor: usize,
    lang: Option<&Language>,
) -> Option<String> {
    let msg = message(e)?;
    let ctx = context(input, cursor);
    let args = [msg.as_str(), &cursor.to_string(), ctx.as_str()];
    let key = PARSE_ERROR_KEY;
    let template = match lang {
        Some(l) => l.or_key(key),
        // No table is the pre-M54 state and also `getOrDefault(key)`'s own
        // default: the key itself, which decomposes to one literal part and
        // therefore prints the key. Visibly wrong beats invisibly wrong.
        None => key,
    };
    Some(match decompose_template(template, args.len()) {
        Some(parts) => parts
            .iter()
            .map(|p| match p {
                Part::Literal(s) => *s,
                Part::Arg(i) => args[*i],
            })
            .collect(),
        // `TranslatableFormatException` — `decompose` answers with the
        // looked-up template, unsubstituted.
        None => template.to_string(),
    })
}

/// `Commands.getParseException`.
///
/// ```java
/// if (!parse.getReader().canRead()) return null;
/// else if (parse.getExceptions().size() == 1) return the one;
/// else return parse.getContext().getRange().isEmpty()
///    ? dispatcherUnknownCommand().createWithContext(parse.getReader())
///    : dispatcherUnknownArgument().createWithContext(parse.getReader());
/// ```
///
/// The range consulted is the **root** context's, not the deepest child's, so
/// "nothing at all was consumed" is what separates *Unknown command* from
/// *Incorrect argument for command*. Returns the type and the cursor its
/// `createWithContext` would have captured.
pub fn parse_exception(parse: &ParseResults) -> Option<(BuiltIn<'_>, usize)> {
    if !parse.reader.can_read() {
        return None;
    }
    if parse.errors.len() == 1 {
        let e = &parse.errors[0];
        return Some((BuiltIn::Reader(&e.error), e.cursor));
    }
    let which = if parse.context.range.is_empty() {
        BuiltIn::UnknownCommand
    } else {
        BuiltIn::UnknownArgument
    };
    Some((which, parse.reader.cursor()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{ArgumentProps, CommandNode, CommandTree, NodeKind, StringType};
    use crate::dispatcher::{parse, CommandCtx, NumKind};
    use std::collections::HashMap;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// Just the one key this module reads, so a test cannot accidentally
    /// depend on the whole shipped table.
    fn lang() -> Language {
        let mut m = HashMap::new();
        m.insert(
            PARSE_ERROR_KEY.to_string(),
            "%s at position %s: %s".to_string(),
        );
        Language::from_map(m)
    }

    // ── BuiltInExceptions' literals ──────────────────────────────────────

    #[test]
    fn every_reachable_literal_is_the_decompiles_text_verbatim() {
        // Pinned against `BuiltInExceptions.java`, not against the
        // implementation: a witness that reads the same constant the renderer
        // reads grades nothing.
        let cases: [(ReaderError, &str); 13] = [
            (ReaderError::ExpectedInt, "Expected integer"),
            (ReaderError::InvalidInt("1x".into()), "Invalid integer '1x'"),
            (ReaderError::ExpectedLong, "Expected long"),
            (ReaderError::InvalidLong("1x".into()), "Invalid long '1x'"),
            (ReaderError::ExpectedDouble, "Expected double"),
            (
                ReaderError::InvalidDouble("1x".into()),
                "Invalid double '1x'",
            ),
            (ReaderError::ExpectedFloat, "Expected float"),
            (ReaderError::InvalidFloat("1x".into()), "Invalid float '1x'"),
            (ReaderError::ExpectedBool, "Expected bool"),
            (
                ReaderError::InvalidBool("yes".into()),
                "Invalid bool, expected true or false but found 'yes'",
            ),
            (
                ReaderError::InvalidEscape("n".into()),
                "Invalid escape sequence 'n' in quoted string",
            ),
            (ReaderError::ExpectedEndOfQuote, "Unclosed quoted string"),
            (
                ReaderError::ExpectedArgumentSeparator,
                "Expected whitespace to end one argument, but found trailing data",
            ),
        ];
        for (e, want) in &cases {
            assert_eq!(message(BuiltIn::Reader(e)).as_deref(), Some(*want), "{e:?}");
        }
        assert_eq!(
            message(BuiltIn::UnknownCommand).as_deref(),
            Some("Unknown command")
        );
        assert_eq!(
            message(BuiltIn::UnknownArgument).as_deref(),
            Some("Incorrect argument for command")
        );
    }

    #[test]
    fn the_range_message_prints_the_bound_first_and_the_value_second() {
        // The transposition: `createWithContext(reader, result, minimum)`
        // feeds `(found, min) -> "... less than " + min + ", found " + found`.
        // Printing them in the order they arrive gives
        // "must not be less than 99, found 10", which reads perfectly
        // plausibly and is backwards.
        assert_eq!(
            message(BuiltIn::Reader(&ReaderError::OutOfRange {
                kind: NumKind::Integer,
                too_high: false,
                found: "-5".into(),
                bound: "0".into(),
            }))
            .unwrap(),
            "Integer must not be less than 0, found -5"
        );
        assert_eq!(
            message(BuiltIn::Reader(&ReaderError::OutOfRange {
                kind: NumKind::Double,
                too_high: true,
                found: "9.5".into(),
                bound: "1.0".into(),
            }))
            .unwrap(),
            "Double must not be more than 1.0, found 9.5"
        );
        // …and the four families differ only in the name they open with.
        for (k, name) in [
            (NumKind::Integer, "Integer"),
            (NumKind::Long, "Long"),
            (NumKind::Float, "Float"),
            (NumKind::Double, "Double"),
        ] {
            let m = message(BuiltIn::Reader(&ReaderError::OutOfRange {
                kind: k,
                too_high: true,
                found: "2".into(),
                bound: "1".into(),
            }))
            .unwrap();
            assert_eq!(m, format!("{name} must not be more than 1, found 2"));
        }
    }

    #[test]
    fn the_two_types_with_no_vanilla_text_answer_none_rather_than_inventing_one() {
        assert_eq!(message(BuiltIn::Reader(&ReaderError::UnknownArgumentType)), None);
        assert_eq!(message(BuiltIn::Reader(&ReaderError::LiteralIncorrect)), None);
    }

    // ── getContext ───────────────────────────────────────────────────────

    #[test]
    fn the_excerpt_is_the_ten_units_before_the_cursor_and_says_so_when_it_elides() {
        // Exactly ten before the cursor is NOT elided — `cursor > 10` is
        // strict, so an eleventh character is what earns the "...".
        assert_eq!(context(&u("/abcdefghij"), 10), "/abcdefghi<--[HERE]");
        assert_eq!(context(&u("/abcdefghij"), 11), "...abcdefghij<--[HERE]");
        // A short input takes the whole of it, without the marker.
        assert_eq!(context(&u("/xy"), 3), "/xy<--[HERE]");
        // A cursor at 1 — where `Unknown command` puts it — is one character.
        assert_eq!(context(&u("/notacommand"), 1), "/<--[HERE]");
    }

    #[test]
    fn a_cursor_past_the_end_is_clamped_rather_than_panicking() {
        // `Math.min(this.input.length(), this.cursor)`.
        assert_eq!(context(&u("/ab"), 99), "/ab<--[HERE]");
        assert_eq!(context(&[], 0), "<--[HERE]");
    }

    // ── getExceptionMessage ──────────────────────────────────────────────

    #[test]
    fn the_message_is_wrapped_with_its_position_and_its_excerpt() {
        let l = lang();
        assert_eq!(
            exception_message(BuiltIn::UnknownCommand, &u("/notacommand"), 1, Some(&l)).unwrap(),
            "Unknown command at position 1: /<--[HERE]"
        );
    }

    #[test]
    fn a_missing_language_table_prints_the_key_rather_than_the_message() {
        // `getOrDefault(key)`'s default is the key, and a template with no
        // `%s` in it drops every argument — which is what vanilla renders for
        // a missing translation, and is exactly the pre-M125 behaviour the
        // other walkers keep.
        assert_eq!(
            exception_message(BuiltIn::UnknownCommand, &u("/x"), 1, None).unwrap(),
            PARSE_ERROR_KEY
        );
    }

    #[test]
    fn a_malformed_template_falls_back_to_the_template_not_to_the_key() {
        // `TranslatableFormatException` -> `[FormattedText.of(format)]`.
        let mut m = HashMap::new();
        m.insert(PARSE_ERROR_KEY.to_string(), "%s at %q".to_string());
        let l = Language::from_map(m);
        assert_eq!(
            exception_message(BuiltIn::UnknownCommand, &u("/x"), 1, Some(&l)).unwrap(),
            "%s at %q"
        );
    }

    #[test]
    fn an_unprintable_type_carries_its_none_all_the_way_out() {
        let l = lang();
        assert_eq!(
            exception_message(
                BuiltIn::Reader(&ReaderError::UnknownArgumentType),
                &u("/x"),
                1,
                Some(&l)
            ),
            None
        );
    }

    // ── Commands.getParseException ───────────────────────────────────────

    fn arg(name: &str, type_name: &str, props: ArgumentProps) -> NodeKind {
        NodeKind::Argument {
            name: name.into(),
            type_id: 0,
            type_name: type_name.into(),
            props,
            suggestions: None,
        }
    }

    fn n(flags: u8, children: Vec<i32>, kind: NodeKind) -> CommandNode {
        CommandNode {
            flags,
            children,
            redirect: 0,
            kind,
        }
    }

    /// `/give <count:0..64>` and `/say <message>`.
    fn tree() -> CommandTree {
        use NodeKind::*;
        CommandTree {
            root: 0,
            nodes: vec![
                n(0, vec![1, 3], Root),
                n(1, vec![2], Literal("give".into())),
                n(
                    2 | 4,
                    vec![],
                    arg(
                        "count",
                        "brigadier:integer",
                        ArgumentProps::RangeI64 {
                            min: Some(0),
                            max: Some(64),
                        },
                    ),
                ),
                n(1, vec![4], Literal("say".into())),
                n(
                    2 | 4,
                    vec![],
                    arg(
                        "message",
                        "brigadier:string",
                        ArgumentProps::String(StringType::GreedyPhrase),
                    ),
                ),
            ],
        }
    }

    /// `/give <count:int> <flag:bool>` as **siblings**, so one bad word makes
    /// two children fail at the same level and the map holds two entries.
    fn two_arg_tree() -> CommandTree {
        use NodeKind::*;
        CommandTree {
            root: 0,
            nodes: vec![
                n(0, vec![1], Root),
                n(1, vec![2, 3], Literal("give".into())),
                n(
                    2 | 4,
                    vec![],
                    arg(
                        "count",
                        "brigadier:integer",
                        ArgumentProps::RangeI64 {
                            min: None,
                            max: None,
                        },
                    ),
                ),
                n(2 | 4, vec![], arg("flag", "brigadier:bool", ArgumentProps::None)),
            ],
        }
    }

    #[test]
    fn nothing_consumed_is_unknown_command_and_something_consumed_is_a_bad_argument() {
        // The **root** context's range is what decides — not how far the
        // reader got and not how many errors there were — so the two differ
        // by whether a literal matched at all.
        let t = tree();
        let bad = u("/notacommand");
        let p = parse(&t, &bad, 1, CommandCtx::default());
        assert_eq!(p.errors.len(), 0, "nothing was even tried");
        assert!(p.context.range.is_empty());
        let (e, cur) = parse_exception(&p).unwrap();
        assert_eq!(e, BuiltIn::UnknownCommand);
        assert_eq!(cur, 1, "the reader never left the slash");

        // Two siblings both refused the same word, so no single one can be
        // blamed and the root's range is non-empty.
        let t2 = two_arg_tree();
        let two = u("/give zz");
        let p = parse(&t2, &two, 1, CommandCtx::default());
        assert_eq!(p.errors.len(), 2, "fixture: not the single-error arm");
        assert!(!p.context.range.is_empty());
        assert_eq!(parse_exception(&p).unwrap().0, BuiltIn::UnknownArgument);

        // …and so is the ZERO-error form of the same arm: a literal matched
        // and then there was nothing left for a child to try.
        let t = tree();
        let trailing = u("/give ");
        let p = parse(&t, &trailing, 1, CommandCtx::default());
        assert_eq!(p.errors.len(), 0);
        assert!(!p.context.range.is_empty());
        assert_eq!(parse_exception(&p).unwrap().0, BuiltIn::UnknownArgument);
    }

    #[test]
    fn a_single_recorded_error_is_reported_as_itself() {
        // The middle arm, which is what makes "Integer must not be more
        // than 64" reachable at all — otherwise every failure would flatten
        // to "Incorrect argument for command".
        let t = tree();
        let over = u("/give 99");
        let p = parse(&t, &over, 1, CommandCtx::default());
        assert_eq!(p.errors.len(), 1);
        let (e, cur) = parse_exception(&p).unwrap();
        assert_eq!(
            message(e).unwrap(),
            "Integer must not be more than 64, found 99"
        );
        // The numeric types rewind before throwing, so the excerpt points at
        // the START of the offending number rather than past it.
        assert_eq!(cur, 6);
        assert_eq!(
            exception_message(e, &over, cur, Some(&lang())).unwrap(),
            "Integer must not be more than 64, found 99 at position 6: /give <--[HERE]"
        );
    }

    #[test]
    fn a_fully_consumed_input_has_no_parse_exception_at_all() {
        // `if (!parse.getReader().canRead()) return null` — the guard that
        // keeps a correct command from reporting one.
        let t = tree();
        let p = parse(&t, &u("/give 5"), 1, CommandCtx::default());
        assert!(parse_exception(&p).is_none());
        let p = parse(&t, &u("/say hello there"), 1, CommandCtx::default());
        assert!(parse_exception(&p).is_none());
    }

    #[test]
    fn literal_incorrect_is_never_recorded_by_a_parse() {
        // The structural claim from the module docs, checked over inputs
        // designed to provoke it: a prefix of a literal, a literal with junk
        // welded on, a literal in the wrong case, and one that is a superset
        // of another. `getRelevantNodes` hands over a literal only on an
        // exact word match, and that match cannot then fail.
        let t = tree();
        for input in [
            "/giv", "/gives", "/give", "/GIVE", "/give ", "/sayy", "/s", "/",
            "/give 5 extra", "/say", "/give;",
        ] {
            let p = parse(&t, &u(input), 1, CommandCtx::default());
            assert!(
                p.errors.iter().all(|e| e.error != ReaderError::LiteralIncorrect),
                "{input} recorded a literalIncorrect"
            );
        }
    }
}
