//! The scoreboard sidebar — the panel down the right of the screen (M132).
//!
//! M65 decoded `set_objective` / `set_score` / `reset_score` /
//! `set_display_objective` into [`crate::scoreboard::Scoreboard`] and said in
//! its own module doc that a sidebar was "[`Scoreboard::display_objective`]
//! plus [`Scoreboard::scores_for_objective`] away". It was; this is that step.
//!
//! ## Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/client/gui/Hud.java` — `extractScoreboardSidebar` (which
//!   objective) and `displayScoreboardSidebar` (everything else), plus the
//!   `SCORE_DISPLAY_ORDER` comparator declared at the top of the class.
//! - `net/minecraft/world/scores/PlayerScoreEntry.java` — `isHidden`,
//!   `ownerName`, `formatValue`.
//! - `net/minecraft/world/scores/PlayerTeam.java` — `formatNameForTeam`,
//!   `getFormattedName`, `applyColor`.
//! - `net/minecraft/world/scores/TeamColor.java` — the colour → display-slot
//!   table.
//! - `net/minecraft/network/chat/numbers/StyledFormat.java` —
//!   `SIDEBAR_DEFAULT`.
//! - `net/minecraft/client/Options.java` — `getBackgroundColor(float)`.
//!
//! This module is **model + layout**: it resolves the objective, builds the
//! rows and places every rect and text origin as integers. It does not draw,
//! and it does not measure text — [`SidebarInput::width_of`] is supplied by
//! the caller, exactly as `rewo_world::chat` takes its width provider, so a
//! gate can feed exact widths and grade the geometry as numbers.
//!
//! ## Five things a plausible implementation gets wrong
//!
//! 1. **The sidebar is not always `DisplaySlot::Sidebar`.** If the local
//!    player is on a team *with a colour*, that colour names its own display
//!    slot and `extractScoreboardSidebar` prefers it. See [`select_objective`].
//! 2. **"Hidden" is a string prefix, not a flag.** `PlayerScoreEntry.isHidden`
//!    is `owner.startsWith("#")` — the convention servers use for fake score
//!    holders that exist only to store numbers.
//! 3. **The score's right edge is two pixels past the measured width.** The
//!    layout computes `left = guiWidth - width - 3` and then
//!    `right = guiWidth - 3 + 2`, so `right - left == width + 2`. Right-aligning
//!    scores to `left + width` — the width the layout just solved for — is one
//!    of two plausible readings and is wrong by two pixels.
//! 4. **The panel is not vertically centred.** `bottom = guiHeight / 2 +
//!    height / 3`, with the *third* of the content height, not the half.
//! 5. **A score's number format is a two-level fallback.** The score's own
//!    override beats the objective's, which beats `StyledFormat.SIDEBAR_DEFAULT`
//!    — and that default is **red**, not white.

use rewo_data::lang::Language;
use rewo_proto::nbt::Nbt;
use rewo_world::chat_style::{parse_component, ChatLine, ChatStyle, NAMED_COLORS};

use crate::scoreboard::{DisplaySlot, NumberFormat, Objective, Scoreboard};
use crate::teams::Team;

// ── Constants, all transcribed ────────────────────────────────────────────

/// `.limit(15L)` in `displayScoreboardSidebar`.
pub const MAX_ENTRIES: usize = 15;

/// The row pitch, `entriesCount * 9` and `bottom - (entriesCount - i) * 9`.
/// Also the header's own height.
pub const LINE_HEIGHT: i32 = 9;

/// The literal `3` in `left = guiWidth() - width - 3` and
/// `right = guiWidth() - 3 + 2`.
///
/// Vanilla declares `int rightPadding = 3;` on the line above and then **never
/// reads it** — both uses are the literal. It joins
/// `OverlayRecipeComponent`'s `int border = 4;` (M104) and
/// `extractRenderState`'s unread locals as a dead declaration; recorded so the
/// next reader does not go looking for a use.
pub const RIGHT_MARGIN: i32 = 3;

/// `right = guiWidth() - 3 + 2` — the `+ 2` that puts the panel's right edge
/// two pixels past `left + width`.
pub const RIGHT_OVERHANG: i32 = 2;

/// `fill(left - 2, ...)` — the background's left overhang past the text.
pub const LEFT_OVERHANG: i32 = 2;

/// `options.getBackgroundColor(0.3F)` with the default
/// `backgroundForChatOnly = true`, i.e. `ARGB.colorFromFloat(0.3, 0, 0, 0)`.
///
/// `as8BitChannel` is `Mth.floor(value * 255.0F)`, so 0.3 is **76** (`0x4C`)
/// and not the 77 a round would give.
pub const BODY_BACKGROUND: u32 = 0x4C00_0000;

/// `options.getBackgroundColor(0.4F)` — `floor(0.4 * 255) = 102` (`0x66`).
///
/// **Both alphas are the DEFAULT PROFILE's, not vanilla's only answer.**
/// `getBackgroundOpacity(default)` is
/// `backgroundForChatOnly.get() ? default : textBackgroundOpacity().get()`,
/// and `backgroundForChatOnly` defaults to `true` — so the two arguments
/// (0.3 and 0.4) are what a default client uses and the header is the more
/// opaque of the two. Set Text Background to "Everywhere" and **both calls
/// return the same slider value**, so the header stops being more opaque and
/// the two bands become indistinguishable. Rewo has no options screen, so the
/// default is the only reachable profile today and the constants are right;
/// the test below asserts `HEADER > BODY`, which is true of this profile and
/// would not be of that one.
pub const HEADER_BACKGROUND: u32 = 0x6600_0000;

/// The `-1` colour argument every one of the three `graphics.text` calls
/// passes: opaque white. A span carrying its own colour keeps that colour and
/// this argument's *alpha* (`Font.StringRenderOutput.getTextColor`).
pub const TEXT_COLOR: [f32; 3] = [1.0, 1.0, 1.0];

/// `StyledFormat.SIDEBAR_DEFAULT` — `Style.EMPTY.withColor(ChatFormatting.RED)`,
/// which is `TextColor.RED`, `0xFF5555`.
///
/// The player-list default one class over is `YELLOW`; the two are easy to
/// swap and neither is white.
pub const SIDEBAR_DEFAULT_RGB: u32 = 0xFF_5555;

