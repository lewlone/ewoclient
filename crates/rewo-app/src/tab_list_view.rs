//! The tab list, resolved from session state and emitted as draws (M151).
//!
//! `rewo_gpu::tab_list` has been model + layout since M52f — 1209 lines, 41
//! tests and, until this module, **zero consumers**. Pressing Tab in
//! `rewo live` showed nothing. This is the join: it turns the session's
//! player state into the `TabEntry` list that module sorts, measures the names
//! it will not measure itself, and turns the placed layout into fills, text and
//! sprite blits.
//!
//! ## Where it lives, and why not in `rewo-net`
//!
//! The sidebar's model and layout are both in `rewo_net::sidebar`, so its
//! `live_cmd::resolve_sidebar` is a five-line adapter. The tab list cannot copy
//! that shape: `TabEntry` and the layout are in **`rewo-gpu`**, the session is
//! in **`rewo-net`**, and neither crate depends on the other — they meet only
//! here. So the join is a `rewo-app` module, split M97's way: [`resolve`] takes
//! plain values and closures and is unit-testable, while
//! [`crate::live_cmd`]'s adapter does the session lookups. `PlaySession` owns a
//! socket and has no test module anywhere in the repo (M71), so anything
//! decided inside it would be untestable by construction.
//!
//! ## Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/client/gui/components/PlayerTabOverlay.java` —
//!   `getPlayerInfos`, `getNameForDisplay`, `decorateName`,
//!   `extractRenderState`, `extractPingIcon`, `extractTablistScore`
//! - `net/minecraft/client/gui/Hud.java:426-438` — `extractTabList`, the two
//!   gates on the whole thing
//! - `net/minecraft/client/Options.java:671` — `keyPlayerList`, GLFW 258
//! - `net/minecraft/client/gui/Font.java:336` — `getTextColor`, which is why
//!   the spectator colour is an *alpha* here and not an RGB
//! - `net/minecraft/network/chat/numbers/StyledFormat.java:30` —
//!   `PLAYER_LIST_DEFAULT`
//!
//! ## Four things that read backwards
//!
//! 1. **The sort key and the drawn name are different strings.**
//!    `PLAYER_COMPARATOR`'s last key is `p.getProfile().name()` — the profile
//!    name — while `maxNameWidth` and every row measure
//!    `getNameForDisplay(info)`, which prefers the server's
//!    `tabListDisplayName` override. So a server that renames everyone to
//!    `[VIP] x` still sorts them alphabetically by their real names.
//! 2. **The spectator colour is white at alpha 144, and the alpha survives a
//!    coloured span.** `-1862270977` is `0x90FFFFFF` (M136 corrected M52f's
//!    grey). `Font.getTextColor` keeps a styled span's own RGB and takes *this*
//!    argument's alpha, so a spectator with a coloured display name is faded,
//!    not recoloured.
//! 3. **A row's background is drawn per row, and the three bands are not.**
//!    `PlayerTabOverlay` fills once per slot where `displayScoreboardSidebar`
//!    one class over fills one rect for every row — M132's `p13` pins the
//!    sidebar's shape and this is the other one.
//! 4. **`onlineMode` is a width input.** With no face the slot is nine pixels
//!    narrower, so it moves every name on the screen and not merely the icons.
//!
//! ## What this module does NOT draw, stated rather than hidden
//!
//! - **The 8×8 player faces.** `showHead` is honoured — the layout reserves
//!   their nine pixels on an online-mode server, which is vanilla's geometry —
//!   but nothing fills the rect. Rewo has no GUI-side path that can sample a
//!   64×64 skin at all: the skin pool lives in the *entity* atlas
//!   (`rewo_gpu::entities::upload_skin`), the HUD atlas is built once in
//!   `HudPass::new` with no runtime upload entry point, and the fetched RGBA
//!   is dropped after `SkinLoader::poll_uploads`. That is a dynamic-texture
//!   pool, not a sprite, and it is its own piece of work.
//! - **`RenderType::HEARTS`.** The 90-pixel column is *reserved*, because that
//!   is `widthForScore` and getting it wrong moves every name; the hearts
//!   themselves are not drawn. `extractTablistHearts` needs eight more sprites
//!   and a per-uuid `HealthState` blink clock
//!   (`PlayerTabOverlay.HealthState`, 20/10-tick durations and a `% 6 >= 3`
//!   phase) that nothing here keeps.
//! - **`Minecraft.isLocalServer()`'s half of the visibility gate.** Rewo has no
//!   integrated server, so the clause that hides a one-player singleplayer list
//!   can never fire — see [`visible`].

use rewo_gpu::tab_list::{self, EntrySlot, PingIcon, ScoreColumn, TabEntry, TabListInput};
use rewo_net::scoreboard::{DisplaySlot, RenderType, Scoreboard};
use rewo_proto::nbt::Nbt;
use rewo_world::chat_style::{parse_component, ChatLine, ChatStyle};

/// `graphics.text(..., -1)` — the default colour every row, header and footer
/// line is drawn with. Opaque white.
pub const NAME_ALPHA: f32 = 1.0;

/// `-1862270977` = `0x90FFFFFF`: alpha **144**, RGB white.
///
/// Carried as an alpha rather than a colour because that is what it does. A
/// span with its own colour keeps that colour and takes this alpha
/// (`Font.getTextColor`), so folding it into the base RGB would recolour a
/// server's coloured display name instead of fading it.
pub const SPECTATOR_ALPHA: f32 = 144.0 / 255.0;

/// The base style every line resolves against: white, no flags.
///
/// `-1`'s RGB is `0xFFFFFF` and `-1862270977`'s is too, so one base serves
/// both rows — only the alpha differs, and `decorateName`'s italic is applied
/// per entry.
const BASE: ChatStyle = ChatStyle::plain([1.0, 1.0, 1.0]);

