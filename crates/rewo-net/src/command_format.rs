//! `CommandSuggestions.formatText` and `getSmartUsage` — the two things that
//! read the parse (M117).
//!
//! M116 built the dispatcher and left both as "reachable but unbuilt". They
//! are unrelated features that happen to share an input: one colours the text
//! you are typing, the other lists what may come next underneath it.
//!
//! # Everything unrecognised is GREY, not the field's own colour
//!
//! `LITERAL_STYLE` is `ChatFormatting.GRAY` and it covers every span between
//! the coloured ones — so the moment a parse exists the whole command dims and
//! only its arguments stay bright. A reader who leaves the gaps at the field's
//! own colour gets a line that never changes, which is the opposite of the
//! effect: the point is that `give` goes grey and `64` goes aqua.
//!
//! The five argument colours cycle **aqua, yellow, green, light purple, gold**,
//! and `nextColor` starts at `-1` with a **pre**-increment, so the first
//! argument is aqua rather than yellow. It also advances for an argument that
//! contributes no span, because the increment is above the guard.
//!
//! # The unparsed tail is red, and it is measured from the READER
//!
//! ```java
//! if (currentParse.getReader().canRead()) {
//!    int start = Math.max(currentParse.getReader().getCursor() - offset, 0);
//!    ...
//!    int end = Math.min(start + currentParse.getReader().getRemainingLength(), text.length());
//! ```
//!
//! Not from the last argument's end — the reader is where the parse gave up,
//! which is earlier than the end of the text whenever a *later* branch was
//! tried and rewound. That is what makes the red span start exactly at the
//! first character the client could not account for.
//!
//! # `getSmartUsage` decides with one string and prints another
//!
//! The multi-child branch builds a `LinkedHashSet` of each child's *deep*
//! usage, and then, if that set has more than one entry, iterates
//! **`children`** again and appends `child.getUsageText()` — the shallow form.
//! So the set is used only to decide whether the alternatives are
//! distinguishable; the text printed comes from a different call. Reusing the
//! set's strings, which is the obvious simplification, prints the deep form
//! inside the pipe list and produces `(give <count>|gamemode <flag>)` where
//! vanilla prints `(give|gamemode)`.
//!
//! Two more that read backwards: `open`/`close` are `[`/`]` when **this** node
//! is executable and `(`/`)` when it is not — optionality of the *children*,
//! keyed off the parent's own command; and `deep` skips the entire expansion,
//! so nesting stops one level down and a usage line is never more than two
//! deep.
//!
//! # Only NON-literal children get a usage line
//!
//! ```java
//! if (!(entry.getKey() instanceof LiteralCommandNode)) {
//!    lines.add(...);
//! }
//! ```
//!
//! So `/g` shows nothing at all — every child of the root is a literal — and
//! the usage box appears only where an **argument** is expected. That single
//! test is why the box is not a permanent menu of every subcommand.
//!
//! # …but the box is not only usage lines (M134)
//!
//! `commandUsage` also holds `updateUsageInfo`'s exception messages, which
//! M117 modelled as a gate and left blank. [`usage_lines`] now returns both,
//! as [`UsageLine`]s carrying the colour each one's style resolves to;
//! [`crate::command_errors`] owns the messages. The `/g`-shows-nothing rule
//! above is exactly what makes the third branch reachable: an empty
//! `fillNodeUsage` under a reader that can still read is what prints
//! *"Unknown command"*.

use crate::command_errors::{self, BuiltIn};
use crate::commands::{CommandNode, CommandTree, NodeKind};
use crate::dispatcher::{ParseResults, ReaderError};
use rewo_data::lang::Language;

/// `ChatFormatting`'s colours, as `Style.withColor` resolves them.
pub const GRAY: u32 = 0xAA_AAAA;
pub const RED: u32 = 0xFF_5555;
/// `ARGUMENT_STYLES`, in order.
pub const ARGUMENT_COLORS: [u32; 5] = [
    0x55_FFFF, // AQUA
    0xFF_FF55, // YELLOW
    0x55_FF55, // GREEN
    0xFF_55FF, // LIGHT_PURPLE
    0xFF_AA00, // GOLD
];

