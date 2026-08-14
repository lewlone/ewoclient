//! The tab list (player list) — model + layout (M52f).
//!
//! Transcribed from the 26.2 decompile,
//! `net/minecraft/client/gui/components/PlayerTabOverlay.java`
//! (`getPlayerInfos`, `PLAYER_COMPARATOR`, `extractRenderState`,
//! `extractPingIcon`). **Nothing here is chosen.** Where a value looks
//! arbitrary it is vanilla's, and rounding it off is a visual regression.
//!
//! This module is **model + layout only — it does not render.** Layout is a
//! pure function of integers, which is the point: the geometry can be graded
//! as numbers rather than by squinting at a screenshot. Text *measurement* is
//! deliberately an input (`max_name_width`), not something this module does —
//! that keeps it free of the glyph cache and lets a gate feed exact widths.
//!
//! ## Two things the layout is easy to get wrong
//!
//! * The list is laid out **column-major** (`col = i / rows`,
//!   `row = i % rows`). Row-major reads the same in a screenshot with one
//!   column and is wrong the moment there are two.
//! * `rows` is **not** 20 whenever the list overflows. It is the *balanced*
//!   height `ceil(n / columns)` — 21 players make two columns of 11, not a
//!   column of 20 and a column of 1.
//!
//! ## What vanilla does NOT do
//!
//! There is **no per-column width**. `maxNameWidth` is a single maximum over
//! every listed player and `slotWidth` is uniform across columns, so a column
//! of short names is exactly as wide as a column of long ones. Sizing each
//! column to its own widest name would look tidier and would not be vanilla.

use std::cmp::Ordering;

// ── Constants, all transcribed ────────────────────────────────────────────

/// `PlayerTabOverlay.MAX_ROWS_PER_COL`. Public in vanilla, and the loop
/// bound that grows the column count.
pub const MAX_ROWS_PER_COL: i32 = 20;

/// `getPlayerInfos`: `.sorted(PLAYER_COMPARATOR).limit(80L)`.
///
/// The `limit` is applied **after** the sort, so it keeps the 80 entries that
/// sort first — not the first 80 the server happened to send.
pub const MAX_ENTRIES: usize = 80;

/// Vertical stride between rows (`yo = yyo + row * 9`).
pub const ROW_STRIDE: i32 = 9;

/// Height of a row's background fill (`fill(xo, yo, xo + slotWidth, yo + 8)`).
///
/// Eight, against a stride of nine — the 1px difference is the gap between
/// rows. Making these equal closes it and the list becomes one solid block.
pub const ROW_BACKGROUND_HEIGHT: i32 = 8;

/// The player face is blitted 8x8, then `xo += 9` — so the face *advance* is
/// one pixel wider than the face.
pub const FACE_SIZE: i32 = 8;
pub const FACE_ADVANCE: i32 = 9;

/// The `+ 13` in the `slotWidth` expression: padding plus room for the ping
/// icon, folded into one literal in vanilla.
pub const SLOT_EXTRA_WIDTH: i32 = 13;

/// Horizontal gap between columns (`xo = xxo + col * slotWidth + col * 5`).
pub const COLUMN_GAP: i32 = 5;

/// `int yyo = 10` — the list's top margin before any header.
pub const TOP_MARGIN: i32 = 10;

/// `screenWidth - 50`, both the `slotWidth` clamp and the header/footer wrap
/// width.
pub const SCREEN_MARGIN: i32 = 50;

/// `widthForScore` when the objective renders as HEARTS.
pub const HEARTS_WIDTH: i32 = 90;

/// The heart sprite is blitted 9x9 (M155) -- on an 8-px pitch at the
/// default column, so neighbours overlap by a column.
pub const HEART_SPRITE_SIZE: i32 = 9;

/// The score is drawn only `if (right - left > 5)`. Strictly greater.
pub const SCORE_MIN_SPAN: i32 = 5;

/// Ping icon blit size (`blitSprite(..., xo + slotWidth - 11, yo, 10, 8)`).
pub const PING_ICON_W: i32 = 10;
pub const PING_ICON_H: i32 = 8;
/// The icon's right inset from the slot's right edge: `slotWidth - 11`.
///
/// Eleven, not ten — the icon is 10 wide, so this leaves a 1px right margin.
pub const PING_ICON_RIGHT_INSET: i32 = 11;

/// `Integer.MIN_VALUE`, the header / list / footer band fill colour.
/// As ARGB that is alpha 128, black.
pub const BAND_COLOR: u32 = 0x8000_0000;

/// `this.minecraft.options.getBackgroundColor(553648127)` — the per-row fill.
///
/// This is the *fallback*; an accessibility text-background setting replaces
/// it. 553648127 is `0x20FFFFFF`: alpha 32, white.
pub const DEFAULT_ROW_BACKGROUND: u32 = 553_648_127;

/// Name colour for a normal player (`-1`, opaque white).
pub const NAME_COLOR: u32 = 0xFFFF_FFFF;
/// Name colour for a spectator: vanilla's literal `-1862270977`, which as an
/// unsigned 32-bit ARGB is **`0x90FFFFFF`** — *white* at alpha 144, not a
/// grey. Spectators are also italicised (`decorateName`).
///
/// M52f wrote `0x9099_9999` here and said so in the doc comment; both were
/// wrong, and nothing caught it because nothing consumed the constant. M132
/// corrected it against the decompile's literal and pinned it below, where
/// the test derives the value from the signed integer rather than restating
/// the hex — a witness that restated it would agree with any hex at all.
pub const SPECTATOR_NAME_COLOR: u32 = 0x90FF_FFFF;

// ── The ping icon bucket ──────────────────────────────────────────────────

/// Which signal-strength sprite a latency maps to.
///
/// From `extractPingIcon`. The thresholds are all **strictly less than**, and
/// the mapping is *inverted* against the numbers: `Ping5` is the best
/// connection (five bars) and belongs to the **lowest** latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PingIcon {
    /// `latency < 0` — no measurement yet.
    Unknown,
    Ping1,
    Ping2,
    Ping3,
    Ping4,
    Ping5,
}

impl PingIcon {
    /// The sprite name, `icon/ping_*`, exactly as vanilla registers it.
    pub fn sprite(self) -> &'static str {
        match self {
            PingIcon::Unknown => "icon/ping_unknown",
            PingIcon::Ping1 => "icon/ping_1",
            PingIcon::Ping2 => "icon/ping_2",
            PingIcon::Ping3 => "icon/ping_3",
            PingIcon::Ping4 => "icon/ping_4",
            PingIcon::Ping5 => "icon/ping_5",
        }
    }
}

/// `extractPingIcon`'s if-chain, verbatim:
///
/// ```text
/// latency <    0  -> ping_unknown
/// latency <  150  -> ping_5
/// latency <  300  -> ping_4
/// latency <  600  -> ping_3
/// latency < 1000  -> ping_2
/// else            -> ping_1
/// ```
///
/// Note there is no upper `Unknown`: a latency of 30 seconds is still
/// `ping_1`, one bar. Only a *negative* value reads as unknown.
pub fn ping_icon(latency: i32) -> PingIcon {
    if latency < 0 {
        PingIcon::Unknown
    } else if latency < 150 {
        PingIcon::Ping5
    } else if latency < 300 {
        PingIcon::Ping4
    } else if latency < 600 {
        PingIcon::Ping3
    } else if latency < 1000 {
        PingIcon::Ping2
    } else {
        PingIcon::Ping1
    }
}

// ── The entry model ───────────────────────────────────────────────────────

/// One listed player.
///
/// The four sort keys vanilla uses (`tab_list_order`, `spectator`, `team`,
/// `name`) are all fields here. **All four are decoded as of M62** —
/// `player_info_update` action 6 is the tab-list order and action 2 the game
/// mode (`PlaySession::tab_list_order` / `game_mode`), and team membership
/// arrives on the separate `set_player_team` packet
/// (`PlaySession::team_of`). Nothing populates them here yet: this module is
/// still model-only and no caller builds a `TabEntry` from a live session.
///
/// Their "absent" values (`0`, `false`, `None`) collapse the sort to a
/// case-insensitive name sort — which is exactly what vanilla does for a
/// server that sets none of them, so the default is a real vanilla case and
/// not a simplification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabEntry {
    pub uuid: u128,
    /// `GameProfile.name()`.
    pub name: String,
    /// `PlayerInfo.getLatency()`, in ms. `None` where the client has no value
    /// at all; it maps to the same icon as a negative latency but is kept
    /// distinct so a caller can tell "not sent" from "server said -1".
    pub ping: Option<i32>,
    /// Whether this row is the local player. **Plays no part in the sort** —
    /// vanilla does not float you to the top. It is here for the renderer.
    pub local: bool,
    /// `PlayerInfo.getTabListOrder()`.
    pub tab_list_order: i32,
    /// `getGameMode() == GameType.SPECTATOR`.
    pub spectator: bool,
    /// `Optionull.mapOrDefault(getTeam(), PlayerTeam::getName, "")` — so a
    /// player with no team sorts as the empty string, ahead of every named
    /// team.
    pub team: Option<String>,
}