/// One placed, resolved row: the layout's geometry plus what goes in it.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedRow {
    /// `getNameForDisplay(info)`, already decorated and styled.
    pub name: ChatLine,
    /// `getGameMode() == SPECTATOR`, which decides the alpha *and* whether a
    /// score is drawn at all.
    pub spectator: bool,
    pub ping: PingIcon,
    /// `entry.formattedScore` — empty when there is no display objective, when
    /// the objective renders as HEARTS, or when this holder has no score.
    pub score: ChatLine,
    /// `entry.scoreWidth`, which the draw right-aligns against.
    pub score_width: i32,
    /// `ScoreDisplayEntry.score` — the RAW integer (M155).
    ///
    /// Carried separately from [`Self::score`] because vanilla's record holds
    /// **both** in both modes: the formatted component is `null` under HEARTS,
    /// and the integer is what `extractTablistHearts` reads. `None` means this
    /// holder has no score for the objective, which is different from a score
    /// of zero — zero is a dead player and draws nothing, while absence means
    /// the column does not apply to this row at all.
    pub score_value: Option<i32>,
}

/// The whole tab list for one frame, resolved but not yet placed.
///
/// Held separately from `TabListLayout` because the layout is a pure function
/// of integers (that is `rewo_gpu::tab_list`'s point) and this is everything
/// the integers cannot carry.
#[derive(Clone, Debug, PartialEq)]
pub struct TabListView {
    /// Sorted and truncated to 80 by `visible_entries`.
    pub entries: Vec<TabEntry>,
    /// Parallel to `entries`.
    pub rows: Vec<ResolvedRow>,
    /// Feeds `rewo_gpu::tab_list::layout`.
    pub input: TabListInput,
    /// `font.split(header, screenWidth - 50)`, already wrapped.
    pub header: Vec<ChatLine>,
    pub footer: Vec<ChatLine>,
}

/// `Hud.extractTabList`'s gate, minus the clause Rewo cannot reach.
///
/// ```java
/// if (!options.keyPlayerList.isDown()
///     || this.minecraft.isLocalServer()
///        && connection.getListedOnlinePlayers().size() <= 1
///        && displayObjective == null) { setVisible(false); }
/// ```
///
/// The second disjunct is `isLocalServer() && …`, and Rewo has no integrated
/// server, so it is always false here — a one-player list on a *dedicated*
/// server does show, which is the case this client always has. Modelling the
/// whole conjunction and passing a constant `false` would look more faithful
/// and would be a branch no input can enter.
///
/// The F1 gate is separate and sits one level up: `extractTabList` is called
/// from inside `Hud.extractRenderState`'s `if (!this.isHidden)` block
/// (`Hud.java:237`), the same gate M132's sidebar reads.
pub fn visible(key_down: bool, hud_hidden: bool) -> bool {
    key_down && !hud_hidden
}

/// Everything [`resolve`] needs that it will not look up itself.
///
/// Closures rather than a session reference, for M97's reason: the lookups
/// live in `PlaySession`, which no test can build.
pub struct TabListLookups<'a> {
    /// `getListedOnlinePlayers()` — `PlaySession::listed_players()`.
    pub listed: &'a [u128],
    /// `profile.name()`. A uuid with no profile name yields **no row**: a name
    /// is the sort key *and* the fallback display text, so there is nothing to
    /// place. Vanilla cannot reach this state (`newEntries` requires a
    /// profile), and answering with the uuid would put a hex string on the
    /// list where a real server never does.
    pub name_of: &'a dyn Fn(u128) -> Option<String>,
    /// `getLatency()`.
    pub ping_of: &'a dyn Fn(u128) -> Option<i32>,
    /// `getGameMode() == SPECTATOR`.
    pub spectator_of: &'a dyn Fn(u128) -> bool,
    /// `getTabListOrder()`, defaulting to vanilla's 0.
    pub order_of: &'a dyn Fn(u128) -> i32,
    /// `getTeam()`, by team **name** — the third sort key.
    pub team_of: &'a dyn Fn(u128) -> Option<String>,
    /// `getTabListDisplayName()`, raw.
    pub display_name_of: &'a dyn Fn(u128) -> Option<Nbt>,
    /// `StringSplitter`'s width provider — style-aware because
    /// `getBoldOffset()` charges one pixel per character (M126).
    pub width_of: &'a dyn Fn(&str, ChatStyle) -> i32,
    pub lang: Option<&'a rewo_data::lang::Language>,
    /// `connection.onlineMode()`.
    pub online_mode: bool,
    /// `graphics.guiWidth()`.
    pub screen_width: i32,
    /// The tab list's header and footer components, as the client holds them.
    pub header: Option<&'a Nbt>,
    pub footer: Option<&'a Nbt>,
    /// `scoreboard.getDisplayObjective(DisplaySlot.LIST)` and the scoreboard
    /// it came from. `None` for the whole pair when no objective is displayed
    /// there, which is the common case.
    pub scoreboard: Option<&'a Scoreboard>,
}

fn line_width(line: &ChatLine, width_of: &dyn Fn(&str, ChatStyle) -> i32) -> i32 {
    line.iter()
        .map(|s| {
            width_of(
                &s.text,
                ChatStyle {
                    color: s.color,
                    bold: s.bold,
                    italic: s.italic,
                    underlined: s.underlined,
                    strikethrough: s.strikethrough,
                    obfuscated: s.obfuscated,
                    events: s.events.clone(),
                },
            )
        })
        .sum()
}

/// `font.split(component, screenWidth - 50)`, or no lines at all when the
/// client holds no component.
fn split_block(
    component: Option<&Nbt>,
    screen_width: i32,
    width_of: &dyn Fn(&str, ChatStyle) -> i32,
    lang: Option<&rewo_data::lang::Language>,
) -> Vec<ChatLine> {
    let Some(c) = component else {
        return Vec::new();
    };
    let spans = parse_component(c, BASE, lang);
    rewo_world::string_splitter::split_lines_wrapped(
        &spans,
        screen_width - tab_list::SCREEN_MARGIN,
        width_of,
    )
    .into_iter()
    .map(|l| l.spans)
    .collect()
}