/// The string whose width separates a name from its score when the score is
/// non-empty: `font.width(": ")`.
///
/// It is *only* a width — nothing draws a colon. Widening the panel by a
/// separator that is never rendered is vanilla's, and dropping it packs the
/// two columns together.
pub const SPACER_TEXT: &str = ": ";

/// The three `graphics.text` calls in `displayScoreboardSidebar` all pass an
/// explicit `false` for `dropShadow`.
///
/// The five-argument overload delegates with `true` (M105), and
/// `PlayerTabOverlay` one class over uses exactly that — so **the sidebar has
/// no drop shadow and the tab list does**, from the same `graphics.text`.
pub const DROP_SHADOW: bool = false;

// ── The resolved model ────────────────────────────────────────────────────

/// An integer rect, as `fill(x0, y0, x1, y1)` becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn corners(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect {
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
    pub fn right(self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(self) -> i32 {
        self.y + self.h
    }
}

/// One row: `DisplayEntry(name, score, scoreWidth)` in vanilla, which is a
/// local `record` inside the method.
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarEntry {
    /// The score holder's key — kept for the sort and for a gate to name a
    /// row, not drawn.
    pub owner: String,
    pub value: i32,
    /// `PlayerTeam.formatNameForTeam(team, score.ownerName())`.
    pub name: ChatLine,
    pub name_width: i32,
    /// `score.formatValue(objective.numberFormatOrDefault(SIDEBAR_DEFAULT))`.
    /// Empty for `BlankFormat`, whose `format` returns `Component.empty()`.
    pub score: ChatLine,
    pub score_width: i32,
}

/// The sidebar, resolved but not yet placed.
#[derive(Debug, Clone, PartialEq)]
pub struct Sidebar {
    /// The objective whose display this is — kept so a caller can log it.
    pub objective: String,
    pub title: ChatLine,
    pub title_width: i32,
    pub entries: Vec<SidebarEntry>,
    /// `biggestWidth`: the widest of the title and every
    /// `name + (score > 0 ? spacer + score : 0)`.
    pub width: i32,
}

/// One placed row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidebarRow {
    pub index: usize,
    /// `graphics.text(font, e.name, left, y, -1, false)`.
    pub name: (i32, i32),
    /// `graphics.text(font, e.score, right - e.scoreWidth, y, -1, false)`.
    /// `None` when the score is empty — vanilla still calls `text` with an
    /// empty component, which draws nothing.
    pub score: Option<(i32, i32)>,
}

/// The placed sidebar.
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarLayout {
    /// `fill(left - 2, headerY - 9 - 1, right, headerY - 1, headerBackground)`.
    pub header_background: Rect,
    /// `fill(left - 2, headerY - 1, right, bottom, background)`.
    pub body_background: Rect,
    /// The title's top-left, already centred over `width`.
    pub title: (i32, i32),
    pub rows: Vec<SidebarRow>,
    /// `left`, kept because every name starts there.
    pub left: i32,
    /// `right`, the score column's right edge.
    pub right: i32,
    /// `bottom`, the body background's lower edge.
    pub bottom: i32,
}

// ── Objective selection ───────────────────────────────────────────────────

/// `extractScoreboardSidebar`'s objective choice.
///
/// ```java
/// Objective teamObjective = null;
/// PlayerTeam playerTeam = scoreboard.getPlayersTeam(player.getScoreboardName());
/// if (playerTeam != null) {
///    Optional<TeamColor> teamColor = playerTeam.getColor();
///    if (teamColor.isPresent()) {
///       teamObjective = scoreboard.getDisplayObjective(teamColor.get().displaySlot());
///    }
/// }
/// Objective displayObjective = teamObjective != null ? teamObjective : scoreboard.getDisplayObjective(DisplaySlot.SIDEBAR);
/// ```
///
/// Two things fall out that reading only the `DisplaySlot.SIDEBAR` line would
/// miss: a coloured team's own slot **overrides** the plain sidebar, and it
/// only overrides when that slot actually holds an objective — an empty team
/// slot falls back rather than blanking the sidebar. A team with **no**
/// colour never reaches the team branch at all, which is why the sixteen
/// `sidebar.team.*` slots are addressed by colour and not by team name.
pub fn select_objective<'a>(
    scoreboard: &'a Scoreboard,
    local_scoreboard_name: &str,
) -> Option<&'a Objective> {
    let team_objective = team_display_slot(scoreboard, local_scoreboard_name)
        .and_then(|slot| scoreboard.display_objective(slot));
    match team_objective {
        Some(o) => Some(o),
        None => scoreboard.display_objective(DisplaySlot::Sidebar),
    }
}

/// `playerTeam.getColor().map(TeamColor::displaySlot)`.
///
/// `TeamColor`'s sixteen constants carry their slot as a constructor argument
/// in declaration order, and `DisplaySlot`'s first three are `LIST`,
/// `SIDEBAR`, `BELOW_NAME` — so colour *n* is slot `3 + n`. The offset is the
/// whole mapping; deriving it from the name would work and would break the
/// day a slot is inserted.
pub fn team_display_slot(scoreboard: &Scoreboard, member: &str) -> Option<DisplaySlot> {
    let team = scoreboard.teams.team_of_member(member)?;
    let color = scoreboard.teams.team(team)?.parameters.as_ref()?.color?;
    DisplaySlot::ALL.get(3 + color as usize).copied()
}

// ── Entry resolution ──────────────────────────────────────────────────────

/// `PlayerScoreEntry.isHidden()` — `this.owner.startsWith("#")`.
pub fn is_hidden(owner: &str) -> bool {
    owner.starts_with('#')
}