/// One coloured span of the input line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub color: u32,
}

/// `CommandSuggestions.formatText`.
///
/// `text` is the **visible** substring of the field and `offset` is where it
/// starts — `EditBox.renderWidget` passes `displayPos`, so a horizontally
/// scrolled field still colours the right characters. Passing 0 with a
/// scrolled field shifts every colour by the scroll.
///
/// Returns the runs in order; concatenating their text reproduces `text`.
pub fn format_text(parse: &ParseResults, text: &[u16], offset: usize) -> Vec<Run> {
    let mut parts: Vec<Run> = Vec::new();
    let mut unformatted_start = 0usize;
    // `-1` with a pre-increment, so the first argument is index 0 (aqua).
    let mut next_color: i32 = -1;
    let push = |parts: &mut Vec<Run>, from: usize, to: usize, color: u32| {
        if to > from && from <= text.len() {
            parts.push(Run {
                text: String::from_utf16_lossy(&text[from..to.min(text.len())]),
                color,
            });
        }
    };

    let last = parse.context.last_child();
    for (_, range) in &last.arguments {
        next_color += 1;
        if next_color as usize >= ARGUMENT_COLORS.len() {
            next_color = 0;
        }
        let start = range.start.saturating_sub(offset);
        // `break`, not `continue` — the arguments are in parse order, so once
        // one begins past the visible text every later one does too.
        if start >= text.len() {
            break;
        }
        let end = range.end.saturating_sub(offset).min(text.len());
        if end > 0 {
            push(&mut parts, unformatted_start, start, GRAY);
            push(&mut parts, start, end, ARGUMENT_COLORS[next_color as usize]);
            unformatted_start = end;
        }
    }

    if parse.reader.can_read() {
        let start = parse.reader.cursor().saturating_sub(offset);
        if start < text.len() {
            let end = (start + parse.reader.remaining_length()).min(text.len());
            push(&mut parts, unformatted_start, start, GRAY);
            push(&mut parts, start, end, RED);
            unformatted_start = end;
        }
    }

    push(&mut parts, unformatted_start, text.len(), GRAY);
    parts
}

/// `CommandNode.getUsageText` — the literal itself, or `<name>`.
fn usage_text(node: &CommandNode) -> String {
    match &node.kind {
        NodeKind::Literal(l) => l.clone(),
        NodeKind::Argument { name, .. } => format!("<{name}>"),
        NodeKind::Root => String::new(),
    }
}

fn children_of(tree: &CommandTree, index: i32) -> Vec<i32> {
    tree.node(index).map(|n| n.children.clone()).unwrap_or_default()
}

/// `CommandDispatcher.getSmartUsage(node, source)` — one entry per child.
///
/// The `optional` each child is rendered with is **the parent's**
/// executability, so a node that can be run on its own wraps every
/// continuation in `[...]`.
pub fn smart_usage(tree: &CommandTree, node: i32) -> Vec<(i32, String)> {
    let Some(parent) = tree.node(node) else {
        return Vec::new();
    };
    let optional = parent.is_executable();
    children_of(tree, node)
        .into_iter()
        .filter_map(|c| smart_usage_inner(tree, c, optional, false).map(|u| (c, u)))
        .collect()
}