/// `getNameForDisplay(info)` then `decorateName(info, …)`.
///
/// ```java
/// info.getTabListDisplayName() != null
///    ? decorateName(info, info.getTabListDisplayName().copy())
///    : decorateName(info, PlayerTeam.formatNameForTeam(info.getTeam(), literal(profile.name())))
/// ```
///
/// **The team formatting applies only to the fallback.** A server that sets a
/// display name has already decided what the row says, and vanilla does not
/// wrap that in the team's prefix and suffix as well — so an implementation
/// that formatted unconditionally would double a team prefix on every renamed
/// player.
///
/// `decorateName` is `withStyle(ITALIC)` for a spectator, which sets italic on
/// the **root**; a child span that sets `italic: false` still wins, which is
/// exactly what passing it as the base style here reproduces.
fn name_for_display(
    display: Option<&Nbt>,
    profile_name: &str,
    team: Option<&rewo_net::teams::Team>,
    spectator: bool,
    lang: Option<&rewo_data::lang::Language>,
) -> ChatLine {
    let base = ChatStyle { italic: spectator, ..BASE };
    match display {
        Some(c) => parse_component(c, base, lang),
        None => rewo_net::sidebar::format_name_for_team(
            team,
            &Nbt::String(profile_name.to_string()),
            base,
            lang,
        ),
    }
}

/// Resolve one frame's tab list.
///
/// Pure: everything it reads arrives through [`TabListLookups`]. Returns a view
/// even when there are no listed players — vanilla draws the bands and the
/// header regardless, and `column_solve(0)` is `(1, 0)`, a real case the layout
/// transcribes the loop rather than the closed form to keep.
pub fn resolve(l: &TabListLookups<'_>) -> TabListView {
    // `getPlayerInfos()`: the listed players, sorted, then the first 80. The
    // TabEntry carries the PROFILE name, because that is the comparator's last
    // key — the display override is a render concern and is resolved below.
    let mut entries: Vec<TabEntry> = Vec::with_capacity(l.listed.len());
    for &uuid in l.listed {
        let Some(name) = (l.name_of)(uuid) else {
            continue;
        };
        entries.push(TabEntry {
            uuid,
            name,
            ping: (l.ping_of)(uuid),
            local: false,
            tab_list_order: (l.order_of)(uuid),
            spectator: (l.spectator_of)(uuid),
            team: (l.team_of)(uuid),
        });
    }
    let entries = tab_list::visible_entries(&entries);

    // The display objective and its render type, resolved once. `widthForScore`
    // is decided by the type and not by the widest score: HEARTS is a flat 90.
    let objective = l
        .scoreboard
        .and_then(|sb| sb.display_objective(DisplaySlot::List));
    let hearts = objective.is_some_and(|o| o.render_type == RenderType::Hearts);

    let spacer_width = (l.width_of)(" ", BASE);
    let mut max_name_width = 0i32;
    let mut max_score_width = 0i32;
    let mut rows: Vec<ResolvedRow> = Vec::with_capacity(entries.len());
    for e in &entries {
        let team = l
            .scoreboard
            .and_then(|sb| sb.teams.team_of_member(&e.name))
            .and_then(|t| l.scoreboard.and_then(|sb| sb.teams.team(t)));
        let display = (l.display_name_of)(e.uuid);
        let name = name_for_display(display.as_ref(), &e.name, team, e.spectator, l.lang);
        max_name_width = max_name_width.max(line_width(&name, l.width_of));

        // `ScoreHolder.fromGameProfile(profile)` is the profile NAME, which is
        // also the key `set_score` files a player's score under.
        let mut score: ChatLine = Vec::new();
        let mut score_width = 0i32;
        // M155 — the raw integer, read whether or not the objective is HEARTS.
        // The formatted half below stays gated on `!hearts`, because vanilla
        // leaves `formattedScore` null in that mode; this one it always fills.
        let score_value = match (objective, l.scoreboard) {
            (Some(obj), Some(sb)) => sb.score(&e.name, &obj.name).map(|s| s.value),
            _ => None,
        };
        if let (Some(obj), Some(sb), false) = (objective, l.scoreboard, hearts) {
            if let Some(s) = sb.score(&e.name, &obj.name) {
                let format = s.number_format.as_ref().or(obj.number_format.as_ref());
                score = rewo_net::sidebar::format_value_with_default(
                    format,
                    s.value,
                    rewo_net::sidebar::PLAYER_LIST_DEFAULT_RGB,
                    BASE,
                    l.lang,
                );
                score_width = line_width(&score, l.width_of);
            }
            // `maxScoreWidth = max(maxScoreWidth, playerScoreWidth > 0 ?
            // spacerWidth + playerScoreWidth : 0)` — the space is charged only
            // when there is a score to separate, so a BlankFormat objective
            // reserves no column at all.
            max_score_width =
                max_score_width.max(if score_width > 0 { spacer_width + score_width } else { 0 });
        }

        rows.push(ResolvedRow {
            name,
            spectator: e.spectator,
            ping: e.ping_icon(),
            score,
            score_width,
            score_value,
        });
    }

    let score = match objective {
        None => ScoreColumn::None,
        Some(_) if hearts => ScoreColumn::Hearts,
        Some(_) => ScoreColumn::Numeric { max_score_width },
    };

    let header = split_block(l.header, l.screen_width, l.width_of, l.lang);
    let footer = split_block(l.footer, l.screen_width, l.width_of, l.lang);
    let input = TabListInput {
        screen_width: l.screen_width,
        show_head: l.online_mode,
        max_name_width,
        score,
        header_lines: header.iter().map(|h| line_width(h, l.width_of)).collect(),
        footer_lines: footer.iter().map(|f| line_width(f, l.width_of)).collect(),
    };

    TabListView { entries, rows, input, header, footer }
}

/// The tab list's fills, in **GUI pixels**.
///
/// Three bands plus one per row — `PlayerTabOverlay` fills a rect per slot
/// where the sidebar fills one for all its rows, which is the shape difference
/// M132's `p13` pins from the other side.
///
/// **No `px` multiply**, deliberately: [`rewo_gpu::hud::HudFill`] is documented
/// as GUI pixels and `HudPass::draw` applies the GUI scale itself. M135 found
/// four producers that multiplied first and whose fills therefore landed off
/// the bottom of the screen — *absent* rather than misplaced, which is why it
/// survived eight milestones. `rewo_gpu::hud::gui_scale` is the one place that
/// scale is computed and nothing here recomputes it.
pub fn fills(layout: &tab_list::TabListLayout) -> Vec<rewo_gpu::hud::HudFill> {
    let fill = |r: tab_list::Rect, argb: u32| rewo_gpu::hud::HudFill {
        x: r.x as f32,
        y: r.y as f32,
        w: r.w as f32,
        h: r.h as f32,
        alpha: ((argb >> 24) & 0xFF) as f32 / 255.0,
        rgb: crate::live_cmd::srgb_bytes_to_linear(argb & 0x00FF_FFFF),
    };
    let mut out = Vec::with_capacity(layout.entries.len() + 3);
    // The header band first, then the list band, then the rows — vanilla's
    // order, and the rows have to come after the band they sit on.
    if let Some(b) = layout.header_band {
        out.push(fill(b, tab_list::BAND_COLOR));
    }
    out.push(fill(layout.list_band, tab_list::BAND_COLOR));
    for e in &layout.entries {
        out.push(fill(e.background, tab_list::DEFAULT_ROW_BACKGROUND));
    }
    if let Some(b) = layout.footer_band {
        out.push(fill(b, tab_list::BAND_COLOR));
    }
    out
}