/// `SCORE_DISPLAY_ORDER`:
///
/// ```java
/// Comparator.comparing(PlayerScoreEntry::value)
///    .reversed()
///    .thenComparing(PlayerScoreEntry::owner, String.CASE_INSENSITIVE_ORDER)
/// ```
///
/// **`.reversed()` applies to the value key only.** `Comparator.reversed()`
/// on the result of `comparing(...)` reverses *that* comparator, and
/// `thenComparing` is then called on the reversed one — so the owner
/// tie-break runs forwards. Reversing the whole chain would sort equal scores
/// Z to A.
pub fn compare_scores(a: (&str, i32), b: (&str, i32)) -> std::cmp::Ordering {
    match b.1.cmp(&a.1) {
        std::cmp::Ordering::Equal => java_compare_ignore_case(a.0, b.0),
        other => other,
    }
}

/// Everything the resolver needs that it will not work out for itself.
pub struct SidebarInput<'a> {
    /// `StringSplitter`'s width provider, style-aware because bold charges one
    /// pixel per character (M126).
    pub width_of: &'a dyn Fn(&str, ChatStyle) -> i32,
    /// Resolves `translate` components (M125). `None` renders a key as itself.
    pub lang: Option<&'a Language>,
}

/// Build the sidebar for `objective`, or `None` when there is nothing to show.
///
/// Vanilla has no such `None`: `displayScoreboardSidebar` is only reached with
/// a non-null objective, and it happily draws a header over zero rows. That is
/// reproduced — an objective with no visible scores yields a `Sidebar` with an
/// empty `entries`, not `None`. The only `None` here is "no objective".
pub fn resolve(
    scoreboard: &Scoreboard,
    local_scoreboard_name: &str,
    input: &SidebarInput<'_>,
) -> Option<Sidebar> {
    let objective = select_objective(scoreboard, local_scoreboard_name)?;
    Some(resolve_for(scoreboard, objective, input))
}

/// The body of [`resolve`] with the objective already chosen.
pub fn resolve_for(
    scoreboard: &Scoreboard,
    objective: &Objective,
    input: &SidebarInput<'_>,
) -> Sidebar {
    let base = ChatStyle::plain(TEXT_COLOR);

    let mut rows: Vec<(&str, &crate::scoreboard::Score)> = scoreboard
        .scores_for_objective(&objective.name)
        .filter(|(owner, _)| !is_hidden(owner))
        .collect();
    rows.sort_by(|a, b| compare_scores((a.0, a.1.value), (b.0, b.1.value)));
    rows.truncate(MAX_ENTRIES);

    let title = parse_component(&objective.display_name, base.clone(), input.lang);
    let title_width = line_width(&title, input.width_of);
    let spacer_width = (input.width_of)(SPACER_TEXT, base.clone());

    let mut biggest = title_width;
    let mut entries = Vec::with_capacity(rows.len());
    for (owner, score) in rows {
        let owner_name = owner_name(owner, score.display.as_ref());
        let team = scoreboard
            .teams
            .team_of_member(owner)
            .and_then(|t| scoreboard.teams.team(t));
        let name = format_name_for_team(team, &owner_name, base.clone(), input.lang);
        let name_width = line_width(&name, input.width_of);

        let format = score
            .number_format
            .as_ref()
            .or(objective.number_format.as_ref());
        let score_line = format_value(format, score.value, base.clone(), input.lang);
        let score_width = line_width(&score_line, input.width_of);

        // `biggestWidth = max(biggest, nameWidth + (scoreWidth > 0 ? spacer + scoreWidth : 0))`
        // — the spacer is charged only when there is a score to separate.
        let charged = name_width + if score_width > 0 { spacer_width + score_width } else { 0 };
        biggest = biggest.max(charged);

        entries.push(SidebarEntry {
            owner: owner.to_string(),
            value: score.value,
            name,
            name_width,
            score: score_line,
            score_width,
        });
    }

    Sidebar {
        objective: objective.name.clone(),
        title,
        title_width,
        entries,
        width: biggest,
    }
}

/// `PlayerScoreEntry.ownerName()` — the display component when the server sent
/// one, else `Component.literal(owner)`.
fn owner_name(owner: &str, display: Option<&Nbt>) -> Nbt {
    match display {
        Some(d) => d.clone(),
        None => Nbt::String(owner.to_string()),
    }
}

/// `PlayerTeam.formatNameForTeam(team, name)`.
///
/// ```java
/// team == null ? name.copy() : team.getFormattedName(name)
/// // getFormattedName: applyColor(Component.empty().append(prefix).append(name).append(suffix))
/// ```
///
/// `applyColor` sets the colour on the **empty root**, whose three children
/// then inherit it through `Style.applyTo` — so a prefix that carries its own
/// colour keeps it and an uncoloured one takes the team's. Painting the three
/// runs the team colour unconditionally is the plausible shortcut and loses
/// every prefix a server deliberately coloured differently.
pub fn format_name_for_team(
    team: Option<&Team>,
    name: &Nbt,
    base: ChatStyle,
    lang: Option<&Language>,
) -> ChatLine {
    let Some(params) = team.and_then(|t| t.parameters.as_ref()) else {
        return parse_component(name, base, lang);
    };
    let root = match params.color {
        Some(c) => ChatStyle {
            color: rgb_of(NAMED_COLORS[c.min(15) as usize].1),
            ..base.clone()
        },
        None => base,
    };
    let mut out = parse_component(&params.player_prefix, root.clone(), lang);
    out.extend(parse_component(name, root.clone(), lang));
    out.extend(parse_component(&params.player_suffix, root, lang));
    out
}

/// `score.formatValue(objective.numberFormatOrDefault(StyledFormat.SIDEBAR_DEFAULT))`,
/// flattened into one call because the two `requireNonNullElse` steps are the
/// same fallback twice.
///
/// `format` is already `score.numberFormatOverride ?? objective.numberFormat`;
/// `None` here is therefore `SIDEBAR_DEFAULT`, red digits.
pub fn format_value(
    format: Option<&NumberFormat>,
    value: i32,
    base: ChatStyle,
    lang: Option<&Language>,
) -> ChatLine {
    let digits = value.to_string();
    match format {
        // `StyledFormat.format` is `Component.literal(digits).withStyle(style)`,
        // and the literal's own style is EMPTY — so the result is exactly the
        // format's style over the digits.
        None => parse_component(
            &Nbt::String(digits),
            ChatStyle { color: rgb_of(SIDEBAR_DEFAULT_RGB), ..base },
            lang,
        ),
        // `BlankFormat.format` returns `Component.empty()`. Zero spans, and
        // therefore zero width — which is what suppresses the `": "` spacer.
        Some(NumberFormat::Blank) => Vec::new(),
        Some(NumberFormat::Styled(style)) => {
            parse_component(&styled_literal(style, &digits), base, lang)
        }
        // `FixedFormat.format` is `this.value.copy()` — the number is
        // discarded entirely and the component drawn in its place.
        Some(NumberFormat::Fixed(component)) => parse_component(component, base, lang),
    }
}