fn smart_usage_inner(tree: &CommandTree, index: i32, optional: bool, deep: bool) -> Option<String> {
    let node = tree.node(index)?;
    // `canUse` — always true here; see `dispatcher`'s module docs.
    let text = usage_text(node);
    let self_text = if optional {
        format!("[{text}]")
    } else {
        text
    };
    let child_optional = node.is_executable();
    let (open, close) = if child_optional { ("[", "]") } else { ("(", ")") };
    if deep {
        // The whole expansion below is skipped, which is what stops a usage
        // line growing past two levels.
        return Some(self_text);
    }
    if node.has_redirect() {
        let redirect = if node.redirect == tree.root {
            "...".to_string()
        } else {
            match tree.node(node.redirect) {
                Some(r) => format!("-> {}", usage_text(r)),
                None => "...".to_string(),
            }
        };
        return Some(format!("{self_text} {redirect}"));
    }
    let children = children_of(tree, index);
    if children.len() == 1 {
        if let Some(usage) = smart_usage_inner(tree, children[0], child_optional, child_optional) {
            return Some(format!("{self_text} {usage}"));
        }
    } else if children.len() > 1 {
        // A LinkedHashSet: distinct deep usages, in first-seen order.
        let mut child_usage: Vec<String> = Vec::new();
        for &c in &children {
            if let Some(u) = smart_usage_inner(tree, c, child_optional, true) {
                if !child_usage.contains(&u) {
                    child_usage.push(u);
                }
            }
        }
        if child_usage.len() == 1 {
            let usage = &child_usage[0];
            let wrapped = if child_optional {
                format!("[{usage}]")
            } else {
                usage.clone()
            };
            return Some(format!("{self_text} {wrapped}"));
        }
        if child_usage.len() > 1 {
            // **The printed text comes from `getUsageText`, not from the set
            // above** — the set only decided that the alternatives differ.
            let joined = children
                .iter()
                .filter_map(|&c| tree.node(c).map(usage_text))
                .collect::<Vec<_>>()
                .join("|");
            if !joined.is_empty() {
                return Some(format!("{self_text} {open}{joined}{close}"));
            }
        }
    }
    Some(self_text)
}

/// `CommandSuggestions.fillNodeUsage` — the usage lines for the node before
/// the cursor, **excluding its literal children**.
pub fn node_usage(tree: &CommandTree, parent: i32) -> Vec<String> {
    smart_usage(tree, parent)
        .into_iter()
        .filter(|(c, _)| !matches!(tree.node(*c).map(|n| &n.kind), Some(NodeKind::Literal(_))))
        .map(|(_, u)| u)
        .collect()
}

/// One line of `commandUsage`, with the colour its own style resolves to.
///
/// Vanilla's list is `List<FormattedCharSequence>` and `extractUsage` draws
/// every entry with the same `-1`; `Font.getTextColor` uses that only as a
/// **default**, so a line's baked style decides. Two styles reach the list
/// from a client parse — `USAGE_FORMAT` (grey) on the usage entries and
/// nothing at all on an exception message, which therefore takes the white.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageLine {
    pub text: String,
    pub color: u32,
}

/// `USAGE_FORMAT` — `Style.EMPTY.withColor(ChatFormatting.GRAY)`.
pub const USAGE_COLOR: u32 = GRAY;

/// The `-1` `extractUsage` passes, which an unstyled message keeps.
pub const ERROR_COLOR: u32 = 0xFF_FFFF;