impl TabEntry {
    /// A minimal entry: the three things Rewo actually decodes today.
    pub fn new(uuid: u128, name: impl Into<String>, ping: Option<i32>) -> TabEntry {
        TabEntry {
            uuid,
            name: name.into(),
            ping,
            local: false,
            tab_list_order: 0,
            spectator: false,
            team: None,
        }
    }

    pub fn local(mut self, local: bool) -> TabEntry {
        self.local = local;
        self
    }

    /// The icon this row draws. A missing ping is treated as vanilla's
    /// "unknown" (`getLatency()` starts at 0 for a fresh entry, but a client
    /// with no value at all has nothing better to show).
    pub fn ping_icon(&self) -> PingIcon {
        ping_icon(self.ping.unwrap_or(-1))
    }

    /// `Optionull.mapOrDefault(..., "")`.
    fn team_key(&self) -> &str {
        self.team.as_deref().unwrap_or("")
    }
}

// ── The sort ──────────────────────────────────────────────────────────────

/// `PLAYER_COMPARATOR`, all four keys in order:
///
/// ```text
/// comparingInt(p -> -p.getTabListOrder())
///   .thenComparingInt(p -> p.getGameMode() == SPECTATOR ? 1 : 0)
///   .thenComparing(p -> team name, or "")
///   .thenComparing(p -> profile name, String::compareToIgnoreCase)
/// ```
///
/// Two asymmetries worth keeping straight: the **team** key uses Java's plain
/// `String.compareTo` (case *sensitive*, so `"Zebra"` sorts before `"apple"`),
/// while the **name** key uses `compareToIgnoreCase`. Reading both as
/// case-insensitive is the plausible-looking mistake.
pub fn compare_entries(a: &TabEntry, b: &TabEntry) -> Ordering {
    // `-p.getTabListOrder()` is int negation in Java, so `-Integer.MIN_VALUE`
    // wraps back to itself rather than panicking or saturating.
    let ord = a
        .tab_list_order
        .wrapping_neg()
        .cmp(&b.tab_list_order.wrapping_neg());
    if ord != Ordering::Equal {
        return ord;
    }

    // Spectators last: the key is 1 for a spectator and 0 for everyone else.
    let spec = i32::from(a.spectator).cmp(&i32::from(b.spectator));
    if spec != Ordering::Equal {
        return spec;
    }

    let team = java_compare(a.team_key(), b.team_key());
    if team != Ordering::Equal {
        return team;
    }

    java_compare_ignore_case(&a.name, &b.name)
}

/// Sort and truncate exactly as `getPlayerInfos` does.
///
/// **Sort first, then take 80.** Truncating first would keep whichever 80 the
/// server sent earliest and then sort those, which on an 81st join silently
/// drops a player who should be visible.
///
/// The sort is stable, matching `Stream.sorted()` on an ordered stream — two
/// entries equal on all four keys keep their arrival order.
pub fn visible_entries(entries: &[TabEntry]) -> Vec<TabEntry> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(compare_entries);
    sorted.truncate(MAX_ENTRIES);
    sorted
}

/// Java's `String.compareTo`, over UTF-16 code units.
///
/// Rust's own `str` ordering compares *bytes*, which agrees with Java for
/// ASCII and disagrees above the BMP — this is the faithful version.
fn java_compare(a: &str, b: &str) -> Ordering {
    let mut ai = a.encode_utf16();
    let mut bi = b.encode_utf16();
    loop {
        match (ai.next(), bi.next()) {
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
            }
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

/// Java's `String.CASE_INSENSITIVE_ORDER` / `compareToIgnoreCase`.
///
/// The real algorithm folds **up then down**, not just down: two code units
/// are compared raw, then as uppercase, then as lowercase, and only a
/// difference surviving all three counts. Single-code-unit mappings only, so
/// the multi-char expansions Rust's `to_uppercase` can produce are ignored —
/// that is what Java's `char`-based `Character.toUpperCase` does too.
fn java_compare_ignore_case(a: &str, b: &str) -> Ordering {
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
        return u; // an unpaired surrogate has no case mapping
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

// ── Layout ────────────────────────────────────────────────────────────────

/// An integer rect. `w`/`h` rather than a second corner, so a blit and a fill
/// read the same way; vanilla's `fill(x1, y1, x2, y2)` becomes
/// `Rect { x: x1, y: y1, w: x2 - x1, h: y2 - y1 }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn corners(x1: i32, y1: i32, x2: i32, y2: i32) -> Rect {
        Rect { x: x1, y: y1, w: x2 - x1, h: y2 - y1 }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
}

/// What the objective column costs, if there is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreColumn {
    /// No display objective.
    None,
    /// `RenderType.HEARTS` — a fixed 90px.
    Hearts,
    /// Anything else: the widest formatted score, already including the
    /// leading-space allowance vanilla adds
    /// (`spacerWidth + playerScoreWidth`, or 0 when the score is empty).
    Numeric { max_score_width: i32 },
}

impl ScoreColumn {
    /// `widthForScore` in `extractRenderState`.
    fn width(self) -> i32 {
        match self {
            ScoreColumn::None => 0,
            ScoreColumn::Hearts => HEARTS_WIDTH,
            ScoreColumn::Numeric { max_score_width } => max_score_width,
        }
    }
    fn present(self) -> bool {
        !matches!(self, ScoreColumn::None)
    }
}

/// Everything the layout needs that this module will not measure itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabListInput {
    pub screen_width: i32,
    /// `this.minecraft.getConnection().onlineMode()` — offline servers draw
    /// no faces, and the slot is 9px narrower for it.
    pub show_head: bool,
    /// The widest display name over **all** listed players. One global value,
    /// not per column.
    pub max_name_width: i32,
    pub score: ScoreColumn,
    /// Width of each already-wrapped header line, top to bottom. Empty for no
    /// header. (`font.split(header, screenWidth - 50)`.)
    pub header_lines: Vec<i32>,
    pub footer_lines: Vec<i32>,
}

impl TabListInput {
    pub fn new(screen_width: i32, show_head: bool, max_name_width: i32) -> TabListInput {
        TabListInput {
            screen_width,
            show_head,
            max_name_width,
            score: ScoreColumn::None,
            header_lines: Vec::new(),
            footer_lines: Vec::new(),
        }
    }
}

/// One player's row, fully placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntrySlot {
    /// Index into the sorted, truncated list.
    pub index: usize,
    pub col: i32,
    pub row: i32,
    /// The row's translucent background.
    pub background: Rect,
    /// The 8x8 player face, when the server is online-mode.
    pub face: Option<Rect>,
    /// Top-left of the name text (vanilla draws text from the top-left of its
    /// line box, not a baseline).
    pub name: (i32, i32),
    /// `(left, right)` of the score span, present only when the objective
    /// exists, the player is not a spectator, and the span exceeds 5px.
    pub score_span: Option<(i32, i32)>,
    pub ping_icon: Rect,
}

/// The placed tab list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabListLayout {
    pub columns: i32,
    /// Rows **per column** — the balanced `ceil(n / columns)`, not 20.
    pub rows: i32,
    pub slot_width: i32,
    /// The full width the bands are drawn to. Starts as the grid's own width
    /// and grows to fit the widest header *or footer* line.
    pub max_line_width: i32,
    pub header_band: Option<Rect>,
    /// Top-left of each header line, already centred.
    pub header_line_origins: Vec<(i32, i32)>,
    pub list_band: Rect,
    pub entries: Vec<EntrySlot>,
    pub footer_band: Option<Rect>,
    pub footer_line_origins: Vec<(i32, i32)>,
}