/// `Component.literal(digits).withStyle(style)` as one component tag.
///
/// `Style.Serializer` and the component codec name the style fields
/// identically, so a `Style` tag plus a `text` key *is* the styled literal.
/// A non-compound style tag is treated as no style rather than as an error —
/// the digits still render, which is the non-fatal choice.
fn styled_literal(style: &Nbt, digits: &str) -> Nbt {
    match style {
        Nbt::Compound(fields) => {
            let mut merged: Vec<(String, Nbt)> = fields
                .iter()
                .filter(|(k, _)| k != "text")
                .cloned()
                .collect();
            merged.push(("text".to_string(), Nbt::String(digits.to_string())));
            Nbt::Compound(merged)
        }
        _ => Nbt::String(digits.to_string()),
    }
}

fn rgb_of(packed: u32) -> [f32; 3] {
    [
        ((packed >> 16) & 0xFF) as f32 / 255.0,
        ((packed >> 8) & 0xFF) as f32 / 255.0,
        (packed & 0xFF) as f32 / 255.0,
    ]
}

/// `Font.width(FormattedText)` — the sum over styled runs.
pub fn line_width(line: &ChatLine, width_of: &dyn Fn(&str, ChatStyle) -> i32) -> i32 {
    line.iter().map(|s| width_of(&s.text, s.style())).sum()
}

// ── Layout ────────────────────────────────────────────────────────────────

/// `displayScoreboardSidebar`'s geometry, verbatim:
///
/// ```java
/// int height   = entriesCount * 9;
/// int bottom   = graphics.guiHeight() / 2 + height / 3;
/// int left     = graphics.guiWidth() - width - 3;
/// int right    = graphics.guiWidth() - 3 + 2;
/// int headerY  = bottom - entriesCount * 9;
/// fill(left - 2, headerY - 9 - 1, right, headerY - 1, headerBackground);
/// fill(left - 2, headerY - 1,     right, bottom,      background);
/// text(title, left + width / 2 - titleWidth / 2, headerY - 9, -1, false);
/// for i: y = bottom - (entriesCount - i) * 9
///        text(name,  left,              y, -1, false);
///        text(score, right - scoreWidth, y, -1, false);
/// ```
///
/// Three of those lines are easy to "tidy" into something wrong. `bottom` uses
/// **`height / 3`**, so the panel is not centred. `right` is
/// `guiWidth - 1`, two past `left + width`, so scores hang past the width the
/// layout solved for. And the header band is nine tall ending one pixel above
/// the body (`headerY - 10 .. headerY - 1`) while the title sits at
/// `headerY - 9` — one pixel of padding above the glyphs and none below.
pub fn layout(sidebar: &Sidebar, gui_width: i32, gui_height: i32) -> SidebarLayout {
    let count = sidebar.entries.len() as i32;
    let width = sidebar.width;
    let height = count * LINE_HEIGHT;
    let bottom = gui_height / 2 + height / 3;
    let left = gui_width - width - RIGHT_MARGIN;
    let right = gui_width - RIGHT_MARGIN + RIGHT_OVERHANG;
    let header_y = bottom - count * LINE_HEIGHT;

    let header_background = Rect::corners(
        left - LEFT_OVERHANG,
        header_y - LINE_HEIGHT - 1,
        right,
        header_y - 1,
    );
    let body_background = Rect::corners(left - LEFT_OVERHANG, header_y - 1, right, bottom);
    let title = (
        left + width / 2 - sidebar.title_width / 2,
        header_y - LINE_HEIGHT,
    );

    let rows = sidebar
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let y = bottom - (count - i as i32) * LINE_HEIGHT;
            SidebarRow {
                index: i,
                name: (left, y),
                score: if e.score.is_empty() {
                    None
                } else {
                    Some((right - e.score_width, y))
                },
            }
        })
        .collect();

    SidebarLayout {
        header_background,
        body_background,
        title,
        rows,
        left,
        right,
        bottom,
    }
}

// ── Java string ordering ──────────────────────────────────────────────────

/// `String.CASE_INSENSITIVE_ORDER`, over UTF-16 code units.
///
/// The same algorithm `rewo_gpu::tab_list` transcribes for the tab list's
/// fourth sort key: compare raw, then folded up, then folded down, and only a
/// difference surviving all three counts.
fn java_compare_ignore_case(a: &str, b: &str) -> std::cmp::Ordering {
    let av: Vec<u16> = a.encode_utf16().collect();
    let bv: Vec<u16> = b.encode_utf16().collect();
    let n = av.len().min(bv.len());
    for k in 0..n {
        let (mut c1, mut c2) = (av[k], bv[k]);
        if c1 != c2 {
            c1 = fold(c1, true);
            c2 = fold(c2, true);
            if c1 != c2 {
                c1 = fold(c1, false);
                c2 = fold(c2, false);
                if c1 != c2 {
                    return c1.cmp(&c2);
                }
            }
        }
    }
    av.len().cmp(&bv.len())
}