/// `CommandSuggestions.updateUsageInfo`, usage half.
///
/// Returns the lines to draw under the field, topmost **last** — see
/// `extractUsage`, which walks them with `height - 27 - 12 * y` so the first
/// entry is the LOWEST and the list grows upward.
///
/// # One list, and the exclusion inside it runs one way
///
/// ```java
/// if (this.input.getCursorPosition() == this.input.getValue().length()) {
///    if (suggestions.isEmpty() && !currentParse.getExceptions().isEmpty()) {
///       ... this.commandUsage.add(getExceptionMessage(exception)) ...
///    } else if (currentParse.getReader().canRead()) {
///       trailingCharacters = true;
///    }
/// }
/// ...
/// if (this.commandUsage.isEmpty()) {
///    List<FormattedCharSequence> usageEntries = this.fillNodeUsage(...);
///    if (usageEntries.isEmpty() && trailingCharacters) {
///       this.commandUsage.add(getExceptionMessage(Commands.getParseException(currentParse)));
///    }
///    this.commandUsage.addAll(usageEntries);
/// }
/// ```
///
/// The messages and the usage entries share `commandUsage`, so this is **not**
/// a third panel — the mutual exclusion M117 recorded is between the box and
/// the suggestion popup, one level up. The exclusion *here* is
/// `if (commandUsage.isEmpty())`, and it is one-directional: an exception
/// suppresses the usage entries, and the usage entries never suppress an
/// exception.
///
/// Note the third branch, which M117 omitted entirely and which is the most
/// visible of the three: with **no** recorded exception at all, a reader that
/// can still read and a node with no argument children produce
/// `Commands.getParseException` — that is what puts *"Unknown command at
/// position 1: /<--\[HERE\]"* under a mistyped command.
///
/// `lang` is `Option` for the reason [`crate::command_errors`] records: a
/// caller without the table renders the key, which is what vanilla renders
/// for a missing translation.
pub fn usage_lines(
    tree: &CommandTree,
    parse: &ParseResults,
    cursor: usize,
    suggestions_empty: bool,
    lang: Option<&Language>,
) -> Vec<UsageLine> {
    let mut usage: Vec<UsageLine> = Vec::new();
    let input = parse.reader.string();
    let error_line = |e: BuiltIn<'_>, at: usize| {
        command_errors::exception_message(e, input, at, lang).map(|text| UsageLine {
            text,
            color: ERROR_COLOR,
        })
    };
    let mut trailing_characters = false;
    // **The gate, kept separate from what it produced.** In vanilla the
    // exception branch always adds at least one line, so
    // `commandUsage.isEmpty()` below is an exact test for "the branch did not
    // run". Rewo has one exception type with no vanilla text
    // (`UnknownArgumentType` — see `command_errors::message`), so the branch
    // can run and add nothing; testing emptiness alone would then fall
    // through and print "what may come next" under a command that is already
    // wrong, which is the one thing this branch exists to prevent. M117
    // shipped the gate without the messages for exactly that reason and it
    // survives here unchanged.
    let mut exception_branch = false;
    // `getCursorPosition() == getValue().length()`. The cursor can never
    // exceed the field's length, so `==` and `>=` agree; `==` is the source.
    if cursor == parse.reader.total_length() {
        if suggestions_empty && !parse.errors.is_empty() {
            exception_branch = true;
            let mut literals = 0;
            for e in &parse.errors {
                if e.error == ReaderError::LiteralIncorrect {
                    literals += 1;
                } else if let Some(line) = error_line(BuiltIn::Reader(&e.error), e.cursor) {
                    usage.push(line);
                }
            }
            if literals > 0 {
                // Dead in vanilla — see `command_errors`' module docs on why
                // `literalIncorrect` cannot be recorded. Transcribed so the
                // shape reads against the source.
                if let Some(line) = error_line(BuiltIn::UnknownArgument, parse.reader.cursor()) {
                    usage.push(line);
                }
            }
        } else if parse.reader.can_read() {
            trailing_characters = true;
        }
    }
    let ctx = parse.context.find_suggestion_context(cursor);
    if usage.is_empty() && !exception_branch {
        let entries = ctx.map(|c| node_usage(tree, c.parent)).unwrap_or_default();
        if entries.is_empty() && trailing_characters {
            // `getParseException` is `@Nullable`, and vanilla dereferences it
            // unguarded — safely, because `trailingCharacters` implies
            // `getReader().canRead()`, which is the only branch that returns
            // null. Rewo keeps the Option rather than relying on that.
            if let Some((e, at)) = command_errors::parse_exception(parse) {
                if let Some(line) = error_line(e, at) {
                    usage.push(line);
                }
            }
        }
        usage.extend(entries.into_iter().map(|text| UsageLine {
            text,
            color: USAGE_COLOR,
        }));
    }
    usage
}

/// `suggestionContextAtCursor.startPos` — where the usage box is anchored.
///
/// The **same** context the suggestions use, which is what keeps the box under
/// the word being completed rather than under the start of the command.
pub fn usage_lines_start(parse: &ParseResults, cursor: usize) -> usize {
    parse
        .context
        .find_suggestion_context(cursor)
        .map(|c| c.start_pos)
        .unwrap_or(0)
}