/// `extractRenderState`'s column solve.
///
/// Verbatim, including the loop shape:
///
/// ```text
/// int rows = slots;
/// for (cols = 1; rows > 20; rows = (slots + cols - 1) / cols) { cols++; }
/// ```
///
/// The update runs *after* the body, so the first iteration bumps `cols` to 2
/// before recomputing. Algebraically this settles on the smallest `cols` with
/// `ceil(n / cols) <= 20`, i.e. `ceil(n / 20)` — **but only for `n >= 1`**.
/// At `n == 0` the loop never runs and the answer is `(1, 0)`, where the
/// closed form would divide by a zero column count. That edge is why this is
/// transcribed as the loop rather than simplified.
pub fn column_solve(slots: usize) -> (i32, i32) {
    let slots = slots as i32;
    let mut rows = slots;
    let mut cols = 1;
    while rows > MAX_ROWS_PER_COL {
        cols += 1;
        rows = (slots + cols - 1) / cols;
    }
    (cols, rows)
}

/// Lay the tab list out. Pure: integers in, integers out.
///
/// `entries` should already be `visible_entries(...)` — the layout does not
/// sort or truncate, so a caller can lay out a hand-built list in a gate.
pub fn layout(input: &TabListInput, entries: &[TabEntry]) -> TabListLayout {
    let slots = entries.len();
    let (columns, rows) = column_solve(slots);

    let width_for_score = input.score.width();
    let head_width = if input.show_head { FACE_ADVANCE } else { 0 };

    // `Math.min(cols * (...), screenWidth - 50) / cols`
    //
    // The clamp is applied to the TOTAL and the division comes after, so this
    // is not the same as clamping a per-column width — with integer
    // truncation the two differ by up to a pixel per column.
    let desired_total =
        columns * (head_width + input.max_name_width + width_for_score + SLOT_EXTRA_WIDTH);
    let slot_width = desired_total.min(input.screen_width - SCREEN_MARGIN) / columns;

    let grid_width = slot_width * columns + (columns - 1) * COLUMN_GAP;
    let xxo = input.screen_width / 2 - grid_width / 2;

    // `maxLineWidth` is widened by header AND footer lines before the header
    // band is drawn — a pass that measured only the header would draw a band
    // narrower than the footer beneath it.
    let mut max_line_width = grid_width;
    for w in input.header_lines.iter().chain(input.footer_lines.iter()) {
        max_line_width = max_line_width.max(*w);
    }

    let band = |y1: i32, y2: i32| {
        Rect::corners(
            input.screen_width / 2 - max_line_width / 2 - 1,
            y1,
            input.screen_width / 2 + max_line_width / 2 + 1,
            y2,
        )
    };

    let mut yyo = TOP_MARGIN;

    let mut header_band = None;
    let mut header_line_origins = Vec::with_capacity(input.header_lines.len());
    if !input.header_lines.is_empty() {
        header_band = Some(band(
            yyo - 1,
            yyo + input.header_lines.len() as i32 * ROW_STRIDE,
        ));
        for w in &input.header_lines {
            header_line_origins.push((input.screen_width / 2 - w / 2, yyo));
            yyo += ROW_STRIDE;
        }
        // The lone `yyo++` after the header loop — one pixel of breathing
        // room between the header band and the list band.
        yyo += 1;
    }

    let list_band = band(yyo - 1, yyo + rows * ROW_STRIDE);

    let mut placed = Vec::with_capacity(slots);
    for (index, entry) in entries.iter().enumerate() {
        let i = index as i32;
        // Column-major. `rows` is the divisor, so entries fill a column top to
        // bottom before starting the next one.
        let col = i / rows;
        let row = i % rows;
        let slot_x = xxo + col * slot_width + col * COLUMN_GAP;
        let y = yyo + row * ROW_STRIDE;

        let background = Rect {
            x: slot_x,
            y,
            w: slot_width,
            h: ROW_BACKGROUND_HEIGHT,
        };

        // Vanilla advances a local `xo` past the face, so everything after it
        // — the name and the score span — shifts right, while the ping icon
        // is explicitly given the *slot* origin back (`xo - (showHead ? 9 : 0)`).
        let (face, text_x) = if input.show_head {
            (
                Some(Rect { x: slot_x, y, w: FACE_SIZE, h: FACE_SIZE }),
                slot_x + FACE_ADVANCE,
            )
        } else {
            (None, slot_x)
        };

        let score_span = if input.score.present() && !entry.spectator {
            let left = text_x + input.max_name_width + 1;
            let right = left + width_for_score;
            // Strictly greater than 5.
            if right - left > SCORE_MIN_SPAN {
                Some((left, right))
            } else {
                None
            }
        } else {
            None
        };

        placed.push(EntrySlot {
            index,
            col,
            row,
            background,
            face,
            name: (text_x, y),
            score_span,
            ping_icon: Rect {
                x: slot_x + slot_width - PING_ICON_RIGHT_INSET,
                y,
                w: PING_ICON_W,
                h: PING_ICON_H,
            },
        });
    }

    let mut footer_band = None;
    let mut footer_line_origins = Vec::with_capacity(input.footer_lines.len());
    if !input.footer_lines.is_empty() {
        // `yyo += rows * 9 + 1` — measured from the list's top, not its band.
        yyo += rows * ROW_STRIDE + 1;
        footer_band = Some(band(
            yyo - 1,
            yyo + input.footer_lines.len() as i32 * ROW_STRIDE,
        ));
        for w in &input.footer_lines {
            footer_line_origins.push((input.screen_width / 2 - w / 2, yyo));
            yyo += ROW_STRIDE;
        }
    }

    TabListLayout {
        columns,
        rows,
        slot_width,
        max_line_width,
        header_band,
        header_line_origins,
        list_band,
        entries: placed,
        footer_band,
        footer_line_origins,
    }
}

// ───────────────────────────────────────────────── M155: RenderType::HEARTS

/// One heart position's sprite, in draw order (M155).
///
/// Every position draws a **container** first and then at most one fill over
/// it, because `extractTablistHearts`'s second loop opens with the same
/// unconditional `blitSprite(sprite, …)` the first loop uses. A filled heart is
/// therefore two layers, not one, and an emitter that drew only the fill would
/// leave the empty half of a half-heart transparent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeartSprite {
    Container,
    ContainerBlinking,
    Full,
    Half,
    /// The blink ghost, drawn from `displayedValue` rather than the live score.
    FullBlinking,
    HalfBlinking,
    /// Hearts at index >= 10 — absorption.
    ///
    /// **These are the `*_blinking` assets and they are NOT the blink layer.**
    /// Vanilla ships no non-blinking absorbing sprite, so
    /// `HEART_ABSORBING_FULL_BLINKING_SPRITE` is the ordinary appearance of a
    /// gold heart. Transcribing by name — "a blinking sprite is drawn only
    /// while blinking" — loses gold hearts entirely.
    AbsorbingFull,
    AbsorbingHalf,
}

/// `PlayerTabOverlay.HealthState` — the per-player blink clock (M155).
///
/// Vanilla keeps one of these per profile id in a map that is **cleared when
/// the list is hidden** (`reset()`), which is why a blink does not survive
/// closing and reopening the tab list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HealthState {
    last_value: i32,
    displayed_value: i32,
    last_update_tick: i64,
    blink_until_tick: i64,
}

/// `DISPLAY_UPDATE_DELAY`.
pub const HEALTH_DISPLAY_UPDATE_DELAY: i64 = 20;
/// `DECREASE_BLINK_DURATION` — losing health blinks for twice as long as
/// gaining it.
pub const HEALTH_DECREASE_BLINK: i64 = 20;
/// `INCREASE_BLINK_DURATION`.
pub const HEALTH_INCREASE_BLINK: i64 = 10;

impl HealthState {
    /// `new HealthState(value)` — both fields seeded, so a player first seen at
    /// any health is **not** blinking and has nothing stale to catch up to.
    pub fn new(value: i32) -> Self {
        Self {
            last_value: value,
            displayed_value: value,
            last_update_tick: 0,
            blink_until_tick: 0,
        }
    }

    /// `update(value, tick)`.
    ///
    /// Two independent halves, and the second is **not** in an `else`: a change
    /// arms the blink, and separately the displayed value catches up once
    /// twenty ticks have passed since the last change. So a second change
    /// **restarts the catch-up clock**, and a value that flickers keeps
    /// `displayedValue` stale indefinitely.
    pub fn update(&mut self, value: i32, tick: i64) {
        if value != self.last_value {
            // A DECREASE blinks 20 and an increase 10 — losing health is
            // twice as loud as gaining it.
            self.blink_until_tick = tick
                + if value < self.last_value {
                    HEALTH_DECREASE_BLINK
                } else {
                    HEALTH_INCREASE_BLINK
                };
            self.last_value = value;
            self.last_update_tick = tick;
        }
        // Strictly greater — at exactly 20 ticks it has NOT caught up yet.
        //
        // **The two halves cannot both fire on one tick, and that is a
        // coincidence of vanilla's own control flow rather than a rule stated
        // anywhere.** The branch above sets `last_update_tick = tick`, so this
        // test reads `0 > 20` whenever the value changed. Writing the two as
        // an `if`/`else` is therefore EQUIVALENT — a mutation adding an early
        // return here survives the battery, correctly. It is left as two
        // statements because that is what the decompile says, and the
        // coincidence is pinned by
        // `a_change_never_catches_up_on_the_same_tick` so a later reader who
        // does collapse them can see it was already known.
        if tick - self.last_update_tick > HEALTH_DISPLAY_UPDATE_DELAY {
            self.displayed_value = value;
        }
    }

