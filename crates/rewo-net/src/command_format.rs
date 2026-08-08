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

use crate::commands::{CommandNode, CommandTree, NodeKind};
use crate::dispatcher::ParseResults;

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

/// `CommandSuggestions.updateUsageInfo`, usage half.
///
/// Returns the lines to draw under the field, topmost **last** — see
/// `extractUsage`, which walks them with `height - 27 - 12 * y` so the first
/// entry is the LOWEST and the list grows upward.
///
/// # The error branch is a GATE here, not a message
///
/// ```java
/// if (this.input.getCursorPosition() == this.input.getValue().length()) {
///    if (suggestions.isEmpty() && !currentParse.getExceptions().isEmpty()) {
///       ... commandUsage.add(getExceptionMessage(exception)) ...
/// ```
///
/// and then `if (this.commandUsage.isEmpty())` guards the usage entries — so
/// when vanilla has an error to show it shows **that instead**. Rewo cannot
/// render the message: `BuiltInExceptions`' text is a set of `LiteralMessage`
/// literals that interpolate the offending value and the bound, and
/// `command.context.parse_error` wraps them with a cursor and an excerpt.
/// Those are transcribable and are simply not transcribed yet.
///
/// What *is* modelled is the gate: when the error branch would fire, this
/// returns **nothing**. Showing the usage entries there would put "what may
/// come next" under a command that is already wrong, which is the one place
/// vanilla deliberately does not.
///
/// `literalIncorrect` is counted separately by vanilla and replaced with a
/// single `dispatcherUnknownArgument` line; with no message text that
/// distinction has nothing to change, and it is recorded rather than
/// implemented.
pub fn usage_lines(
    tree: &CommandTree,
    parse: &ParseResults,
    cursor: usize,
    suggestions_empty: bool,
) -> Vec<String> {
    let at_end = cursor >= parse.reader.total_length();
    if at_end && suggestions_empty && !parse.errors.is_empty() {
        return Vec::new();
    }
    let Some(ctx) = parse.context.find_suggestion_context(cursor) else {
        return Vec::new();
    };
    node_usage(tree, ctx.parent)
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
    use crate::dispatcher::parse;

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
        let p = parse(&t, &units, 1);
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
        let p = parse(&t, &units, 1);
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

    #[test]
    fn the_usage_box_appears_where_an_argument_is_expected_and_not_before() {
        let t = tree();
        let give = u("/give ");
        let p = parse(&t, &give, 1);
        assert_eq!(usage_lines(&t, &p, give.len(), true), ["<count> <flag>"]);
        // …and nothing at the top level, where every child is a literal.
        let slash = u("/");
        let p = parse(&t, &slash, 1);
        assert!(usage_lines(&t, &p, slash.len(), true).is_empty());
    }

    #[test]
    fn an_error_at_the_end_of_the_line_suppresses_the_usage_entries() {
        // Vanilla shows the exception message there instead; Rewo has no
        // message yet, and showing "what may come next" under a command that
        // is already wrong is the one thing vanilla deliberately does not.
        let t = tree();
        let bad = u("/give 5x");
        let p = parse(&t, &bad, 1);
        assert!(!p.errors.is_empty(), "fixture precondition");
        assert!(usage_lines(&t, &p, bad.len(), true).is_empty());
        // The gate needs ALL THREE of its terms: with a suggestion to show,
        // or with the cursor away from the end, the entries come back.
        assert!(!usage_lines(&t, &p, bad.len(), false).is_empty());
        assert!(!usage_lines(&t, &p, 6, true).is_empty());
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