/// The ping icons, one per placed row.
///
/// A separate list from the fills because a sprite names an atlas rect rather
/// than a colour; `HudPass::draw` emits them after every fill, which is the
/// order they need — a row's icon sits *inside* that row's background.
pub fn icons(view: &TabListView, layout: &tab_list::TabListLayout) -> Vec<rewo_gpu::hud::HudBlit> {
    layout
        .entries
        .iter()
        .filter_map(|slot| {
            let row = view.rows.get(slot.index)?;
            Some(rewo_gpu::hud::HudBlit {
                x: slot.ping_icon.x as f32,
                y: slot.ping_icon.y as f32,
                w: slot.ping_icon.w as f32,
                h: slot.ping_icon.h as f32,
                icon: rewo_gpu::hud::HudIcon::Ping(ping_slot(row.ping)),
            })
        })
        .collect()
}

/// `AvatarRenderer.isPlayerUpsideDown` — `"Dinnerbone".equals(name) ||
/// "Grumm".equals(name)` (M155).
///
/// **Exact string equality, so it is case sensitive**: "dinnerbone" is not
/// flipped.
pub fn is_upside_down_name(name: &str) -> bool {
    name == "Dinnerbone" || name == "Grumm"
}

/// The tab list's 8x8 player faces (M155).
///
/// Separate from [`icons`] for the same reason [`hearts`] is: r47 counts
/// `icons(..).len()`, and this list is per-row-with-a-skin rather than
/// per-row.
///
/// `face_of` answers a player's atlas slot, or `None` while their skin is
/// still in flight — which is the ordinary state for the first frames after a
/// join and draws no face rather than someone else's.
///
/// **The flip needs the player to be LOADED, not merely listed.** Vanilla's
/// `level.getPlayerByUUID(id)` returns null for a player outside render
/// distance, and `flip` is `player != null && isPlayerUpsideDown(player)` — so
/// a Dinnerbone across the map is on the tab list the right way up. Reading
/// the name alone flips them everywhere, which looks more "correct" and is
/// not what vanilla does.
pub fn faces(
    view: &TabListView,
    layout: &tab_list::TabListLayout,
    face_of: &dyn Fn(u128) -> Option<u8>,
    show_hat_of: &dyn Fn(u128) -> bool,
    loaded_of: &dyn Fn(u128) -> bool,
) -> Vec<rewo_gpu::hud::HudBlit> {
    let mut out = Vec::new();
    for slot in &layout.entries {
        let Some(rect) = slot.face else { continue };
        let Some(e) = view.entries.get(slot.index) else {
            continue;
        };
        let Some(face) = face_of(e.uuid) else { continue };
        let flip = loaded_of(e.uuid) && is_upside_down_name(&e.name);
        let blit = |hat: bool| rewo_gpu::hud::HudBlit {
            x: rect.x as f32,
            y: rect.y as f32,
            w: rect.w as f32,
            h: rect.h as f32,
            icon: rewo_gpu::hud::HudIcon::Face { slot: face, hat, flip },
        };
        out.push(blit(false));
        // `if (hat) extractHat(..)` — a second blit over the first, never a
        // different sprite.
        if show_hat_of(e.uuid) {
            out.push(blit(true));
        }
    }
    out
}

/// The health column's heart sprites, when the objective renders as HEARTS
/// (M155).
///
/// Separate from [`icons`] rather than folded into it for two reasons. It needs
/// per-player mutable state — the blink clock — which `icons` does not have and
/// should not grow; and `live --render-check`'s r47 counts `icons(..).len()`,
/// so putting hearts there would silently move a number a gate asserts.
///
/// `states` is keyed by profile id and **mutated**: vanilla's
/// `healthStates.computeIfAbsent(profileId, …)` then `health.update(score,
/// tick)` runs once per rendered row per frame, which is what advances the
/// blink. A caller that rebuilt the map each frame would never blink at all,
/// because a fresh `HealthState` is seeded with its own value and has nothing
/// to catch up to.
pub fn hearts(
    view: &TabListView,
    layout: &tab_list::TabListLayout,
    states: &mut std::collections::HashMap<u128, tab_list::HealthState>,
    gui_tick: i64,
) -> Vec<rewo_gpu::hud::HudBlit> {
    let mut out = Vec::new();
    for slot in &layout.entries {
        let Some(row) = view.rows.get(slot.index) else {
            continue;
        };
        // `score_span` is already `None` for a spectator, which is vanilla's
        // rule that a spectator's score is not drawn at all.
        let (Some((left, right)), Some(score)) = (slot.score_span, row.score_value) else {
            continue;
        };
        // `entries` is parallel to `rows` and carries the profile id, which
        // is the key vanilla files its `healthStates` map under.
        let Some(uuid) = view.entries.get(slot.index).map(|e| e.uuid) else {
            continue;
        };
        let health = states
            .entry(uuid)
            .or_insert_with(|| tab_list::HealthState::new(score));
        health.update(score, gui_tick);
        for b in tab_list::heart_blits(score, *health, gui_tick, left, right) {
            out.push(rewo_gpu::hud::HudBlit {
                x: (left + b.dx) as f32,
                y: slot.name.1 as f32,
                w: tab_list::HEART_SPRITE_SIZE as f32,
                h: tab_list::HEART_SPRITE_SIZE as f32,
                icon: rewo_gpu::hud::HudIcon::Heart(b.sprite),
            });
        }
    }
    out
}