    pub fn displayed_value(self) -> i32 {
        self.displayed_value
    }

    /// `isBlinking(tick)` — `blinkUntil > tick && (blinkUntil - tick) % 6 >= 3`.
    ///
    /// **A six-tick square wave measured from the END of the blink, not the
    /// start**, which inverts the phase against the obvious reading. A decrease
    /// arms `tick + 20`, and `20 % 6 == 2`, so the heart is DARK on the tick
    /// the damage lands; an increase arms `tick + 10`, and `10 % 6 == 4`, so
    /// that one is LIT immediately. The two directions start on opposite
    /// phases and neither starts at the beginning of a cycle.
    pub fn is_blinking(self, tick: i64) -> bool {
        self.blink_until_tick > tick && (self.blink_until_tick - tick).rem_euclid(6) >= 3
    }
}

/// `Mth.positiveCeilDiv(input, divisor)` — `-floorDiv(-input, divisor)`.
fn positive_ceil_div(input: i32, divisor: i32) -> i32 {
    -(-input).div_euclid(divisor)
}

/// The two heart counts, which round in **opposite directions** (M155).
///
/// `fullHearts` CEILS and `heartsToRender` FLOORS, so at an odd value they
/// disagree — and `fullHearts` can **exceed** `heartsToRender` (score 21 gives
/// 11 against 10), at which point the container-only loop
/// `for heart in full..render` runs zero times. Reading them as one number,
/// or as ceil/ceil, silently drops the empty containers past the fill.
pub fn heart_counts(score: i32, displayed: i32) -> (i32, i32) {
    let full_hearts = positive_ceil_div(score.max(displayed), 2);
    let hearts_to_render = score.max(displayed.max(20)) / 2;
    (full_hearts, hearts_to_render)
}

/// `widthPerHeart` — `floor(min((right - left - 4) / heartsToRender, 9))`.
///
/// **At the default column this is 8, with 9-px sprites**, so every heart
/// overlaps its neighbour by a column. The `9.0` cap is unreachable at
/// [`HEARTS_WIDTH`]: `floor(min(86 / 10, 9)) == 8`. It exists for a
/// hypothetically wider column and is transcribed rather than dropped.
pub fn width_per_heart(left: i32, right: i32, hearts_to_render: i32) -> i32 {
    if hearts_to_render <= 0 {
        return 0;
    }
    let per = (right - left - 4) as f32 / hearts_to_render as f32;
    per.min(9.0).floor() as i32
}

/// One heart blit: an x offset from `left`, and its sprite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeartBlit {
    pub dx: i32,
    pub sprite: HeartSprite,
}

/// `extractTablistHearts`'s sprite half, in draw order (M155).
///
/// Returns an empty vec when the column is too narrow — that is the **text
/// readout** case, which the caller renders instead; see
/// [`hearts_text_readout`].
///
/// The whole body is gated on `fullHearts > 0`, so a dead player (score 0 with
/// a caught-up display) draws **nothing at all** rather than ten empty
/// containers.
pub fn heart_blits(score: i32, health: HealthState, tick: i64, left: i32, right: i32) -> Vec<HeartBlit> {
    let displayed = health.displayed_value();
    let (full_hearts, hearts_to_render) = heart_counts(score, displayed);
    if full_hearts <= 0 {
        return Vec::new();
    }
    let per = width_per_heart(left, right, hearts_to_render);
    if per <= 3 {
        return Vec::new();
    }
    let blink = health.is_blinking(tick);
    let container = if blink {
        HeartSprite::ContainerBlinking
    } else {
        HeartSprite::Container
    };
    let mut out = Vec::new();

    // The containers PAST the fill. Runs zero times when `fullHearts` exceeds
    // `heartsToRender`, which an odd score reaches.
    for heart in full_hearts..hearts_to_render {
        out.push(HeartBlit { dx: heart * per, sprite: container });
    }
    for heart in 0..full_hearts {
        // Every filled position gets its container too — the second loop opens
        // with the same unconditional blit.
        out.push(HeartBlit { dx: heart * per, sprite: container });
        if blink {
            // The GHOST layer, driven by `displayedValue` rather than the live
            // score — which is what makes a drop show where the health *was*.
            if heart * 2 + 1 < displayed {
                out.push(HeartBlit { dx: heart * per, sprite: HeartSprite::FullBlinking });
            }
            if heart * 2 + 1 == displayed {
                out.push(HeartBlit { dx: heart * per, sprite: HeartSprite::HalfBlinking });
            }
        }
        if heart * 2 + 1 < score {
            out.push(HeartBlit {
                dx: heart * per,
                sprite: if heart >= 10 {
                    HeartSprite::AbsorbingFull
                } else {
                    HeartSprite::Full
                },
            });
        }
        if heart * 2 + 1 == score {
            out.push(HeartBlit {
                dx: heart * per,
                sprite: if heart >= 10 {
                    HeartSprite::AbsorbingHalf
                } else {
                    HeartSprite::Half
                },
            });
        }
    }
    out
}