/// `Character.toUpperCase(char)` / `toLowerCase(char)`: map only when the
/// result is a single code unit, else leave the input alone.
fn fold(u: u16, upper: bool) -> u16 {
    let Some(c) = char::from_u32(u as u32) else {
        return u;
    };
    let mut it = if upper {
        c.to_uppercase().collect::<Vec<char>>()
    } else {
        c.to_lowercase().collect::<Vec<char>>()
    };
    if it.len() != 1 {
        return u;
    }
    let mapped = it.pop().unwrap();
    let mut buf = [0u16; 2];
    let enc = mapped.encode_utf16(&mut buf);
    if enc.len() == 1 { enc[0] } else { u }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoreboard::{
        ObjectiveDisplay, ObjectiveMethod, RenderType, SetDisplayObjective, SetObjective, SetScore,
    };
    use crate::teams::{
        parse_set_player_team, CollisionRule, SetPlayerTeam, TeamMethod, TeamParameters, Visibility,
    };

    /// A width provider with a fixed advance per byte, so a witness can name a
    /// width in characters. Bold charges one extra pixel per character, which
    /// is `GlyphInfo.getBoldOffset()` (M126) and is what makes the provider
    /// style-aware rather than a `&str -> i32`.
    fn w6(text: &str, style: ChatStyle) -> i32 {
        let extra = i32::from(style.bold);
        text.chars().count() as i32 * (6 + extra)
    }

    fn input<'a>() -> SidebarInput<'a> {
        SidebarInput { width_of: &w6, lang: None }
    }

    fn literal(s: &str) -> Nbt {
        Nbt::String(s.to_string())
    }

    fn add_objective(sb: &mut Scoreboard, name: &str, title: &str) {
        sb.apply_set_objective(&SetObjective {
            name: name.to_string(),
            method: ObjectiveMethod::Add,
            display: Some(ObjectiveDisplay {
                display_name: literal(title),
                render_type: RenderType::Integer,
                number_format: None,
            }),
        });
    }

    fn set_score(sb: &mut Scoreboard, owner: &str, objective: &str, value: i32) {
        sb.apply_set_score(&SetScore {
            owner: owner.to_string(),
            objective_name: objective.to_string(),
            score: value,
            display: None,
            number_format: None,
        });
    }

    fn display(sb: &mut Scoreboard, slot: DisplaySlot, objective: Option<&str>) {
        sb.apply_set_display_objective(&SetDisplayObjective {
            slot,
            objective_name: objective.map(str::to_string),
        });
    }

    fn team(
        name: &str,
        color: Option<u8>,
        prefix: Nbt,
        suffix: Nbt,
        members: &[&str],
    ) -> SetPlayerTeam {
        SetPlayerTeam {
            name: name.to_string(),
            method: TeamMethod::Add,
            parameters: Some(TeamParameters {
                display_name: literal(name),
                player_prefix: prefix,
                player_suffix: suffix,
                name_tag_visibility: Visibility::Always,
                collision_rule: CollisionRule::Always,
                color,
                options: 0,
            }),
            players: members.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn plain(line: &ChatLine) -> String {
        line.iter().map(|s| s.text.as_str()).collect()
    }

    // -- objective selection ----------------------------------------------

    #[test]
    fn the_plain_sidebar_slot_is_used_when_the_player_has_no_team() {
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "kills", "Kills");
        display(&mut sb, DisplaySlot::Sidebar, Some("kills"));
        assert_eq!(
            select_objective(&sb, "Steve").map(|o| o.name.as_str()),
            Some("kills")
        );
    }

    #[test]
    fn a_coloured_teams_own_slot_overrides_the_plain_sidebar() {
        // The finding: `extractScoreboardSidebar` looks at the local player's
        // team colour FIRST. Reading only the `DisplaySlot.SIDEBAR` line gives
        // "kills" here, where vanilla shows "team_deaths".
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "kills", "Kills");
        add_objective(&mut sb, "team_deaths", "Deaths");
        display(&mut sb, DisplaySlot::Sidebar, Some("kills"));
        // TeamColor.RED is id 12, so its slot is DisplaySlot::ALL[3 + 12].
        display(&mut sb, DisplaySlot::TeamRed, Some("team_deaths"));
        sb.teams
            .apply(&team("red", Some(12), literal(""), literal(""), &["Steve"]));

        assert_eq!(team_display_slot(&sb, "Steve"), Some(DisplaySlot::TeamRed));
        assert_eq!(
            select_objective(&sb, "Steve").map(|o| o.name.as_str()),
            Some("team_deaths")
        );
        // ...and only for that player.
        assert_eq!(
            select_objective(&sb, "Alex").map(|o| o.name.as_str()),
            Some("kills")
        );
    }

    #[test]
    fn an_empty_team_slot_falls_back_rather_than_blanking_the_sidebar() {
        // `teamObjective != null ? teamObjective : getDisplayObjective(SIDEBAR)`.
        // A team colour whose slot holds nothing must not suppress the sidebar.
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "kills", "Kills");
        display(&mut sb, DisplaySlot::Sidebar, Some("kills"));
        sb.teams
            .apply(&team("red", Some(12), literal(""), literal(""), &["Steve"]));
        assert_eq!(
            select_objective(&sb, "Steve").map(|o| o.name.as_str()),
            Some("kills")
        );
    }

    #[test]
    fn a_team_with_no_colour_never_reaches_the_team_branch() {
        // `getColor()` is an Optional and the branch is guarded on isPresent().
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "kills", "Kills");
        display(&mut sb, DisplaySlot::Sidebar, Some("kills"));
        sb.teams
            .apply(&team("grey", None, literal(""), literal(""), &["Steve"]));
        assert_eq!(team_display_slot(&sb, "Steve"), None);
        assert_eq!(
            select_objective(&sb, "Steve").map(|o| o.name.as_str()),
            Some("kills")
        );
    }

    #[test]
    fn every_team_colour_maps_to_its_own_slot_at_an_offset_of_three() {
        // TeamColor declares BLACK..WHITE as 0..15 and DisplaySlot's first
        // three are LIST, SIDEBAR, BELOW_NAME.
        let mut sb = Scoreboard::new();
        for id in 0u8..16 {
            sb.teams.apply(&team(
                &format!("t{id}"),
                Some(id),
                literal(""),
                literal(""),
                &["Steve"],
            ));
            assert_eq!(
                team_display_slot(&sb, "Steve"),
                Some(DisplaySlot::ALL[3 + id as usize]),
                "colour {id}"
            );
        }
        assert_eq!(team_display_slot(&sb, "Steve"), Some(DisplaySlot::TeamWhite));
    }

    #[test]
    fn no_display_objective_anywhere_is_no_sidebar() {
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "kills", "Kills");
        assert!(select_objective(&sb, "Steve").is_none());
    }

    // -- entry selection ---------------------------------------------------

    #[test]
    fn a_holder_whose_name_starts_with_a_hash_is_hidden() {
        assert!(is_hidden("#totals"));
        assert!(!is_hidden("Steve"));
        // Only the FIRST character. A hash anywhere else is an ordinary name.
        assert!(!is_hidden("a#b"));
        assert!(!is_hidden(""));
    }

    #[test]
    fn hidden_holders_do_not_reach_the_rows() {
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "o", "T");
        display(&mut sb, DisplaySlot::Sidebar, Some("o"));
        set_score(&mut sb, "#internal", "o", 999);
        set_score(&mut sb, "Steve", "o", 1);
        let s = resolve(&sb, "Steve", &input()).unwrap();
        assert_eq!(s.entries.len(), 1);
        assert_eq!(s.entries[0].owner, "Steve");
    }

    #[test]
    fn scores_sort_by_value_descending_and_tie_break_by_owner_forwards() {
        // `.reversed()` binds to the VALUE key only -- `Comparator.reversed()`
        // is called on `comparing(value)` and `thenComparing` on the result,
        // so equal scores sort A to Z, not Z to A.
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "o", "T");
        display(&mut sb, DisplaySlot::Sidebar, Some("o"));
        set_score(&mut sb, "bravo", "o", 5);
        set_score(&mut sb, "Alpha", "o", 5);
        set_score(&mut sb, "charlie", "o", 9);
        let s = resolve(&sb, "x", &input()).unwrap();
        let owners: Vec<&str> = s.entries.iter().map(|e| e.owner.as_str()).collect();
        assert_eq!(owners, ["charlie", "Alpha", "bravo"]);
    }

    #[test]
    fn only_fifteen_rows_survive_and_they_are_the_fifteen_highest() {
        // The limit is applied AFTER the sort, so it keeps the top scores and
        // not whichever the map happened to hand over first.
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "o", "T");
        display(&mut sb, DisplaySlot::Sidebar, Some("o"));
        for i in 0..40 {
            set_score(&mut sb, &format!("p{i:02}"), "o", i);
        }
        let s = resolve(&sb, "x", &input()).unwrap();
        assert_eq!(s.entries.len(), MAX_ENTRIES);
        assert_eq!(s.entries[0].value, 39);
        assert_eq!(s.entries[MAX_ENTRIES - 1].value, 25);
    }

    #[test]
    fn an_objective_with_no_scores_still_produces_a_sidebar() {
        // Vanilla draws the header band over zero rows; `None` would be a
        // different behaviour, and there is no code path in `Hud` for it.
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "o", "Title");
        display(&mut sb, DisplaySlot::Sidebar, Some("o"));
        let s = resolve(&sb, "x", &input()).unwrap();
        assert!(s.entries.is_empty());
        assert_eq!(plain(&s.title), "Title");
    }

    // -- name formatting ---------------------------------------------------

    #[test]
    fn an_unteamed_owner_renders_as_its_own_name_in_the_base_colour() {
        let line = format_name_for_team(None, &literal("Steve"), ChatStyle::plain(TEXT_COLOR), None);
        assert_eq!(plain(&line), "Steve");
        assert_eq!(line[0].color, TEXT_COLOR);
    }

    #[test]
    fn a_teams_prefix_and_suffix_wrap_the_name_and_the_colour_is_inherited() {
        let mut sb = Scoreboard::new();
        // TeamColor.AQUA is 11 -> NAMED_COLORS[11] = 0x55FFFF.
        sb.teams
            .apply(&team("t", Some(11), literal("<"), literal(">"), &["Steve"]));
        let t = sb.teams.team("t").unwrap();
        let line =
            format_name_for_team(Some(t), &literal("Steve"), ChatStyle::plain(TEXT_COLOR), None);
        assert_eq!(plain(&line), "<Steve>");
        let aqua = [0x55 as f32 / 255.0, 1.0, 1.0];
        for span in &line {
            assert_eq!(span.color, aqua, "span {:?}", span.text);
        }
    }

    #[test]
    fn a_prefix_with_its_own_colour_keeps_it_against_the_teams() {
        // `applyColor` sets the colour on the empty ROOT; the three children
        // inherit through `Style.applyTo`, which is a null check. Painting all
        // three the team colour unconditionally loses this.
        let mut sb = Scoreboard::new();
        let prefix = Nbt::Compound(vec![
            ("text".to_string(), literal("*")),
            ("color".to_string(), literal("gold")),
        ]);
        sb.teams
            .apply(&team("t", Some(11), prefix, literal(""), &["Steve"]));
        let t = sb.teams.team("t").unwrap();
        let line =
            format_name_for_team(Some(t), &literal("Steve"), ChatStyle::plain(TEXT_COLOR), None);
        assert_eq!(plain(&line), "*Steve");
        let gold = [1.0, 0xAA as f32 / 255.0, 0.0];
        assert_eq!(line[0].color, gold);
        assert_ne!(line[1].color, gold);
    }

    #[test]
    fn a_score_display_component_replaces_the_owner_name() {
        // `ownerName()` prefers `display` -- the holder's key still drives the
        // sort and the team lookup, but the *text* is the server's.
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "o", "T");
        display(&mut sb, DisplaySlot::Sidebar, Some("o"));
        sb.apply_set_score(&SetScore {
            owner: "Steve".to_string(),
            objective_name: "o".to_string(),
            score: 3,
            display: Some(literal("The Builder")),
            number_format: None,
        });
        let s = resolve(&sb, "x", &input()).unwrap();
        assert_eq!(plain(&s.entries[0].name), "The Builder");
        assert_eq!(s.entries[0].owner, "Steve");
    }

    // -- number formats ----------------------------------------------------

    #[test]
    fn the_default_score_format_is_red_digits() {
        // StyledFormat.SIDEBAR_DEFAULT is RED (0xFF5555). The player-list
        // default one class over is YELLOW, and neither is white.
        let line = format_value(None, 42, ChatStyle::plain(TEXT_COLOR), None);
        assert_eq!(plain(&line), "42");
        assert_eq!(
            line[0].color,
            [1.0, 0x55 as f32 / 255.0, 0x55 as f32 / 255.0]
        );
    }

    #[test]
    fn a_blank_format_produces_no_spans_at_all() {
        // `BlankFormat.format` is `Component.empty()`, so the width is zero --
        // which is what suppresses the ": " spacer in the width solve.
        let line = format_value(
            Some(&NumberFormat::Blank),
            42,
            ChatStyle::plain(TEXT_COLOR),
            None,
        );
        assert!(line.is_empty());
    }

    #[test]
    fn a_fixed_format_discards_the_number_entirely() {
        let fixed = NumberFormat::Fixed(literal("--"));
        let line = format_value(Some(&fixed), 42, ChatStyle::plain(TEXT_COLOR), None);
        assert_eq!(plain(&line), "--");
    }

    #[test]
    fn a_styled_format_colours_the_digits_it_still_renders() {
        let style = Nbt::Compound(vec![("color".to_string(), literal("green"))]);
        let line = format_value(
            Some(&NumberFormat::Styled(style)),
            7,
            ChatStyle::plain(TEXT_COLOR),
            None,
        );
        assert_eq!(plain(&line), "7");
        assert_eq!(
            line[0].color,
            [0x55 as f32 / 255.0, 1.0, 0x55 as f32 / 255.0]
        );
    }

    #[test]
    fn a_scores_own_format_beats_the_objectives_which_beats_the_default() {
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&SetObjective {
            name: "o".to_string(),
            method: ObjectiveMethod::Add,
            display: Some(ObjectiveDisplay {
                display_name: literal("T"),
                render_type: RenderType::Integer,
                number_format: Some(NumberFormat::Fixed(literal("obj"))),
            }),
        });
        display(&mut sb, DisplaySlot::Sidebar, Some("o"));
        set_score(&mut sb, "objective_wins", "o", 1);
        sb.apply_set_score(&SetScore {
            owner: "score_wins".to_string(),
            objective_name: "o".to_string(),
            score: 2,
            display: None,
            number_format: Some(NumberFormat::Fixed(literal("own"))),
        });
        let s = resolve(&sb, "x", &input()).unwrap();
        let by: Vec<(&str, String)> = s
            .entries
            .iter()
            .map(|e| (e.owner.as_str(), plain(&e.score)))
            .collect();
        assert_eq!(
            by,
            [("score_wins", "own".into()), ("objective_wins", "obj".into())]
        );

        // ...and with no objective format at all, the default is reached.
        let mut sb2 = Scoreboard::new();
        add_objective(&mut sb2, "o", "T");
        display(&mut sb2, DisplaySlot::Sidebar, Some("o"));
        set_score(&mut sb2, "a", "o", 5);
        let s2 = resolve(&sb2, "x", &input()).unwrap();
        assert_eq!(plain(&s2.entries[0].score), "5");
    }

    // -- width solve -------------------------------------------------------

    #[test]
    fn the_spacer_is_charged_only_when_a_score_has_width() {
        // `biggest = max(biggest, nameWidth + (scoreWidth > 0 ? spacer + scoreWidth : 0))`.
        // ": " is two characters, so six pixels each under `w6`.
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "o", "");
        display(&mut sb, DisplaySlot::Sidebar, Some("o"));
        set_score(&mut sb, "abcd", "o", 7); // name 24, score "7" -> 6
        let s = resolve(&sb, "x", &input()).unwrap();
        assert_eq!(s.width, 24 + 12 + 6);

        // The same row with a blank format charges neither spacer nor score.
        let mut sb2 = Scoreboard::new();
        add_objective(&mut sb2, "o", "");
        display(&mut sb2, DisplaySlot::Sidebar, Some("o"));
        sb2.apply_set_score(&SetScore {
            owner: "abcd".to_string(),
            objective_name: "o".to_string(),
            score: 7,
            display: None,
            number_format: Some(NumberFormat::Blank),
        });
        let s2 = resolve(&sb2, "x", &input()).unwrap();
        assert_eq!(s2.width, 24);
    }

    #[test]
    fn the_title_seeds_the_width_so_a_wide_title_widens_the_panel() {
        // `int biggestWidth = objectiveDisplayNameWidth;` -- the loop only ever
        // raises it. Seeding at zero lets a long title overhang the panel.
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "o", "A very wide title indeed");
        display(&mut sb, DisplaySlot::Sidebar, Some("o"));
        set_score(&mut sb, "ab", "o", 1);
        let s = resolve(&sb, "x", &input()).unwrap();
        assert_eq!(s.title_width, 24 * 6);
        assert_eq!(s.width, 24 * 6);
    }

    // -- layout ------------------------------------------------------------

    fn three_rows() -> Sidebar {
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "o", "Ttl");
        display(&mut sb, DisplaySlot::Sidebar, Some("o"));
        set_score(&mut sb, "aaaaa", "o", 3);
        set_score(&mut sb, "bbbbb", "o", 2);
        set_score(&mut sb, "ccccc", "o", 1);
        resolve(&sb, "x", &input()).unwrap()
    }

    #[test]
    fn the_panel_hangs_two_pixels_past_the_width_it_solved_for() {
        // left = guiWidth - width - 3; right = guiWidth - 3 + 2. The score
        // column's right edge is therefore `left + width + 2`, and
        // right-aligning to `left + width` is the other plausible reading.
        let s = three_rows();
        let l = layout(&s, 320, 240);
        assert_eq!(l.left, 320 - s.width - 3);
        assert_eq!(l.right, 320 - 1);
        assert_eq!(l.right - l.left, s.width + 2);
    }

    #[test]
    fn the_panel_sits_a_third_of_its_height_below_centre() {
        // bottom = guiHeight / 2 + height / 3, with the THIRD of the content
        // height. Half would centre it.
        let s = three_rows();
        let l = layout(&s, 320, 240);
        assert_eq!(l.bottom, 240 / 2 + (3 * LINE_HEIGHT) / 3);
        assert_eq!(l.bottom, 129);
    }

    #[test]
    fn the_header_band_is_nine_tall_and_stops_one_pixel_above_the_body() {
        // fill(left-2, headerY-10, right, headerY-1) then
        // fill(left-2, headerY-1, right, bottom).
        let s = three_rows();
        let l = layout(&s, 320, 240);
        let header_y = l.bottom - 3 * LINE_HEIGHT;
        assert_eq!(l.header_background.y, header_y - LINE_HEIGHT - 1);
        assert_eq!(l.header_background.h, LINE_HEIGHT);
        assert_eq!(l.header_background.bottom(), header_y - 1);
        assert_eq!(l.body_background.y, header_y - 1);
        assert_eq!(l.body_background.bottom(), l.bottom);
        // Both bands share the same horizontal span, two pixels left of the
        // names and out to `right`.
        assert_eq!(l.header_background.x, l.left - 2);
        assert_eq!(l.body_background.x, l.left - 2);
        assert_eq!(l.header_background.right(), l.right);
        assert_eq!(l.body_background.right(), l.right);
    }

    #[test]
    fn the_title_sits_one_pixel_below_its_bands_top_and_is_centred_over_width() {
        // The band spans headerY-10..headerY-1 and the text is at headerY-9:
        // one pixel of padding above the glyphs and none below.
        let s = three_rows();
        let l = layout(&s, 320, 240);
        assert_eq!(l.title.1, l.header_background.y + 1);
        assert_eq!(l.title.0, l.left + s.width / 2 - s.title_width / 2);
    }

    #[test]
    fn rows_are_placed_upward_from_the_bottom_so_the_top_score_is_highest() {
        let s = three_rows();
        let l = layout(&s, 320, 240);
        assert_eq!(l.rows.len(), 3);
        for (i, r) in l.rows.iter().enumerate() {
            assert_eq!(r.name.0, l.left);
            assert_eq!(r.name.1, l.bottom - (3 - i as i32) * LINE_HEIGHT);
        }
        assert_eq!(l.rows[0].name.1, l.body_background.y + 1);
        assert_eq!(l.rows[2].name.1, l.bottom - LINE_HEIGHT);
    }

    #[test]
    fn a_score_is_right_aligned_to_the_panels_right_edge() {
        let s = three_rows();
        let l = layout(&s, 320, 240);
        for (row, entry) in l.rows.iter().zip(&s.entries) {
            assert_eq!(row.score, Some((l.right - entry.score_width, row.name.1)));
        }
    }

    #[test]
    fn an_empty_score_places_nothing() {
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "o", "T");
        display(&mut sb, DisplaySlot::Sidebar, Some("o"));
        sb.apply_set_score(&SetScore {
            owner: "a".to_string(),
            objective_name: "o".to_string(),
            score: 1,
            display: None,
            number_format: Some(NumberFormat::Blank),
        });
        let s = resolve(&sb, "x", &input()).unwrap();
        let l = layout(&s, 320, 240);
        assert_eq!(l.rows[0].score, None);
    }

    #[test]
    fn an_empty_sidebar_collapses_its_body_to_nothing_and_keeps_its_header() {
        // entriesCount 0 -> height 0 -> headerY == bottom == guiHeight/2, so
        // the body band is one pixel tall and the header band still draws.
        let mut sb = Scoreboard::new();
        add_objective(&mut sb, "o", "T");
        display(&mut sb, DisplaySlot::Sidebar, Some("o"));
        let s = resolve(&sb, "x", &input()).unwrap();
        let l = layout(&s, 320, 240);
        assert_eq!(l.bottom, 120);
        assert_eq!(l.body_background.h, 1);
        assert_eq!(l.header_background.h, LINE_HEIGHT);
        assert!(l.rows.is_empty());
    }

    // -- the transcribed colour literals -----------------------------------

    #[test]
    fn the_two_background_alphas_floor_rather_than_round() {
        // `as8BitChannel` is `Mth.floor(value * 255.0F)`: 0.3 -> 76, not 77.
        assert_eq!(BODY_BACKGROUND >> 24, 76);
        assert_eq!(HEADER_BACKGROUND >> 24, 102);
        assert_eq!(BODY_BACKGROUND & 0x00FF_FFFF, 0);
        assert_eq!(HEADER_BACKGROUND & 0x00FF_FFFF, 0);
        // The header is the *more* opaque of the two -- 0.4 against 0.3.
        // True of the DEFAULT profile, which is the only one Rewo can be in;
        // see `HEADER_BACKGROUND`'s docs for the option that equalises them.
        assert!(HEADER_BACKGROUND >> 24 > BODY_BACKGROUND >> 24);
    }

    #[test]
    fn the_sidebar_default_is_red_and_matches_the_named_colour_table() {
        assert_eq!(SIDEBAR_DEFAULT_RGB, NAMED_COLORS[12].1);
        assert_eq!(NAMED_COLORS[12].0, "red");
    }

    #[test]
    fn the_team_roster_reaches_the_lookup_through_the_real_packet_walk() {
        // Not a hand-built TeamParameters: the bytes go through
        // `parse_set_player_team`, so this witnesses the decode as well as the
        // colour -> slot map.
        let mut body: Vec<u8> = Vec::new();
        body.push(3);
        body.extend_from_slice(b"red");
        body.push(0); // ADD
        body.extend_from_slice(&[8, 0, 0]); // display name: TAG_String ""
        body.extend_from_slice(&[8, 0, 1, b'[']); // prefix "["
        body.extend_from_slice(&[8, 0, 1, b']']); // suffix "]"
        body.push(0); // visibility ALWAYS
        body.push(0); // collision ALWAYS
        body.push(1); // colour present
        body.push(12); // RED
        body.push(0); // options
        body.push(1); // one player
        body.push(5);
        body.extend_from_slice(b"Steve");
        let p = parse_set_player_team(&body).unwrap();

        let mut sb = Scoreboard::new();
        sb.teams.apply(&p);
        assert_eq!(team_display_slot(&sb, "Steve"), Some(DisplaySlot::TeamRed));
        let t = sb.teams.team("red").unwrap();
        let line =
            format_name_for_team(Some(t), &literal("Steve"), ChatStyle::plain(TEXT_COLOR), None);
        assert_eq!(plain(&line), "[Steve]");
    }
}