/// [`PingIcon`] into the index `rewo_data::assets::PING_SPRITES` uses.
///
/// The enum's own order and the sprite array's are the same by construction,
/// but they are written in two crates that do not depend on each other, so the
/// mapping is spelled out rather than cast from a discriminant.
pub fn ping_slot(icon: PingIcon) -> u8 {
    match icon {
        PingIcon::Unknown => 0,
        PingIcon::Ping1 => 1,
        PingIcon::Ping2 => 2,
        PingIcon::Ping3 => 3,
        PingIcon::Ping4 => 4,
        PingIcon::Ping5 => 5,
    }
}

/// The tab list's text: the header lines, every row's name and score, then the
/// footer lines.
///
/// `px` is the GUI scale, because [`rewo_gpu::world::OwnedTextLine`] is in
/// **screen** pixels while the layout is in GUI pixels — the other half of the
/// convention split that produced M135's bug, and the reason the two emitters
/// beside each other take different units.
///
/// Every call drops a shadow: `PlayerTabOverlay` uses the five-argument
/// `graphics.text`, which delegates with `dropShadow = true` (M105), where
/// `displayScoreboardSidebar` passes an explicit `false` on all three of its
/// calls. The asymmetry is vanilla's.
pub fn text(
    view: &TabListView,
    layout: &tab_list::TabListLayout,
    px: f32,
    width_of: &dyn Fn(&str, ChatStyle) -> i32,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    let mut out: Vec<rewo_gpu::world::OwnedTextLine> = Vec::new();
    let mut push = |line: &ChatLine, x: i32, y: i32, alpha: f32| {
        let mut pen = x as f32 * px;
        for span in line {
            let style = ChatStyle {
                color: span.color,
                bold: span.bold,
                italic: span.italic,
                underlined: span.underlined,
                strikethrough: span.strikethrough,
                obfuscated: span.obfuscated,
                events: span.events.clone(),
            };
            let w = width_of(&span.text, style);
            if !span.text.is_empty() {
                out.push(rewo_gpu::world::OwnedTextLine {
                    x: pen,
                    y: y as f32 * px,
                    px,
                    color_linear: crate::live_cmd::srgb_bytes_to_linear_f(span.color),
                    alpha,
                    shadow: true,
                    style: rewo_gpu::text::TextStyle {
                        bold: span.bold,
                        italic: span.italic,
                        underlined: span.underlined,
                        strikethrough: span.strikethrough,
                        obfuscated: span.obfuscated,
                    },
                    text: span.text.clone(),
                });
            }
            pen += w as f32 * px;
        }
    };

    for (line, origin) in view.header.iter().zip(&layout.header_line_origins) {
        push(line, origin.0, origin.1, NAME_ALPHA);
    }
    for slot in &layout.entries {
        let Some(row) = view.rows.get(slot.index) else {
            continue;
        };
        let alpha = if row.spectator { SPECTATOR_ALPHA } else { NAME_ALPHA };
        push(&row.name, slot.name.0, slot.name.1, alpha);
        // `graphics.text(font, entry.formattedScore, right - entry.scoreWidth,
        // yo, -1)` — right-aligned to the span's RIGHT edge, and drawn at the
        // full default alpha even on a row whose name is faded. The span never
        // exists for a spectator anyway: `extractTablistScore` is reached only
        // inside `info.getGameMode() != SPECTATOR`, which is also what
        // `EntrySlot::score_span` gates on.
        if let Some((_, right)) = slot.score_span {
            if !row.score.is_empty() {
                push(&row.score, right - row.score_width, slot.name.1, NAME_ALPHA);
            }
        }
    }
    for (line, origin) in view.footer.iter().zip(&layout.footer_line_origins) {
        push(line, origin.0, origin.1, NAME_ALPHA);
    }
    out
}