/// The `widthPerHeart <= 3` text readout — its string and its colour (M155).
///
/// **Reached by HIGH HEALTH, never by a narrow window.** The column is a
/// constant [`HEARTS_WIDTH`] (90), so `right - left` never varies and the only
/// free variable is `heartsToRender`: `floor(86 / n) <= 3` iff `n >= 22`, i.e.
/// `max(score, max(displayed, 20)) >= 44`. A reader who assumes this is the
/// narrow-window fallback will look for it at small window sizes and never
/// find it.
///
/// The colour ramps red to green over `score / 20` and is **opaque black in
/// the blue channel** — `(1-pct)*255 << 16 | pct*255 << 8` has no blue term at
/// all, so full health is pure `0x00FF00` rather than a white-ish green.
pub fn hearts_text_readout(score: i32) -> Option<(f32, i32)> {
    let hearts_to_render = score.max(20) / 2;
    if width_per_heart(0, HEARTS_WIDTH, hearts_to_render) > 3 {
        return None;
    }
    let pct = (score as f32 / 20.0).clamp(0.0, 1.0);
    let color = (((1.0 - pct) * 255.0) as i32) << 16 | ((pct * 255.0) as i32) << 8;
    Some((score as f32 / 2.0, color))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(names: &[&str]) -> Vec<TabEntry> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| TabEntry::new(i as u128, *n, Some(0)))
            .collect()
    }

    /// Sort, then read the names back out — owned, so callers can compare
    /// against a literal array without borrowing a temporary.
    fn sorted_names(entries: &[TabEntry]) -> Vec<String> {
        visible_entries(entries)
            .into_iter()
            .map(|e| e.name)
            .collect()
    }

    fn n_players(n: usize) -> Vec<TabEntry> {
        (0..n)
            .map(|i| TabEntry::new(i as u128, format!("p{:03}", i), Some(0)))
            .collect()
    }

    /// The two name colours, derived from vanilla's own signed literals.
    ///
    /// `graphics.text(font, name, xo, yo, info.getGameMode() == SPECTATOR ?
    /// -1862270977 : -1)`. Deriving them from the `i32` is the point: a test
    /// that restated the hex would pass against whatever hex the constant
    /// happened to hold, which is exactly how M52f's `0x9099_9999` survived.
    #[test]
    fn a_spectators_name_is_white_at_alpha_144_not_grey() {
        assert_eq!(NAME_COLOR, (-1i32) as u32);
        assert_eq!(SPECTATOR_NAME_COLOR, (-1862270977i32) as u32);
        // The RGB half is FULL white; only the alpha differs from a normal
        // player's. A grey reading dims the letters as well as fading them.
        assert_eq!(SPECTATOR_NAME_COLOR & 0x00FF_FFFF, 0x00FF_FFFF);
        assert_eq!(SPECTATOR_NAME_COLOR >> 24, 144);
    }

    // ── ping buckets ──────────────────────────────────────────────────────

    #[test]
    fn a_negative_latency_is_the_only_unknown_ping() {
        assert_eq!(ping_icon(-1), PingIcon::Unknown);
        assert_eq!(ping_icon(-9999), PingIcon::Unknown);
        // Zero is a perfect connection, not a missing one.
        assert_eq!(ping_icon(0), PingIcon::Ping5);
        // And there is no upper Unknown — a terrible ping is still one bar.
        assert_eq!(ping_icon(600_000), PingIcon::Ping1);
    }

    #[test]
    fn the_ping_thresholds_are_strictly_less_than_at_150_300_600_and_1000() {
        // Each boundary value belongs to the *worse* bucket, because every
        // comparison in extractPingIcon is `<`, not `<=`.
        assert_eq!(ping_icon(149), PingIcon::Ping5);
        assert_eq!(ping_icon(150), PingIcon::Ping4);
        assert_eq!(ping_icon(299), PingIcon::Ping4);
        assert_eq!(ping_icon(300), PingIcon::Ping3);
        assert_eq!(ping_icon(599), PingIcon::Ping3);
        assert_eq!(ping_icon(600), PingIcon::Ping2);
        assert_eq!(ping_icon(999), PingIcon::Ping2);
        assert_eq!(ping_icon(1000), PingIcon::Ping1);
    }

    #[test]
    fn more_bars_means_less_latency_not_more() {
        // Sensitivity partner: the sprite numbering runs opposite to the
        // latency it represents, so a "higher number = worse" reading would
        // pass a bucket-boundary test and still draw every icon inverted.
        assert_eq!(ping_icon(10), PingIcon::Ping5);
        assert_eq!(ping_icon(5000), PingIcon::Ping1);
        assert_eq!(PingIcon::Ping5.sprite(), "icon/ping_5");
        assert_eq!(PingIcon::Unknown.sprite(), "icon/ping_unknown");
    }

    #[test]
    fn an_entry_without_a_ping_reads_as_unknown() {
        assert_eq!(TabEntry::new(1, "a", None).ping_icon(), PingIcon::Unknown);
        assert_eq!(TabEntry::new(1, "a", Some(42)).ping_icon(), PingIcon::Ping5);
    }

    // ── the column solve ──────────────────────────────────────────────────

    #[test]
    fn twenty_players_still_fit_one_column() {
        // The loop condition is `rows > 20`, so exactly 20 does not overflow.
        assert_eq!(column_solve(1), (1, 1));
        assert_eq!(column_solve(19), (1, 19));
        assert_eq!(column_solve(20), (1, 20));
        assert_eq!(column_solve(21), (2, 11));
    }

    #[test]
    fn columns_are_balanced_rather_than_filled_to_twenty() {
        // 21 players are 11 + 10, NOT 20 + 1. This is the rule most likely to
        // be "simplified" into filling each column before starting the next.
        assert_eq!(column_solve(21), (2, 11));
        assert_eq!(column_solve(30), (2, 15));
        assert_eq!(column_solve(41), (3, 14));
        assert_eq!(column_solve(61), (4, 16));
    }

    #[test]
    fn the_column_solve_matches_a_hand_walked_table() {
        // Walked by hand through the Java loop, one row per interesting shape:
        // the exit condition tests the PREVIOUS iteration's `rows`, and the
        // update recomputes `rows` with the ALREADY-incremented `cols`. A
        // version that recomputed before incrementing lands one column short
        // on every overflowing size.
        const TABLE: &[(usize, i32, i32)] = &[
            (0, 1, 0),   // loop never runs
            (1, 1, 1),
            (20, 1, 20), // `rows > 20` is false at exactly 20
            (21, 2, 11), // not 20 + 1
            (39, 2, 20),
            (40, 2, 20), // exactly fills two columns
            (41, 3, 14), // 41 -> cols 2 gives 21, still > 20 -> cols 3
            (60, 3, 20),
            (61, 4, 16),
            (79, 4, 20),
            (80, 4, 20), // the cap, exactly four full columns
        ];
        for &(slots, cols, rows) in TABLE {
            assert_eq!(column_solve(slots), (cols, rows), "slots={slots}");
        }
    }

    #[test]
    fn the_column_count_matches_ceil_over_twenty_for_every_nonempty_size() {
        // The loop settles on the smallest cols with ceil(n/cols) <= 20, which
        // is ceil(n/20). Proving the equivalence over the whole live range
        // guards a rewrite in either direction.
        for n in 1..=MAX_ENTRIES {
            let (cols, rows) = column_solve(n);
            let expected = (n as i32 + MAX_ROWS_PER_COL - 1) / MAX_ROWS_PER_COL;
            assert_eq!(cols, expected, "n={n}");
            assert_eq!(rows, (n as i32 + cols - 1) / cols, "n={n}");
            assert!(rows <= MAX_ROWS_PER_COL, "n={n} rows={rows}");
            assert!(cols * rows >= n as i32, "n={n} grid too small");
        }
    }

    #[test]
    fn an_empty_list_gives_one_column_of_zero_rows_not_a_division_by_zero() {
        // Sensitivity partner for the closed form: ceil(0/20) is 0 columns,
        // and `i / rows` with rows = 0 would panic. The loop's answer is (1, 0).
        assert_eq!(column_solve(0), (1, 0));
        let l = layout(&TabListInput::new(854, true, 60), &[]);
        assert_eq!((l.columns, l.rows), (1, 0));
        assert!(l.entries.is_empty());
    }

    #[test]
    fn eighty_players_fill_four_columns_of_twenty() {
        assert_eq!(column_solve(MAX_ENTRIES), (4, 20));
    }

    // ── the sort ──────────────────────────────────────────────────────────

    #[test]
    fn names_sort_case_insensitively() {
        let e = named(&["zeta", "Alpha", "beta"]);
        let got = sorted_names(&e);
        assert_eq!(got, ["Alpha", "beta", "zeta"]);
    }

    #[test]
    fn a_case_sensitive_name_sort_would_put_uppercase_first_and_does_not() {
        // Sensitivity partner: plain byte ordering puts every capital ahead of
        // every lowercase, so "Zeta" would lead. compareToIgnoreCase does not.
        let e = named(&["apple", "Zeta", "Banana"]);
        let got = sorted_names(&e);
        assert_eq!(got, ["apple", "Banana", "Zeta"]);
        assert_ne!(got[0], "Banana");
    }

    #[test]
    fn team_outranks_name_and_no_team_sorts_first() {
        let mut a = TabEntry::new(1, "aaa", Some(0));
        a.team = Some("zebra".into());
        let mut b = TabEntry::new(2, "zzz", Some(0));
        b.team = None; // -> "" , which precedes every named team
        let got = sorted_names(&[a, b]);
        assert_eq!(got, ["zzz", "aaa"]);
    }

    #[test]
    fn the_team_key_is_case_sensitive_unlike_the_name_key() {
        // Sensitivity partner: only the NAME comparator ignores case. Folding
        // the team key too would swap these, and looks entirely reasonable.
        let mut a = TabEntry::new(1, "a", Some(0));
        a.team = Some("apple".into());
        let mut b = TabEntry::new(2, "b", Some(0));
        b.team = Some("Zebra".into());
        // 'Z' (0x5A) < 'a' (0x61) under String.compareTo.
        let got = sorted_names(&[a, b]);
        assert_eq!(got, ["b", "a"]);
    }

    #[test]
    fn spectators_sort_after_everyone_regardless_of_name() {
        let mut spec = TabEntry::new(1, "aaa", Some(0));
        spec.spectator = true;
        let alive = TabEntry::new(2, "zzz", Some(0));
        let got = sorted_names(&[spec, alive]);
        assert_eq!(got, ["zzz", "aaa"]);
    }

    #[test]
    fn a_higher_tab_list_order_sorts_first_because_the_key_is_negated() {
        let mut lo = TabEntry::new(1, "aaa", Some(0));
        lo.tab_list_order = 1;
        let mut hi = TabEntry::new(2, "zzz", Some(0));
        hi.tab_list_order = 5;
        let got = sorted_names(&[lo, hi]);
        // Sensitivity partner for a dropped minus sign: without it "aaa"
        // (order 1) would lead on both the order key and the name key, so the
        // bug would be invisible unless the higher order also has a later name.
        assert_eq!(got, ["zzz", "aaa"]);
    }

    #[test]
    fn the_tab_list_order_key_wraps_rather_than_overflowing_on_int_min() {
        // Java's `-p.getTabListOrder()` is int negation: -MIN_VALUE == MIN_VALUE.
        // A saturating or checked negation would order this differently, and a
        // plain `-x` in Rust panics in debug.
        let mut a = TabEntry::new(1, "a", Some(0));
        a.tab_list_order = i32::MIN;
        let mut b = TabEntry::new(2, "b", Some(0));
        b.tab_list_order = 0;
        assert_eq!(compare_entries(&a, &b), Ordering::Less);
    }

    #[test]
    fn the_sort_keys_apply_in_order_and_the_local_player_is_not_one_of_them() {
        // `local` must not float you to the top — vanilla has no such key.
        let mut me = TabEntry::new(1, "zzz", Some(0)).local(true);
        me.spectator = false;
        let them = TabEntry::new(2, "aaa", Some(0));
        let got = sorted_names(&[me, them]);
        assert_eq!(got, ["aaa", "zzz"]);
    }

    #[test]
    fn ties_keep_arrival_order_because_the_sort_is_stable() {
        let a = TabEntry::new(10, "same", Some(0));
        let b = TabEntry::new(20, "same", Some(0));
        let c = TabEntry::new(30, "same", Some(0));
        let got: Vec<u128> = visible_entries(&[a, b, c]).iter().map(|x| x.uuid).collect();
        assert_eq!(got, [10, 20, 30]);
    }

    #[test]
    fn the_eighty_cap_keeps_the_first_eighty_after_sorting_not_before() {
        // 100 players named in DESCENDING order. Truncating before sorting
        // would keep p099..p000 and show the wrong half of the server.
        let mut e: Vec<TabEntry> = (0..100)
            .map(|i| TabEntry::new(i as u128, format!("p{:03}", 99 - i), Some(0)))
            .collect();
        e.reverse();
        let out = visible_entries(&e);
        assert_eq!(out.len(), MAX_ENTRIES);
        assert_eq!(out[0].name, "p000");
        assert_eq!(out[MAX_ENTRIES - 1].name, "p079");
    }

    // ── layout ────────────────────────────────────────────────────────────

    #[test]
    fn entries_are_laid_out_column_major() {
        // With 21 players (2 columns of 11), index 11 must start the SECOND
        // column at the top — not sit in the second row of the first.
        let e = n_players(21);
        let l = layout(&TabListInput::new(854, true, 60), &e);
        assert_eq!((l.columns, l.rows), (2, 11));
        assert_eq!((l.entries[0].col, l.entries[0].row), (0, 0));
        assert_eq!((l.entries[10].col, l.entries[10].row), (0, 10));
        assert_eq!((l.entries[11].col, l.entries[11].row), (1, 0));
        assert_eq!((l.entries[20].col, l.entries[20].row), (1, 9));
    }

    #[test]
    fn row_major_would_place_index_one_in_the_second_column_and_does_not() {
        // Sensitivity partner: `col = i % cols; row = i / cols` is the obvious
        // alternative and produces an identical single-column screenshot.
        let e = n_players(21);
        let l = layout(&TabListInput::new(854, true, 60), &e);
        assert_eq!(l.entries[1].col, 0, "index 1 belongs under index 0");
        assert_eq!(l.entries[1].row, 1);
        assert!(l.entries[1].background.y > l.entries[0].background.y);
        assert_eq!(l.entries[1].background.x, l.entries[0].background.x);
    }

    #[test]
    fn columns_are_spaced_by_the_slot_width_plus_five() {
        let e = n_players(21);
        let l = layout(&TabListInput::new(854, true, 60), &e);
        let c0 = l.entries[0].background.x;
        let c1 = l.entries[11].background.x;
        assert_eq!(c1 - c0, l.slot_width + COLUMN_GAP);
    }

    #[test]
    fn rows_are_nine_apart_but_only_eight_tall() {
        // The 1px difference is the visible gap between rows; equalising them
        // would fuse the list into one block.
        let e = n_players(3);
        let l = layout(&TabListInput::new(854, true, 60), &e);
        assert_eq!(
            l.entries[1].background.y - l.entries[0].background.y,
            ROW_STRIDE
        );
        assert_eq!(l.entries[0].background.h, ROW_BACKGROUND_HEIGHT);
        assert_ne!(ROW_STRIDE, ROW_BACKGROUND_HEIGHT);
    }

    #[test]
    fn the_slot_width_clamp_divides_after_the_min_not_before() {
        // `min(cols * per, screenWidth - 50) / cols`. Clamping a per-column
        // width instead would give `min(per, (screenWidth - 50) / cols)`,
        // which differs under integer truncation. Pick a case where it does:
        // 3 columns, screen 400 -> (400 - 50) = 350; 350 / 3 = 116, whereas
        // clamping per-column then flooring gives the same 116 only by luck,
        // so assert against the transcribed formula directly.
        let e = n_players(41); // 3 columns
        let input = TabListInput::new(400, true, 500); // deliberately over-wide
        let l = layout(&input, &e);
        assert_eq!(l.columns, 3);
        let desired = 3 * (FACE_ADVANCE + 500 + 0 + SLOT_EXTRA_WIDTH);
        assert_eq!(l.slot_width, desired.min(400 - SCREEN_MARGIN) / 3);
        assert_eq!(l.slot_width, 350 / 3);
    }

    #[test]
    fn a_narrow_list_keeps_its_natural_width_instead_of_stretching() {
        // The clamp is a maximum, not a target: a short list does not expand
        // to fill `screenWidth - 50`.
        let e = n_players(2);
        let l = layout(&TabListInput::new(1920, true, 40), &e);
        assert_eq!(l.slot_width, FACE_ADVANCE + 40 + SLOT_EXTRA_WIDTH);
        assert!(l.slot_width < 1920 - SCREEN_MARGIN);
    }

    #[test]
    fn hiding_the_face_narrows_the_slot_by_nine_and_shifts_the_name_left() {
        let e = n_players(2);
        let with = layout(&TabListInput::new(854, true, 60), &e);
        let without = layout(&TabListInput::new(854, false, 60), &e);
        assert_eq!(with.slot_width - without.slot_width, FACE_ADVANCE);
        assert!(with.entries[0].face.is_some());
        assert!(without.entries[0].face.is_none());
        // The face is 8 wide but advances 9.
        assert_eq!(with.entries[0].face.unwrap().w, FACE_SIZE);
        assert_eq!(
            with.entries[0].name.0 - with.entries[0].background.x,
            FACE_ADVANCE
        );
        assert_eq!(
            without.entries[0].name.0 - without.entries[0].background.x,
            0
        );
    }

    #[test]
    fn the_ping_icon_sits_against_the_slots_right_edge_not_the_names() {
        // Vanilla hands extractPingIcon the SLOT origin back (`xo - 9` when a
        // face was drawn), so the icon does not shift with the face.
        let e = n_players(2);
        for show_head in [true, false] {
            let l = layout(&TabListInput::new(854, show_head, 60), &e);
            let s = &l.entries[0];
            assert_eq!(
                s.ping_icon.x,
                s.background.x + l.slot_width - PING_ICON_RIGHT_INSET
            );
            // 10 wide inset by 11 leaves exactly one pixel of right margin.
            assert_eq!(s.background.right() - s.ping_icon.right(), 1);
            assert_eq!((s.ping_icon.w, s.ping_icon.h), (PING_ICON_W, PING_ICON_H));
            assert_eq!(s.ping_icon.y, s.background.y);
        }
    }

    #[test]
    fn the_grid_is_centred_on_the_screen() {
        let e = n_players(21);
        let l = layout(&TabListInput::new(854, true, 60), &e);
        let grid_w = l.slot_width * l.columns + (l.columns - 1) * COLUMN_GAP;
        assert_eq!(l.entries[0].background.x, 854 / 2 - grid_w / 2);
    }

    #[test]
    fn the_list_starts_ten_pixels_down_when_there_is_no_header() {
        let e = n_players(3);
        let l = layout(&TabListInput::new(854, true, 60), &e);
        assert_eq!(l.entries[0].background.y, TOP_MARGIN);
        assert_eq!(l.list_band.y, TOP_MARGIN - 1);
        assert_eq!(l.list_band.h, 1 + l.rows * ROW_STRIDE);
        assert!(l.header_band.is_none());
        assert!(l.footer_band.is_none());
    }

    #[test]
    fn a_header_pushes_the_list_down_by_nine_per_line_plus_one() {
        // The trailing `yyo++` after the header loop is a single pixel and is
        // easy to drop; without it every row sits one pixel high.
        let e = n_players(3);
        let mut input = TabListInput::new(854, true, 60);
        input.header_lines = vec![40, 40];
        let l = layout(&input, &e);
        assert_eq!(l.entries[0].background.y, TOP_MARGIN + 2 * ROW_STRIDE + 1);
        assert_eq!(l.header_line_origins.len(), 2);
        assert_eq!(l.header_line_origins[0].1, TOP_MARGIN);
        assert_eq!(l.header_line_origins[1].1, TOP_MARGIN + ROW_STRIDE);
        let hb = l.header_band.unwrap();
        assert_eq!(hb.y, TOP_MARGIN - 1);
        assert_eq!(hb.bottom(), TOP_MARGIN + 2 * ROW_STRIDE);
    }

    #[test]
    fn the_footer_starts_one_pixel_below_the_last_row() {
        let e = n_players(3);
        let mut input = TabListInput::new(854, true, 60);
        input.footer_lines = vec![40];
        let l = layout(&input, &e);
        let expected = TOP_MARGIN + l.rows * ROW_STRIDE + 1;
        assert_eq!(l.footer_line_origins[0].1, expected);
        assert_eq!(l.footer_band.unwrap().y, expected - 1);
    }

    #[test]
    fn the_bands_widen_to_fit_a_footer_line_not_just_a_header_line() {
        // Sensitivity partner: maxLineWidth is finished before ANY band is
        // drawn, so a wide footer widens the header's band too. Measuring the
        // header alone first is the natural mistake and looks fine until a
        // server sets a long footer.
        let e = n_players(3);
        let mut input = TabListInput::new(854, true, 60);
        input.header_lines = vec![10];
        input.footer_lines = vec![700];
        let l = layout(&input, &e);
        assert_eq!(l.max_line_width, 700);
        assert_eq!(l.header_band.unwrap().w, l.footer_band.unwrap().w);
        assert_eq!(l.list_band.w, l.footer_band.unwrap().w);
    }

    #[test]
    fn the_bands_never_shrink_below_the_grid_width() {
        let e = n_players(21);
        let mut input = TabListInput::new(854, true, 60);
        input.header_lines = vec![5];
        let l = layout(&input, &e);
        let grid_w = l.slot_width * l.columns + (l.columns - 1) * COLUMN_GAP;
        assert_eq!(l.max_line_width, grid_w);
        assert!(l.list_band.w >= grid_w);
    }

    #[test]
    fn an_odd_band_width_loses_a_pixel_to_truncation_rather_than_gaining_one() {
        // The band is two centre-relative corners,
        // `sw/2 - mlw/2 - 1` .. `sw/2 + mlw/2 + 1`, so its width is
        // `2 * (mlw / 2) + 2` — which is `mlw + 2` only when `mlw` is EVEN.
        // Odd widths truncate to `mlw + 1`, and the band sits a pixel further
        // right of centre than left. "Overhangs by one on each side" is the
        // plausible reading and is wrong half the time; this pins both parities
        // so a future tidy-up to `mlw + 2` fails here instead of shipping a
        // one-pixel drift.
        let e = n_players(1);
        let band_w = |line: i32| {
            let mut input = TabListInput::new(854, true, 60);
            input.header_lines = vec![line];
            let l = layout(&input, &e);
            assert_eq!(l.max_line_width, line, "fixture must be the widest");
            l.list_band.w
        };
        assert_eq!(band_w(700), 702); // even -> mlw + 2
        assert_eq!(band_w(701), 702); // odd  -> mlw + 1, the same band
    }

    #[test]
    fn a_score_column_appears_only_when_it_exceeds_five_pixels() {
        // `if (right - left > 5)` — strictly greater, so a 5px column draws
        // nothing at all.
        let e = n_players(1);
        for (w, want) in [(0, false), (5, false), (6, true)] {
            let mut input = TabListInput::new(854, true, 60);
            input.score = ScoreColumn::Numeric { max_score_width: w };
            let l = layout(&input, &e);
            assert_eq!(l.entries[0].score_span.is_some(), want, "width {w}");
        }
    }

    #[test]
    fn hearts_pin_the_score_column_to_ninety_regardless_of_text_width() {
        let e = n_players(1);
        let mut input = TabListInput::new(1920, true, 60);
        input.score = ScoreColumn::Hearts;
        let l = layout(&input, &e);
        let (left, right) = l.entries[0].score_span.unwrap();
        assert_eq!(right - left, HEARTS_WIDTH);
        // And the slot is widened to hold it.
        assert_eq!(
            l.slot_width,
            FACE_ADVANCE + 60 + HEARTS_WIDTH + SLOT_EXTRA_WIDTH
        );
    }

    #[test]
    fn a_spectator_gets_no_score_even_with_an_objective() {
        let mut e = n_players(1);
        e[0].spectator = true;
        let mut input = TabListInput::new(854, true, 60);
        input.score = ScoreColumn::Hearts;
        let l = layout(&input, &e);
        assert!(l.entries[0].score_span.is_none());
    }

    #[test]
    fn the_score_span_starts_one_pixel_past_the_global_name_column() {
        // `left = xo + maxNameWidth + 1` uses the GLOBAL max name width, so
        // every score in every column lines up regardless of its own name.
        let e = n_players(2);
        let mut input = TabListInput::new(854, true, 60);
        input.score = ScoreColumn::Numeric { max_score_width: 20 };
        let l = layout(&input, &e);
        let s = &l.entries[0];
        assert_eq!(s.score_span.unwrap().0, s.name.0 + 60 + 1);
    }

    #[test]
    fn every_slot_is_inside_its_own_column_band() {
        // A whole-grid invariant: nothing overlaps and nothing escapes.
        let e = n_players(80);
        let l = layout(&TabListInput::new(854, true, 60), &e);
        assert_eq!(l.entries.len(), 80);
        for s in &l.entries {
            let expected_x =
                l.entries[0].background.x + s.col * (l.slot_width + COLUMN_GAP);
            assert_eq!(s.background.x, expected_x);
            assert_eq!(s.background.y, l.list_band.y + 1 + s.row * ROW_STRIDE);
            assert!(s.row < l.rows);
            assert!(s.col < l.columns);
            assert!(s.ping_icon.right() <= s.background.right());
        }
    }
}