/// Where `extractUsage` starts a line, given the field's geometry.
///
/// `Mth.clamp(getScreenX(startPos), 0, getScreenX(0) + innerWidth - width)` —
/// the same shape as the popup's, and with the same consequence when the box
/// is wider than the field: `clamp` returns its MAX, so the box hangs off the
/// left rather than pinning to zero.
pub fn usage_position(screen_x_of_start: i32, screen_x_of_zero: i32, inner_width: i32, box_width: i32) -> i32 {
    let max = screen_x_of_zero + inner_width - box_width;
    screen_x_of_start.max(0).min(max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{ArgumentProps, StringType};
    use crate::dispatcher::{parse, CommandCtx};

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

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

    /// `/give <count> <flag>` (both brigadier primitives so both parse),
    /// `/gamemode <flag>`, and `/tp` with two literal children.
    fn tree() -> CommandTree {
        use NodeKind::*;
        CommandTree {
            root: 0,
            nodes: vec![
                n(0, vec![1, 4, 6], Root),
                n(1, vec![2], Literal("give".into())),
                n(
                    2,
                    vec![3],
                    arg(
                        "count",
                        "brigadier:integer",
                        ArgumentProps::RangeI64 { min: None, max: None },
                    ),
                ),
                n(2 | 4, vec![], arg("flag", "brigadier:bool", ArgumentProps::None)),
                n(1, vec![5], Literal("gamemode".into())),
                n(2 | 4, vec![], arg("mode", "brigadier:bool", ArgumentProps::None)),
                n(1 | 4, vec![7, 8], Literal("tp".into())),
                n(1 | 4, vec![], Literal("here".into())),
                n(1 | 4, vec![], Literal("there".into())),
            ],
        }
    }

    fn runs(input: &str) -> Vec<Run> {
        let t = tree();
        let units = u(input);
        let p = parse(&t, &units, 1, CommandCtx::default());
        format_text(&p, &units, 0)
    }

    // ── formatText ───────────────────────────────────────────────────────

    #[test]
    fn the_literal_is_grey_and_the_first_argument_is_aqua() {
        // `nextColor` pre-increments from -1, so the FIRST argument is index 0.
        // Starting it at 0 and post-incrementing gives yellow, which is the
        // second colour and is wrong for every single-argument command there
        // is.
        assert_eq!(
            runs("/give 5"),
            [
                Run { text: "/give ".into(), color: GRAY },
                Run { text: "5".into(), color: ARGUMENT_COLORS[0] },
            ]
        );
    }

    #[test]
    fn a_second_argument_takes_the_next_colour_and_the_gap_stays_grey() {
        assert_eq!(
            runs("/give 5 true"),
            [
                Run { text: "/give ".into(), color: GRAY },
                Run { text: "5".into(), color: ARGUMENT_COLORS[0] },
                Run { text: " ".into(), color: GRAY },
                Run { text: "true".into(), color: ARGUMENT_COLORS[1] },
            ]
        );
    }

    #[test]
    fn the_unparsed_tail_is_red_and_starts_where_the_reader_stopped() {
        // Not at the last argument's end: the reader is where the parse gave
        // up. `notacommand` matches no literal, so nothing is consumed and the
        // whole thing after the slash is red.
        let r = runs("/notacommand");
        assert_eq!(r.last().unwrap().color, RED);
        assert_eq!(r[0], Run { text: "/".into(), color: GRAY });
        assert_eq!(r[1].text, "notacommand");
    }

    #[test]
    fn a_fully_parsed_command_has_no_red_at_all() {
        assert!(runs("/give 5 true").iter().all(|r| r.color != RED));
    }

    #[test]
    fn the_runs_concatenate_back_to_the_input() {
        // The property that stops a colouring bug from also being a text bug.
        for input in ["/give 5", "/give 5 true", "/notacommand", "/", "/tp here"] {
            let joined: String = runs(input).iter().map(|r| r.text.as_str()).collect();
            assert_eq!(joined, input, "{input}");
        }
    }

    #[test]
    fn the_offset_shifts_the_spans_by_the_fields_scroll() {
        // `EditBox` passes `displayPos`, so `text` is the VISIBLE substring.
        // With offset 0 and a truncated text the colours would land on the
        // wrong characters.
        let t = tree();
        let units = u("/give 5 true");
        let p = parse(&t, &units, 1, CommandCtx::default());
        // The field scrolled six characters: the visible text is "5 true".
        let visible = u("5 true");
        let r = format_text(&p, &visible, 6);
        assert_eq!(r[0], Run { text: "5".into(), color: ARGUMENT_COLORS[0] });
        assert_eq!(r[1], Run { text: " ".into(), color: GRAY });
        assert_eq!(r[2], Run { text: "true".into(), color: ARGUMENT_COLORS[1] });
    }

    #[test]
    fn a_command_with_no_arguments_is_entirely_grey() {
        assert_eq!(
            runs("/tp here"),
            [Run { text: "/tp here".into(), color: GRAY }]
        );
    }

    // ── getSmartUsage ────────────────────────────────────────────────────

    #[test]
    fn a_usage_line_names_the_child_and_its_single_continuation() {
        // `give`'s child is `<count>`, whose own single child is `<flag>` —
        // and `<count>` is not executable, so the continuation is not
        // bracketed.
        let t = tree();
        assert_eq!(smart_usage(&t, 1), [(2, "<count> <flag>".to_string())]);
    }

    #[test]
    fn several_distinguishable_children_become_a_pipe_list_of_their_SHALLOW_text() {
        // The finding: the LinkedHashSet decides, and `getUsageText` prints.
        // Reusing the set's deep strings would give `(here|there)` here too by
        // luck — so the fixture needs children whose deep form differs from
        // their shallow one, which is what the root gives.
        let t = tree();
        let usage = smart_usage(&t, 0);
        let give = usage.iter().find(|(c, _)| *c == 1).unwrap();
        assert_eq!(give.1, "give <count> <flag>");
        // `tp` is executable, so ITS children are bracketed rather than
        // parenthesised, and they are printed shallow.
        let tp = usage.iter().find(|(c, _)| *c == 6).unwrap();
        assert_eq!(tp.1, "tp [here|there]");
    }

    #[test]
    fn an_executable_parent_wraps_each_child_in_square_brackets() {
        // `getSmartUsage(node, source)`'s `optional` is the PARENT's
        // executability, so `tp`'s children come back as `[here]`/`[there]`.
        let t = tree();
        let usage = smart_usage(&t, 6);
        assert_eq!(
            usage,
            [(7, "[here]".to_string()), (8, "[there]".to_string())]
        );
    }

    #[test]
    fn identical_child_usages_collapse_to_one_rather_than_a_pipe_list() {
        // `childUsage.size() == 1` after the set — two children whose deep
        // usage is the same string produce one, not `(a|a)`.
        //
        // The grandchildren are what make `deep` observable. It is passed
        // `true` for exactly this loop, so the set holds the SHALLOW form
        // `<x>` twice and collapses. Ignoring `deep` expands them to
        // `<x> <p>` and `<x> <q>`, two distinct entries, and the collapse
        // becomes a pipe list of two identical names — `dup (<x>|<x>)`, which
        // is both wrong and absurd. A fixture whose children are leaves cannot
        // see any of that, because expanding a leaf changes nothing.
        let mut t = tree();
        t.nodes[0].children.push(9);
        t.nodes.push(n(1, vec![10, 11], NodeKind::Literal("dup".into())));
        t.nodes.push(n(2, vec![12], arg("x", "brigadier:bool", ArgumentProps::None)));
        t.nodes.push(n(2, vec![13], arg("x", "brigadier:bool", ArgumentProps::None)));
        t.nodes.push(n(2 | 4, vec![], arg("p", "brigadier:bool", ArgumentProps::None)));
        t.nodes.push(n(2 | 4, vec![], arg("q", "brigadier:bool", ArgumentProps::None)));
        // Each child on its own still expands one level, because `deep` is
        // false at the top of `getSmartUsage(node, source)`.
        assert_eq!(
            smart_usage(&t, 9),
            [(10, "<x> <p>".into()), (11, "<x> <q>".into())]
        );
        // …and the parent's view of them collapses, because the SET is built
        // from the shallow form.
        let root = smart_usage(&t, 0);
        assert_eq!(root.iter().find(|(c, _)| *c == 9).unwrap().1, "dup <x>");
    }

    // ── fillNodeUsage ────────────────────────────────────────────────────

    #[test]
    fn only_non_literal_children_get_a_usage_line() {
        // Which is why `/g` shows no box at all: every child of the root is a
        // literal. Including them would turn the usage box into a permanent
        // menu of every subcommand.
        let t = tree();
        assert!(node_usage(&t, 0).is_empty());
        assert_eq!(node_usage(&t, 1), ["<count> <flag>"]);
        assert!(node_usage(&t, 6).is_empty(), "tp's children are literals");
    }

    /// The one key `getExceptionMessage` resolves, so a test cannot depend on
    /// the whole shipped table.
    fn lang() -> rewo_data::lang::Language {
        let mut m = std::collections::HashMap::new();
        m.insert(
            crate::command_errors::PARSE_ERROR_KEY.to_string(),
            "%s at position %s: %s".to_string(),
        );
        rewo_data::lang::Language::from_map(m)
    }

    fn texts(lines: &[UsageLine]) -> Vec<&str> {
        lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn the_usage_box_appears_where_an_argument_is_expected_and_not_before() {
        let t = tree();
        let l = lang();
        let give = u("/give ");
        let p = parse(&t, &give, 1, CommandCtx::default());
        assert_eq!(
            texts(&usage_lines(&t, &p, give.len(), true, Some(&l))),
            ["<count> <flag>"]
        );
        // …and at the top level, where every child is a literal, the usage
        // entries are empty — so the THIRD branch fires instead of nothing.
        // `/` alone is not an error: the reader consumed the slash before the
        // parse began, so it cannot read and there is no trailing text.
        let slash = u("/");
        let p = parse(&t, &slash, 1, CommandCtx::default());
        assert!(usage_lines(&t, &p, slash.len(), true, Some(&l)).is_empty());
    }

    #[test]
    fn an_error_at_the_end_of_the_line_replaces_the_usage_entries_with_its_message() {
        // The gate M117 shipped, now with the message it was standing in for.
        // Showing "what may come next" under a command that is already wrong
        // is the one thing vanilla deliberately does not.
        let t = tree();
        let l = lang();
        let bad = u("/give 5x");
        let p = parse(&t, &bad, 1, CommandCtx::default());
        assert!(!p.errors.is_empty(), "fixture precondition");
        let lines = usage_lines(&t, &p, bad.len(), true, Some(&l));
        assert_eq!(
            texts(&lines),
            ["Expected whitespace to end one argument, but found trailing data \
              at position 7: /give 5<--[HERE]"]
        );
        // …and it is WHITE, where the entries it replaced are grey.
        assert_eq!(lines[0].color, ERROR_COLOR);
        // The branch needs ALL THREE of its terms: with a suggestion to show,
        // or with the cursor away from the end, the entries come back.
        assert_eq!(
            texts(&usage_lines(&t, &p, bad.len(), false, Some(&l))),
            ["<count> <flag>"]
        );
        assert_eq!(
            texts(&usage_lines(&t, &p, 6, true, Some(&l))),
            ["<count> <flag>"]
        );
    }

    #[test]
    fn the_usage_entries_are_grey_and_an_exception_is_white() {
        // `extractUsage` passes ONE colour for the whole list and
        // `Font.getTextColor` treats it as a default, so the styles in the
        // list decide. A box drawn in a single colour is wrong for one of the
        // two kinds of line whichever colour is picked.
        let t = tree();
        let l = lang();
        let give = u("/give ");
        let p = parse(&t, &give, 1, CommandCtx::default());
        assert_eq!(
            usage_lines(&t, &p, give.len(), true, Some(&l))[0].color,
            USAGE_COLOR
        );
        assert_eq!(USAGE_COLOR, GRAY);
        assert_ne!(USAGE_COLOR, ERROR_COLOR);
    }

    #[test]
    fn an_unknown_command_reports_itself_with_no_exception_recorded_at_all() {
        // The third branch, which M117 omitted: `getExceptions()` is EMPTY
        // here — the root's children are all literals and none matched, so
        // `getRelevantNodes` returned the (empty) argument set and nothing
        // was ever tried. What produces the line is `trailingCharacters` plus
        // an empty `fillNodeUsage`.
        let t = tree();
        let l = lang();
        let bad = u("/notacommand");
        let p = parse(&t, &bad, 1, CommandCtx::default());
        assert!(p.errors.is_empty(), "fixture: nothing was even tried");
        let lines = usage_lines(&t, &p, bad.len(), true, Some(&l));
        assert_eq!(
            texts(&lines),
            ["Unknown command at position 1: /<--[HERE]"]
        );
        assert_eq!(lines[0].color, ERROR_COLOR);
    }

    #[test]
    fn a_bad_argument_at_the_end_reports_the_argument_rather_than_the_command() {
        // Same branch, other arm of `getParseException`: `tp` matched, so the
        // root's range is non-empty. `tp`'s children are literals, so
        // `fillNodeUsage` is empty and the branch is reached.
        let t = tree();
        let l = lang();
        let bad = u("/tp nowhere");
        let p = parse(&t, &bad, 1, CommandCtx::default());
        // Position **4**, not 3: `parseNodes` skips the separator before
        // recursing, so the reader the exception captures sits past the space
        // and the excerpt therefore ends with it. The witness first claimed
        // `/tp<--[HERE]` and was wrong.
        assert_eq!(
            texts(&usage_lines(&t, &p, bad.len(), true, Some(&l))),
            ["Incorrect argument for command at position 4: /tp <--[HERE]"]
        );
    }

    #[test]
    fn a_usage_entry_beats_the_third_branch_when_there_is_one_to_show() {
        // `if (usageEntries.isEmpty() && trailingCharacters)` — both terms.
        // `/give x` leaves the reader mid-input AND has a `<count>` line to
        // offer, and vanilla shows the line rather than the exception.
        let t = tree();
        let l = lang();
        let bad = u("/give x");
        let p = parse(&t, &bad, 1, CommandCtx::default());
        // Cursor away from the end, so the first branch cannot fire and only
        // the third is in play.
        let lines = usage_lines(&t, &p, 6, true, Some(&l));
        assert_eq!(texts(&lines), ["<count> <flag>"]);
        assert_eq!(lines[0].color, USAGE_COLOR);
    }

    #[test]
    fn an_unnameable_error_prints_nothing_rather_than_inventing_a_message() {
        // An argument type this client has no reader for reports
        // `UnknownArgumentType`, which has no `BuiltInExceptions`
        // counterpart. M121 closed every `minecraft:` type, so the reachable
        // case is a modded server's own — and the GATE still fires, so the
        // usage entries stay suppressed. That is M117's behaviour preserved
        // exactly where there is no vanilla text to print.
        //
        // The first draft used `minecraft:entity`, which PARSES a bare player
        // name perfectly well and recorded no error at all — a fixture that
        // could not express its claim.
        let mut t = tree();
        t.nodes[0].children.push(9);
        t.nodes.push(n(1, vec![10], NodeKind::Literal("msg".into())));
        t.nodes.push(n(
            2 | 4,
            vec![],
            arg("modded:thing", "modded:thing", ArgumentProps::None),
        ));
        let l = lang();
        let bad = u("/msg Steve");
        let p = parse(&t, &bad, 1, CommandCtx::default());
        assert_eq!(
            p.errors.iter().map(|e| e.error.clone()).collect::<Vec<_>>(),
            [crate::dispatcher::ReaderError::UnknownArgumentType]
        );
        assert!(usage_lines(&t, &p, bad.len(), true, Some(&l)).is_empty());
    }

    #[test]
    fn the_usage_box_hangs_off_the_left_when_it_is_wider_than_the_field() {
        // `Mth.clamp(v, 0, max)` with max < 0 returns MAX — the popup's rule
        // one widget over.
        assert_eq!(usage_position(40, 4, 316, 20), 40);
        assert_eq!(usage_position(40, 4, 316, 600), -280);
    }

    #[test]
    fn a_greedy_string_argument_still_gets_a_line() {
        let mut t = tree();
        t.nodes[0].children.push(9);
        t.nodes.push(n(1, vec![10], NodeKind::Literal("say".into())));
        t.nodes.push(n(
            2 | 4,
            vec![],
            arg(
                "message",
                "brigadier:string",
                ArgumentProps::String(StringType::GreedyPhrase),
            ),
        ));
        assert_eq!(node_usage(&t, 9), ["<message>"]);
    }
}