/// The face rects the layout reserved and nothing fills — see the module doc.
///
/// Exposed so a gate can assert the reservation *happened* on an online-mode
/// server rather than inferring it from a slot width, and so the gap is a named
/// thing in the code rather than an absence.
pub fn face_rects(layout: &tab_list::TabListLayout) -> Vec<(usize, tab_list::Rect)> {
    layout
        .entries
        .iter()
        .filter_map(|e: &EntrySlot| e.face.map(|r| (e.index, r)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A width provider that charges six pixels a character and one more per
    /// character when bold, which is `getBoldOffset()`'s rule (M126).
    fn w(s: &str, style: ChatStyle) -> i32 {
        s.chars().count() as i32 * if style.bold { 7 } else { 6 }
    }

    struct Fixture {
        listed: Vec<u128>,
        names: Vec<(u128, &'static str)>,
        spectators: Vec<u128>,
        orders: Vec<(u128, i32)>,
        display: Vec<(u128, Nbt)>,
        pings: Vec<(u128, i32)>,
        online_mode: bool,
        header: Option<Nbt>,
        footer: Option<Nbt>,
        scoreboard: Option<Scoreboard>,
        screen_width: i32,
    }

    impl Default for Fixture {
        fn default() -> Self {
            Fixture {
                listed: Vec::new(),
                names: Vec::new(),
                spectators: Vec::new(),
                orders: Vec::new(),
                display: Vec::new(),
                pings: Vec::new(),
                online_mode: false,
                header: None,
                footer: None,
                scoreboard: None,
                screen_width: 320,
            }
        }
    }

    impl Fixture {
        fn with(names: &[(u128, &'static str)]) -> Fixture {
            Fixture {
                listed: names.iter().map(|(u, _)| *u).collect(),
                names: names.to_vec(),
                ..Fixture::default()
            }
        }
        fn resolve(&self) -> TabListView {
            let name_of = |u: u128| {
                self.names
                    .iter()
                    .find(|(k, _)| *k == u)
                    .map(|(_, n)| n.to_string())
            };
            let ping_of = |u: u128| self.pings.iter().find(|(k, _)| *k == u).map(|(_, p)| *p);
            let spectator_of = |u: u128| self.spectators.contains(&u);
            let order_of = |u: u128| {
                self.orders
                    .iter()
                    .find(|(k, _)| *k == u)
                    .map_or(0, |(_, o)| *o)
            };
            let team_of = |u: u128| {
                let n = name_of(u)?;
                self.scoreboard
                    .as_ref()?
                    .teams
                    .team_of_member(&n)
                    .map(str::to_string)
            };
            let display_name_of = |u: u128| {
                self.display
                    .iter()
                    .find(|(k, _)| *k == u)
                    .map(|(_, c)| c.clone())
            };
            super::resolve(&TabListLookups {
                listed: &self.listed,
                name_of: &name_of,
                ping_of: &ping_of,
                spectator_of: &spectator_of,
                order_of: &order_of,
                team_of: &team_of,
                display_name_of: &display_name_of,
                width_of: &w,
                lang: None,
                online_mode: self.online_mode,
                screen_width: self.screen_width,
                header: self.header.as_ref(),
                footer: self.footer.as_ref(),
                scoreboard: self.scoreboard.as_ref(),
            })
        }
    }

    fn plain(line: &ChatLine) -> String {
        rewo_world::chat_style::plain_text(line)
    }

    /// The gate, both halves.
    ///
    /// `isLocalServer()`'s clause cannot fire here — see [`visible`]'s doc — so
    /// the only two inputs are the key and F1.
    #[test]
    fn the_list_needs_the_key_down_and_the_hud_shown() {
        assert!(visible(true, false));
        assert!(!visible(false, false));
        assert!(!visible(true, true));
        assert!(!visible(false, true));
    }

    /// The sort key is the PROFILE name; the drawn text is the display
    /// override.
    ///
    /// The fixture names them in opposite orders on purpose: sorting on the
    /// display name puts `zzz`'s row first, and the two readings are
    /// indistinguishable whenever a server sets no overrides — which is most
    /// of them.
    #[test]
    fn the_sort_reads_the_profile_name_and_the_row_draws_the_override() {
        let mut f = Fixture::with(&[(1, "alpha"), (2, "beta")]);
        f.display = vec![
            (1, Nbt::String("zzz".into())),
            (2, Nbt::String("aaa".into())),
        ];
        let v = f.resolve();
        assert_eq!(
            v.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
        assert_eq!(plain(&v.rows[0].name), "zzz");
        assert_eq!(plain(&v.rows[1].name), "aaa");
        // …and the width is measured off the DRAWN text, not the sort key.
        // Both are three characters here, so widen one.
        let mut f2 = f;
        f2.display[0] = (1, Nbt::String("zzzzzzzzzz".into()));
        assert_eq!(f2.resolve().input.max_name_width, 60);
    }

    /// A display name is NOT team-formatted; the profile-name fallback is.
    ///
    /// Vanilla's ternary puts `formatNameForTeam` only on the else branch, so
    /// formatting unconditionally doubles a prefix on every renamed player.
    #[test]
    fn only_the_fallback_name_is_team_formatted() {
        use rewo_net::teams::{CollisionRule, Team, TeamParameters, Visibility};
        let team = Team {
            name: "t".into(),
            parameters: Some(TeamParameters {
                display_name: Nbt::String("T".into()),
                player_prefix: Nbt::String("[T] ".into()),
                player_suffix: Nbt::String("!".into()),
                name_tag_visibility: Visibility::Always,
                collision_rule: CollisionRule::Always,
                color: None,
                options: 0,
            }),
            members: Default::default(),
        };
        let fallback = name_for_display(None, "Steve", Some(&team), false, None);
        assert_eq!(plain(&fallback), "[T] Steve!");
        let overridden = name_for_display(
            Some(&Nbt::String("Boss".into())),
            "Steve",
            Some(&team),
            false,
            None,
        );
        assert_eq!(plain(&overridden), "Boss");
    }

    /// `decorateName` italicises a spectator's whole name, and the fade is an
    /// ALPHA rather than a grey.
    ///
    /// The alpha is derived from vanilla's signed literal rather than restated,
    /// which is the shape M136's correction of `SPECTATOR_NAME_COLOR` needed:
    /// a witness that restated the hex agreed with whatever hex was there.
    #[test]
    fn a_spectator_is_italic_and_faded_not_greyed() {
        let spec = ((-1862270977i32) as u32) >> 24;
        assert_eq!(spec, 144);
        assert!((SPECTATOR_ALPHA - spec as f32 / 255.0).abs() < 1e-6);
        // The RGB half is full white in BOTH colours, so the difference really
        // is only the alpha.
        assert_eq!((-1862270977i32) as u32 & 0x00FF_FFFF, 0x00FF_FFFF);
        assert_eq!((-1i32) as u32 & 0x00FF_FFFF, 0x00FF_FFFF);

        let mut f = Fixture::with(&[(1, "Ghost"), (2, "Solid")]);
        f.spectators = vec![1];
        let v = f.resolve();
        // The spectator sorts LAST, whatever the names say.
        assert_eq!(v.entries[0].name, "Solid");
        assert!(v.rows[1].spectator);
        assert!(v.rows[1].name.iter().all(|s| s.italic));
        assert!(v.rows[0].name.iter().all(|s| !s.italic));
    }

    /// A uuid the server listed but never gave a profile for yields no row.
    ///
    /// It is the sort key and the fallback text at once, so there is nothing to
    /// place; answering with the uuid would put a hex string on the list.
    #[test]
    fn a_listed_uuid_with_no_profile_name_is_skipped() {
        let mut f = Fixture::with(&[(1, "Real")]);
        f.listed.push(99);
        let v = f.resolve();
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.rows.len(), 1);
    }

    /// `showHead` is a WIDTH input, not only a visibility one.
    ///
    /// The face costs nine pixels of the slot, so turning it on moves every
    /// name to the right AND widens the grid — which is why `onlineMode` had to
    /// be decoded rather than assumed.
    #[test]
    fn online_mode_widens_the_slot_by_the_faces_nine_pixels() {
        let f = Fixture::with(&[(1, "abc")]);
        let off = f.resolve();
        let mut on = f;
        on.online_mode = true;
        let on = on.resolve();
        let lo = tab_list::layout(&off.input, &off.entries);
        let li = tab_list::layout(&on.input, &on.entries);
        assert_eq!(li.slot_width - lo.slot_width, 9);
        assert!(lo.entries[0].face.is_none());
        assert!(li.entries[0].face.is_some());
        // And the reserved rects are exposed rather than silently absent.
        assert_eq!(face_rects(&lo).len(), 0);
        assert_eq!(face_rects(&li).len(), 1);
    }

    /// The fills are three bands and one per row, not one band for all rows.
    #[test]
    fn every_row_gets_its_own_background_fill() {
        let mut f = Fixture::with(&[(1, "a"), (2, "b"), (3, "c")]);
        f.header = Some(Nbt::String("Head".into()));
        f.footer = Some(Nbt::String("Foot".into()));
        let v = f.resolve();
        let l = tab_list::layout(&v.input, &v.entries);
        let fl = fills(&l);
        // header band + list band + 3 rows + footer band.
        assert_eq!(fl.len(), 6);
        // The two bands are `Integer.MIN_VALUE` — alpha 128 — and the rows are
        // `0x20FFFFFF`, alpha 32. Reading either as the other makes the whole
        // panel one flat tone.
        assert!((fl[0].alpha - 128.0 / 255.0).abs() < 1e-6);
        assert!((fl[1].alpha - 128.0 / 255.0).abs() < 1e-6);
        assert!((fl[2].alpha - 32.0 / 255.0).abs() < 1e-6);
        assert!((fl[5].alpha - 128.0 / 255.0).abs() < 1e-6);
        // The bands are black and the rows are white, which is what makes the
        // rows *lighter* than the panel rather than darker.
        assert_eq!(fl[0].rgb, [0.0; 3]);
        assert!(fl[2].rgb.iter().all(|c| (*c - 1.0).abs() < 1e-6));
    }

    /// With no header or footer there are no bands for them — vanilla skips the
    /// block entirely rather than drawing a zero-height one.
    #[test]
    fn an_absent_header_draws_no_band_at_all() {
        let f = Fixture::with(&[(1, "a")]);
        let v = f.resolve();
        let l = tab_list::layout(&v.input, &v.entries);
        assert!(l.header_band.is_none() && l.footer_band.is_none());
        assert_eq!(fills(&l).len(), 2); // the list band and one row
    }

    /// One ping icon per row, and the bucket is the layout module's.
    #[test]
    fn each_row_carries_its_own_ping_icon() {
        let mut f = Fixture::with(&[(1, "a"), (2, "b"), (3, "c")]);
        f.pings = vec![(1, -1), (2, 10), (3, 5000)];
        let v = f.resolve();
        let l = tab_list::layout(&v.input, &v.entries);
        let ic = icons(&v, &l);
        assert_eq!(ic.len(), 3);
        assert_eq!(
            ic.iter().map(|b| b.icon).collect::<Vec<_>>(),
            [
                rewo_gpu::hud::HudIcon::Ping(0), // negative -> unknown
                rewo_gpu::hud::HudIcon::Ping(5), // 10ms -> five bars
                rewo_gpu::hud::HudIcon::Ping(1), // 5s -> one bar
            ]
        );
        // Ten by eight, at the slot's right end.
        assert_eq!((ic[0].w, ic[0].h), (10.0, 8.0));
    }

    /// A player with no `UPDATE_LATENCY` at all reads as unknown, not as zero.
    #[test]
    fn an_unsent_ping_is_unknown_rather_than_a_perfect_connection() {
        let f = Fixture::with(&[(1, "a")]);
        let v = f.resolve();
        assert_eq!(v.rows[0].ping, PingIcon::Unknown);
        assert_eq!(ping_slot(v.rows[0].ping), 0);
    }

    /// The header wraps at `screenWidth - 50`, and its widths feed the band.
    #[test]
    fn the_header_wraps_at_fifty_pixels_in_from_each_edge() {
        let mut f = Fixture::with(&[(1, "a")]);
        f.screen_width = 100; // wrap budget 50 px = 8 characters at 6px
        f.header = Some(Nbt::String("aaaaaaaa bbbbbbbb".into()));
        let v = f.resolve();
        assert_eq!(v.header.len(), 2);
        assert_eq!(plain(&v.header[0]), "aaaaaaaa");
        assert_eq!(plain(&v.header[1]), "bbbbbbbb");
        assert_eq!(v.input.header_lines, vec![48, 48]);
    }

    /// `maxLineWidth` grows to fit the FOOTER as well, before the header's band
    /// is drawn.
    ///
    /// A pass that measured only the header would draw a band narrower than the
    /// footer's beneath it, and the two are drawn from the same width.
    #[test]
    fn a_wide_footer_widens_the_headers_band_too() {
        let mut f = Fixture::with(&[(1, "a")]);
        f.header = Some(Nbt::String("hi".into()));
        f.footer = Some(Nbt::String("a much wider footer".into()));
        let v = f.resolve();
        let l = tab_list::layout(&v.input, &v.entries);
        let (h, ft) = (l.header_band.unwrap(), l.footer_band.unwrap());
        assert_eq!(h.w, ft.w);
        assert_eq!(l.max_line_width, 19 * 6);
    }

    /// The text emitter produces a line per header line, per row name and per
    /// footer line — and the row's alpha tracks the spectator flag.
    #[test]
    fn the_emitter_fades_a_spectators_row_and_not_the_rest() {
        let mut f = Fixture::with(&[(1, "Ghost"), (2, "Solid")]);
        f.spectators = vec![1];
        let v = f.resolve();
        let l = tab_list::layout(&v.input, &v.entries);
        let t = text(&v, &l, 2.0, &w);
        assert_eq!(t.len(), 2);
        // Sorted: the spectator is last.
        assert_eq!(t[0].text, "Solid");
        assert!((t[0].alpha - 1.0).abs() < 1e-6);
        assert_eq!(t[1].text, "Ghost");
        assert!((t[1].alpha - SPECTATOR_ALPHA).abs() < 1e-6);
        // Both drop a shadow — the five-argument `graphics.text` overload.
        assert!(t.iter().all(|l| l.shadow));
        // `px` scales the layout's GUI coordinates into screen ones.
        assert_eq!(t[0].x, l.entries[0].name.0 as f32 * 2.0);
        assert_eq!(t[0].y, l.entries[0].name.1 as f32 * 2.0);
    }

    // ── The LIST display objective ────────────────────────────────────────

    fn scoreboard_with(render: RenderType, scores: &[(&str, i32)]) -> Scoreboard {
        use rewo_net::scoreboard::{ObjectiveMethod, SetDisplayObjective, SetObjective, SetScore};
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&SetObjective {
            name: "obj".into(),
            method: ObjectiveMethod::Add,
            display: Some(rewo_net::scoreboard::ObjectiveDisplay {
                display_name: Nbt::String("Obj".into()),
                render_type: render,
                number_format: None,
            }),
        });
        for (owner, value) in scores {
            sb.apply_set_score(&SetScore {
                owner: (*owner).into(),
                objective_name: "obj".into(),
                score: *value,
                display: None,
                number_format: None,
            });
        }
        sb.apply_set_display_objective(&SetDisplayObjective {
            slot: DisplaySlot::List,
            objective_name: Some("obj".into()),
        });
        sb
    }

    /// The numeric column's width is the widest `spacer + score`, and the
    /// spacer is charged only where there is a score.
    #[test]
    fn the_score_column_charges_a_spacer_only_for_a_real_score() {
        let mut f = Fixture::with(&[(1, "aa"), (2, "bb")]);
        f.scoreboard = Some(scoreboard_with(RenderType::Integer, &[("aa", 1234)]));
        let v = f.resolve();
        // "1234" is 4 chars * 6 = 24, plus a 6px space.
        assert_eq!(v.input.score, ScoreColumn::Numeric { max_score_width: 30 });
        assert_eq!(plain(&v.rows[0].score), "1234");
        assert_eq!(v.rows[0].score_width, 24);
        // The holder with no score contributes nothing and draws nothing.
        assert!(v.rows[1].score.is_empty());
        assert_eq!(v.rows[1].score_width, 0);
    }

    /// `PLAYER_LIST_DEFAULT` is YELLOW, not the sidebar's red and not white.
    #[test]
    fn an_unformatted_score_is_yellow() {
        let mut f = Fixture::with(&[(1, "aa")]);
        f.scoreboard = Some(scoreboard_with(RenderType::Integer, &[("aa", 7)]));
        let v = f.resolve();
        let c = v.rows[0].score[0].color;
        let yellow = [255.0 / 255.0, 255.0 / 255.0, 85.0 / 255.0];
        for k in 0..3 {
            assert!((c[k] - yellow[k]).abs() < 1e-6, "{c:?} vs {yellow:?}");
        }
        assert_ne!(c[2], 85.0 / 255.0 * 0.0); // guards against a black read
        // …and it is NOT the sidebar's red, which is the swap this can make.
        assert!(c[1] > 0.9, "red would have g = 0x55");
    }

    /// A HEARTS objective reserves ninety pixels and formats no digits.
    ///
    /// The reservation is vanilla's `widthForScore`, and dropping it would move
    /// every name — so the column is reserved even though M151 does not draw
    /// the hearts. Stated in the module doc, asserted here.
    #[test]
    fn a_hearts_objective_reserves_ninety_pixels_and_no_text() {
        let mut f = Fixture::with(&[(1, "aa")]);
        f.scoreboard = Some(scoreboard_with(RenderType::Hearts, &[("aa", 14)]));
        let v = f.resolve();
        assert_eq!(v.input.score, ScoreColumn::Hearts);
        assert!(v.rows[0].score.is_empty());
        let l = tab_list::layout(&v.input, &v.entries);
        // 90 is far more than the 5px minimum, so the span exists…
        assert!(l.entries[0].score_span.is_some());
        // …and the emitter puts nothing in it.
        let t = text(&v, &l, 1.0, &w);
        assert_eq!(t.len(), 1, "the name only");
    }

    /// A spectator gets no score span at all, whatever the objective says.
    #[test]
    fn a_spectator_row_has_no_score() {
        let mut f = Fixture::with(&[(1, "aa"), (2, "bb")]);
        f.spectators = vec![1];
        f.scoreboard = Some(scoreboard_with(RenderType::Integer, &[("aa", 5), ("bb", 6)]));
        let v = f.resolve();
        let l = tab_list::layout(&v.input, &v.entries);
        // Sorted: the spectator is last.
        assert!(l.entries[0].score_span.is_some());
        assert!(l.entries[1].score_span.is_none());
        let t = text(&v, &l, 1.0, &w);
        // Two names and ONE score.
        assert_eq!(t.len(), 3);
    }

    /// The score is right-aligned to the span's right edge, not its left.
    #[test]
    fn the_score_is_right_aligned_to_the_spans_right_edge() {
        let mut f = Fixture::with(&[(1, "aa"), (2, "bb")]);
        f.scoreboard = Some(scoreboard_with(RenderType::Integer, &[("aa", 1), ("bb", 22222)]));
        let v = f.resolve();
        let l = tab_list::layout(&v.input, &v.entries);
        let t = text(&v, &l, 1.0, &w);
        let scores: Vec<&rewo_gpu::world::OwnedTextLine> =
            t.iter().filter(|l| l.text.chars().all(|c| c.is_ascii_digit())).collect();
        assert_eq!(scores.len(), 2);
        // Their RIGHT edges agree; their left edges do not.
        let right0 = scores[0].x + v.rows[0].score_width as f32;
        let right1 = scores[1].x + v.rows[1].score_width as f32;
        assert_eq!(right0, right1);
        assert_ne!(scores[0].x, scores[1].x);
    }

    /// No display objective in the LIST slot means no column and no digits —
    /// the ordinary case, and the control for every test above.
    #[test]
    fn a_sidebar_objective_does_not_reach_the_tab_list() {
        use rewo_net::scoreboard::SetDisplayObjective;
        let mut sb = scoreboard_with(RenderType::Integer, &[("aa", 5)]);
        // Move it to the SIDEBAR slot and clear LIST.
        sb.apply_set_display_objective(&SetDisplayObjective {
            slot: DisplaySlot::List,
            objective_name: None,
        });
        sb.apply_set_display_objective(&SetDisplayObjective {
            slot: DisplaySlot::Sidebar,
            objective_name: Some("obj".into()),
        });
        let mut f = Fixture::with(&[(1, "aa")]);
        f.scoreboard = Some(sb);
        let v = f.resolve();
        assert_eq!(v.input.score, ScoreColumn::None);
        assert!(v.rows[0].score.is_empty());
    }
}

#[cfg(test)]
mod face_tests {
    use super::*;

    /// **Exact string equality, and it is case sensitive.**
    #[test]
    fn only_two_names_are_upside_down_and_the_case_matters() {
        assert!(is_upside_down_name("Dinnerbone"));
        assert!(is_upside_down_name("Grumm"));
        assert!(!is_upside_down_name("dinnerbone"), "case sensitive");
        assert!(!is_upside_down_name("Dinnerbone2"));
        assert!(!is_upside_down_name("Notch"));
    }
}