#[cfg(test)]
mod hearts_tests {
    use super::*;

    /// **The two counts round in OPPOSITE directions**, and the consequence is
    /// that `fullHearts` can exceed `heartsToRender`.
    ///
    /// Swept rather than spot-checked, against literals re-declared from
    /// `PlayerTabOverlay:274-275` — so this grades the transcription and not
    /// itself.
    #[test]
    fn the_two_heart_counts_round_two_different_ways() {
        let mut seen_full_exceeds_render = false;
        for score in 0..=60 {
            for displayed in 0..=60 {
                let (full, render) = heart_counts(score, displayed);
                assert_eq!(
                    full,
                    -(-(score.max(displayed))).div_euclid(2),
                    "fullHearts CEILS: score {score} displayed {displayed}"
                );
                assert_eq!(
                    render,
                    score.max(displayed.max(20)) / 2,
                    "heartsToRender FLOORS: score {score} displayed {displayed}"
                );
                if full > render {
                    seen_full_exceeds_render = true;
                }
            }
        }
        assert!(
            seen_full_exceeds_render,
            "the sweep never reached the case the two roundings exist to create"
        );
        // The named example, so a future reader has one without re-running it.
        assert_eq!(heart_counts(21, 21), (11, 10));
    }

    /// At the default column the pitch is 8 with 9-px sprites, so hearts
    /// overlap — and the 9.0 cap is unreachable there.
    #[test]
    fn the_pitch_is_eight_and_the_hearts_overlap() {
        assert_eq!(width_per_heart(0, HEARTS_WIDTH, 10), 8);
        assert!(
            width_per_heart(0, HEARTS_WIDTH, 10) < 9,
            "the sprite is 9 wide on an 8 pitch: every heart overlaps its neighbour"
        );
        // The cap IS reachable on a wider column, which is why it is kept.
        assert_eq!(width_per_heart(0, 200, 10), 9);
    }

    /// **The text readout is reached by HIGH HEALTH, not by a narrow window.**
    ///
    /// The column is a constant 90, so `right - left` never varies: the only
    /// way to `widthPerHeart <= 3` is a score at or above 44.
    #[test]
    fn the_text_readout_is_reached_by_high_health() {
        assert!(hearts_text_readout(42).is_none(), "42 still draws sprites");
        assert!(hearts_text_readout(44).is_some(), "44 crosses to text");
        // And at 44 the emitter really does produce nothing to blit.
        assert!(heart_blits(44, HealthState::new(44), 0, 0, HEARTS_WIDTH).is_empty());
        // A SEPARATE state — reusing the 44 one leaves `displayed` at 44, which
        // pushes `heartsToRender` to 22 and crosses the threshold anyway. The
        // first draft of this witness did exactly that and "passed its claim"
        // for the wrong reason.
        assert!(!heart_blits(42, HealthState::new(42), 0, 0, HEARTS_WIDTH).is_empty());
    }

    /// Full health is PURE GREEN: the colour expression has no blue term.
    #[test]
    fn the_readout_colour_has_no_blue_channel() {
        let (hearts, color) = hearts_text_readout(44).expect("text at 44");
        assert_eq!(hearts, 22.0, "the readout is in HEARTS, not points");
        assert_eq!(color & 0xFF, 0, "no blue term exists in the expression");
        // A clamped pct of 1.0 gives 0x00FF00 exactly.
        assert_eq!(color, 0x00FF00);
    }

    /// Zero health draws NOTHING — the whole body is gated on `fullHearts > 0`.
    #[test]
    fn zero_health_draws_nothing_rather_than_ten_containers() {
        let h = HealthState::new(0);
        assert!(heart_blits(0, h, 0, 0, HEARTS_WIDTH).is_empty());
        // ...and one point of health draws a half heart over a container.
        assert!(!heart_blits(1, HealthState::new(1), 0, 0, HEARTS_WIDTH).is_empty());
    }

    /// **A damage blink starts DARK and a heal blink starts LIT.**
    ///
    /// `isBlinking` measures the square wave from the END of the blink, so the
    /// two durations (20 and 10) land on opposite phases of the six-tick cycle
    /// — `20 % 6 == 2` (below the 3 threshold) against `10 % 6 == 4`. Any
    /// implementation measuring from the START gets both of these backwards.
    #[test]
    fn a_damage_blink_starts_dark_and_a_heal_blink_starts_lit() {
        let mut hurt = HealthState::new(20);
        hurt.update(19, 100);
        assert!(!hurt.is_blinking(100), "a decrease is DARK on the tick it lands");

        let mut healed = HealthState::new(20);
        healed.update(21, 100);
        assert!(healed.is_blinking(100), "an increase is LIT on the tick it lands");
    }

    /// The blink ends, and it is a square wave rather than a fade.
    #[test]
    fn the_blink_is_a_six_tick_square_wave_that_terminates() {
        let mut h = HealthState::new(20);
        h.update(10, 0);
        let on: Vec<i64> = (0..25).filter(|t| h.is_blinking(*t)).collect();
        // Measured from the END, so the first LIT tick is 3 rather than 0 —
        // and the last is 17, three short of the 20 the blink is armed to.
        assert_eq!(on, vec![3, 4, 5, 9, 10, 11, 15, 16, 17]);
        assert!(
            !h.is_blinking(20),
            "strictly greater: it is over at the end tick"
        );
    }

    /// **A second change SHORTENS the blink rather than extending it**, because
    /// the field is assigned rather than maxed.
    #[test]
    fn a_second_change_shortens_the_blink() {
        let mut h = HealthState::new(20);
        h.update(10, 0); // decrease: blink until 20
        h.update(11, 1); // increase one tick later: blink until 11, not 20
        assert!(h.is_blinking(8), "still blinking inside the shorter window");
        assert!(!h.is_blinking(9), "and 9 is a DARK phase of that window, not its end");
        assert!(
            !h.is_blinking(12),
            "and over well before the decrease's 20"
        );
    }

    /// The displayed value catches up **only after** the delay, and the test
    /// is STRICTLY greater — at exactly 20 it has not caught up.
    #[test]
    fn the_displayed_value_catches_up_only_after_the_last_change() {
        let mut h = HealthState::new(20);
        h.update(10, 0);
        assert_eq!(h.displayed_value(), 20, "still showing the old value");
        h.update(10, 20);
        assert_eq!(h.displayed_value(), 20, "20 is NOT yet past the delay");
        h.update(10, 21);
        assert_eq!(h.displayed_value(), 10, "and 21 is");
    }

    /// **A flickering value keeps the display stale indefinitely**, because a
    /// change restarts the catch-up clock and the two halves of `update` are
    /// independent rather than an if/else.
    #[test]
    fn a_flickering_value_never_catches_up() {
        let mut h = HealthState::new(20);
        for t in 0..200 {
            h.update(if t % 2 == 0 { 10 } else { 11 }, t);
        }
        assert_eq!(
            h.displayed_value(),
            20,
            "every tick changed the value, so the delay never elapsed"
        );
    }

    /// **The ghost layer is the OLD value, not the new one** — which is what
    /// makes a drop show where the health used to be.
    #[test]
    fn the_ghost_layer_is_drawn_from_the_displayed_value() {
        let mut h = HealthState::new(20);
        h.update(10, 0);
        // Tick 3 is the FIRST lit tick (see the square-wave test) — tick 1 is
        // dark, because the phase is measured from the end of the blink.
        assert!(h.is_blinking(3));
        let blits = heart_blits(10, h, 3, 0, HEARTS_WIDTH);
        let ghosts: Vec<i32> = blits
            .iter()
            .filter(|b| b.sprite == HeartSprite::FullBlinking)
            .map(|b| b.dx / 8)
            .collect();
        assert_eq!(
            ghosts,
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            "the ghost spans the OLD 20 health, past the live 10"
        );
        let live: Vec<i32> = blits
            .iter()
            .filter(|b| b.sprite == HeartSprite::Full)
            .map(|b| b.dx / 8)
            .collect();
        assert_eq!(live, vec![0, 1, 2, 3, 4], "and the live fill is only five");
    }

    /// A filled heart is TWO layers: its own container, then the fill.
    #[test]
    fn a_filled_heart_sits_on_its_own_container() {
        let h = HealthState::new(20);
        let blits = heart_blits(20, h, 0, 0, HEARTS_WIDTH);
        let at_zero: Vec<HeartSprite> =
            blits.iter().filter(|b| b.dx == 0).map(|b| b.sprite).collect();
        assert_eq!(
            at_zero,
            vec![HeartSprite::Container, HeartSprite::Full],
            "container first, then the fill over it — and in that order"
        );
    }

    /// **The eleventh heart is gold, and its sprite is a `*_blinking` asset
    /// that is not the blink layer.** Vanilla ships no non-blinking absorbing
    /// sprite.
    #[test]
    fn the_eleventh_heart_is_absorption_and_is_not_blinking() {
        let h = HealthState::new(22);
        let blits = heart_blits(22, h, 0, 0, HEARTS_WIDTH);
        assert!(!h.is_blinking(0), "nothing is blinking in this fixture");
        // The pitch here is SEVEN, not eight: 22 health renders 11 hearts, and
        // `floor(86 / 11)` is 7. Assuming the 20-health pitch of 8 looks in
        // the wrong place and finds nothing.
        let per = width_per_heart(0, HEARTS_WIDTH, 11);
        assert_eq!(per, 7, "11 hearts pack tighter than 10");
        let tenth: Vec<HeartSprite> = blits
            .iter()
            .filter(|b| b.dx == 10 * per)
            .map(|b| b.sprite)
            .collect();
        assert!(
            tenth.contains(&HeartSprite::AbsorbingFull),
            "heart index 10 is absorption: {tenth:?}"
        );
        let ninth: Vec<HeartSprite> = blits
            .iter()
            .filter(|b| b.dx == 9 * per)
            .map(|b| b.sprite)
            .collect();
        assert!(ninth.contains(&HeartSprite::Full), "and index 9 is ordinary");
    }

    /// **The two halves of `update` provably cannot both fire on one tick**,
    /// because the first sets `last_update_tick = tick` and the second then
    /// reads `0 > 20`.
    ///
    /// This pins the coincidence rather than the code: it is why an `if`/`else`
    /// is equivalent to vanilla's two statements, and why a mutation adding an
    /// early return survives the battery.
    #[test]
    fn a_change_never_catches_up_on_the_same_tick() {
        for delay in [0i64, 1, 20, 21, 1000] {
            let mut h = HealthState::new(20);
            // Let the clock run well past the delay first, so the ONLY reason
            // the catch-up does not fire is the reset in the change branch.
            h.update(20, delay);
            h.update(10, delay);
            assert_eq!(
                h.displayed_value(),
                20,
                "a tick that changed the value must not also catch up (delay {delay})"
            );
        }
    }

    /// A fresh state is not blinking and has nothing to catch up to, whatever
    /// the health — which is what stops every player flashing on join.
    #[test]
    fn a_newly_seen_player_does_not_blink() {
        for v in [0, 1, 20, 40] {
            let h = HealthState::new(v);
            assert!(!h.is_blinking(0));
            assert_eq!(h.displayed_value(), v);
        }
    }
}
