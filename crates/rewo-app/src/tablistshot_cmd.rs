//! `rewo tablistshot --check` — the M151 tab-list oracle.
//!
//! The path under test, end to end:
//!
//! ```text
//! raw player_info_update / tab_list / set_objective / set_score bodies (built here)
//!   -> rewo_net::play::parse_player_info, tab_list_text::parse_tab_list, …
//!                                              (the production decoders)
//!   -> tab_list_view::resolve                  (the SAME resolver the frame calls)
//!   -> rewo_gpu::tab_list::layout              (production geometry)
//!   -> tab_list_view::{fills, icons, faces, hearts, text}
//!                                              (the SAME emitters the frame calls)
//!   -> WorldRenderer::{set_hud_fills, set_hud_icons, set_text}
//!   -> Offscreen::read_rgba                    (real pixels)
//! ```
//!
//! **`faces` and `hearts` joined that list at M158, and the three milestones
//! before it are why they are named here explicitly.** M155 shipped both
//! emitters with exactly one caller each — the frame loop — and nothing else in
//! the tree called either: not this gate, which built its icon list from
//! `icons()` alone, and not a unit test, since neither name appears twice in
//! `tab_list_view.rs`. `tools/m158_gap.py` measured it rather than arguing it:
//! replacing each body with `return Vec::new()` left the gate at 34/34 green.
//! The frame loop concatenates `icons`, then `faces`, then `hearts`, and this
//! gate now does the same in the same order, because each later group draws
//! over its own row.
//!
//! ## The rules this gate is built around, each earned elsewhere
//!
//! **The gate drives the real emitters.** M45's `install_shapes` failure and
//! M41's rotted `swingshot` fixture were both gates that had quietly stopped
//! testing their subject because they reimplemented a slice of the app's setup.
//! So the wire bodies go through `rewo_net`'s own decoders and the fills, icons
//! and text come out of `tab_list_view`'s own functions — the width closure is
//! the only thing rebuilt, and it is an input in the windowed path too.
//!
//! **A witness must not compute its expectation from the constant the renderer
//! reads.** M93q's `LOOM_PREVIEW_BACKING` was wrong for a milestone because its
//! pixel witness read the same `pub const` the draw did. So every number
//! predicted below is a **literal transcribed from `extractRenderState`**, and
//! [`check_transcription`] then grades those literals against
//! `rewo_gpu::tab_list::layout` over a range — which is what stops a slip in
//! one copy cancelling a slip in the other.
//!
//! **The detector must not share a colour with its background.** Three detector
//! errors on this project were that shape. So the *names* are made magenta
//! through a `§` colour code the display-name override carries, the clear is
//! white, and `p1` asserts a frame with the list resolved-to-nothing carries
//! none of it.
//!
//! **A control frame must not change with its subject.** M94's `b1` compared an
//! open book against a shut one and read a recipe slot's black border against
//! the *menu*, which had moved. Here the control is the same scene with the key
//! up, and every probe is checked against it at the same coordinates.
//!
//! **Fail-closed** on a fixed [`EXPECTED_WITNESSES`] count.

use clap::Args as ClapArgs;

use rewo_data::assets;
use rewo_data::DataPaths;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::tab_list::{self, PingIcon, ScoreColumn};
use rewo_gpu::world::{SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_proto::nbt::Nbt;
use rewo_world::chat_style::ChatStyle;

use crate::tab_list_view::{self, TabListLookups, TabListView};

/// Total named properties this gate asserts. Locked so a skipped property fails
/// the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 42;

const W: u32 = 640;
const H: u32 = 480;
/// `gui_scale(640, 480)` = 2, asserted by `p0` rather than assumed.
const SCALE: i32 = 2;
const GUI_W: i32 = W as i32 / SCALE;
const GUI_H: i32 = H as i32 / SCALE;

/// `§d` — light purple, `0xFF55FF`. Close enough to magenta for a detector and
/// far enough from white, the two bands' black and the row fills' white that no
/// probe can confuse them.
const MAGENTA_CODE: &str = "\u{a7}d";

#[derive(ClapArgs)]
pub struct TablistshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the convention `sidebarshot`/`titleshot`/`eventshot` use.
    #[arg(long, default_value_t = false)]
    check: bool,
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Write the rendered frames here for eyeballing. Never read back.
    #[arg(long)]
    out_dir: Option<std::path::PathBuf>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[tablistshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

fn client_jar(version: &str) -> Option<std::path::PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

pub fn run(args: TablistshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[tablistshot] mode: {mode} (serverless; the oracle asserts \
         unconditionally). Model half against the 26.2 decompile's \
         `PlayerTabOverlay.extractRenderState`; pixel half against magenta \
         names on a white clear."
    );

    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_transcription(&mut c);
    check_wire(&mut c);
    check_pixels(&mut c, &args, &baked)?;

    println!(
        "[tablistshot] witnesses observed: {} / {}",
        c.witnessed, EXPECTED_WITNESSES
    );
    if !c.failures.is_empty() {
        return Err(format!(
            "{} propert{} failed: {}",
            c.failures.len(),
            if c.failures.len() == 1 { "y" } else { "ies" },
            c.failures.join(", ")
        ));
    }
    if c.witnessed != EXPECTED_WITNESSES {
        return Err(format!(
            "witness count {} != expected {EXPECTED_WITNESSES} — a named \
             property was skipped (fail-closed)",
            c.witnessed
        ));
    }
    println!("[tablistshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ---------------------------------------------------------------------------
// The decompile, transcribed here.
//
// Re-declared rather than imported from `rewo_gpu::tab_list`, for the reason
// `sidebarshot`/`healthbarshot`/`titleshot` all state: a witness that asks the
// implementation what to expect asserts only that the implementation equals
// itself. Where a sample's answer is a small integer it is ALSO pinned as a
// literal below, which is what stops a mis-transcription here cancelling one
// there.
// ---------------------------------------------------------------------------

/// ```java
/// int rows = slots;
/// for (cols = 1; rows > 20; rows = (slots + cols - 1) / cols) { cols++; }
/// ```
fn ref_columns(slots: i32) -> (i32, i32) {
    let mut rows = slots;
    let mut cols = 1;
    while rows > 20 {
        cols += 1;
        rows = (slots + cols - 1) / cols;
    }
    (cols, rows)
}

/// ```java
/// int slotWidth = Math.min(cols * ((showHead ? 9 : 0) + maxNameWidth + widthForScore + 13),
///                          screenWidth - 50) / cols;
/// int xxo = screenWidth / 2 - (slotWidth * cols + (cols - 1) * 5) / 2;
/// ```
fn ref_slot_width(cols: i32, show_head: i32, name_w: i32, score_w: i32, screen_w: i32) -> i32 {
    (cols * (show_head + name_w + score_w + 13)).min(screen_w - 50) / cols
}

/// `xo = xxo + col * slotWidth + col * 5`, `yo = yyo + row * 9`.
fn ref_slot_origin(xxo: i32, yyo: i32, slot_w: i32, col: i32, row: i32) -> (i32, i32) {
    (xxo + col * slot_w + col * 5, yyo + row * 9)
}

fn check_transcription(c: &mut Checker) {
    // Hand-computed, so a slip in the helpers cannot cancel a slip in the
    // layout. 21 players is the interesting one: the loop balances to two
    // columns of ELEVEN, not a column of 20 and a column of 1.
    let literals = ref_columns(0) == (1, 0)
        && ref_columns(1) == (1, 1)
        && ref_columns(20) == (1, 20)
        && ref_columns(21) == (2, 11)
        && ref_columns(41) == (3, 14)
        && ref_columns(80) == (4, 20)
        // The clamp is on the TOTAL and the divide comes after, so it is not
        // the same as clamping a per-column width: 3 columns of 100 wanted,
        // clamped to 270, is 90 each — and clamping each to 270 would give 100.
        && ref_slot_width(3, 0, 87, 0, 320) == 90
        && ref_slot_width(1, 0, 87, 0, 320) == 100
        // showHead costs exactly nine.
        && ref_slot_width(1, 9, 87, 0, 320) == 109
        && ref_slot_origin(10, 10, 90, 0, 0) == (10, 10)
        && ref_slot_origin(10, 10, 90, 1, 2) == (105, 28);

    // The same functions against the production layout, over a range no single
    // fixture could cover.
    let mut production = true;
    for slots in [0usize, 1, 5, 20, 21, 41, 80] {
        for show_head in [false, true] {
            for name_w in [1i32, 40, 87, 400] {
                for screen_w in [320i32, 427, 200] {
                    let entries = fake_entries(slots);
                    let input = tab_list::TabListInput {
                        screen_width: screen_w,
                        show_head,
                        max_name_width: name_w,
                        score: ScoreColumn::None,
                        header_lines: Vec::new(),
                        footer_lines: Vec::new(),
                    };
                    let l = tab_list::layout(&input, &entries);
                    let (cols, rows) = ref_columns(slots as i32);
                    let sw = ref_slot_width(cols, i32::from(show_head) * 9, name_w, 0, screen_w);
                    let xxo = screen_w / 2 - (sw * cols + (cols - 1) * 5) / 2;
                    production &= l.columns == cols && l.rows == rows && l.slot_width == sw;
                    for (i, e) in l.entries.iter().enumerate() {
                        let (col, row) = (i as i32 / rows.max(1), i as i32 % rows.max(1));
                        let (x, y) = ref_slot_origin(xxo, 10, sw, col, row);
                        production &= e.background.x == x
                            && e.background.y == y
                            && e.background.w == sw
                            && e.background.h == 8
                            // Column-MAJOR: entry i is in column i/rows.
                            && e.col == col
                            && e.row == row
                            // The ping icon is measured from the SLOT origin,
                            // not from the text origin past the face.
                            && e.ping_icon.x == x + sw - 11
                            && e.name.0 == x + if show_head { 9 } else { 0 };
                    }
                }
            }
        }
    }
    c.record(
        "m0.the_transcription_matches_its_literals_and_the_production_layout",
        literals && production,
        "column solve / slot width / slot origin / ping inset, graded against \
         hand-computed values and then against `rewo_gpu::tab_list::layout` \
         over 7 player counts x 2 head modes x 4 name widths x 3 window widths",
    );

    // The ping bucket, both ends and every boundary. `extractPingIcon`'s chain
    // is strictly-less-than throughout and INVERTED against the numbers: the
    // best connection is five bars.
    let ping = [
        (-1, PingIcon::Unknown),
        (-1000, PingIcon::Unknown),
        (0, PingIcon::Ping5),
        (149, PingIcon::Ping5),
        (150, PingIcon::Ping4),
        (299, PingIcon::Ping4),
        (300, PingIcon::Ping3),
        (599, PingIcon::Ping3),
        (600, PingIcon::Ping2),
        (999, PingIcon::Ping2),
        (1000, PingIcon::Ping1),
        (30_000, PingIcon::Ping1),
    ];
    let bucket = ping.iter().all(|(ms, want)| tab_list::ping_icon(*ms) == *want);
    // …and the sprite index the renderer uses is the BAR COUNT, not the order
    // the if-chain tests them in.
    let slots = tab_list_view::ping_slot(PingIcon::Unknown) == 0
        && tab_list_view::ping_slot(PingIcon::Ping1) == 1
        && tab_list_view::ping_slot(PingIcon::Ping5) == 5;
    c.record(
        "m1.the_ping_buckets_are_strictly_less_than_and_inverted",
        bucket && slots,
        "0 and 149 are five bars, 1000 and 30000 are one, and only a NEGATIVE \
         latency is unknown — a 30-second ping is still one bar",
    );

    // The two colours, derived from vanilla's signed literals rather than
    // restated — the shape M136's correction of `SPECTATOR_NAME_COLOR` needed.
    let spec = (-1862270977i32) as u32;
    c.record(
        "m2.a_spectator_is_white_at_alpha_144_not_grey",
        spec >> 24 == 144
            && spec & 0x00FF_FFFF == 0x00FF_FFFF
            && tab_list::SPECTATOR_NAME_COLOR == spec
            && (tab_list_view::SPECTATOR_ALPHA - 144.0 / 255.0).abs() < 1e-6
            && (tab_list_view::NAME_ALPHA - 1.0).abs() < 1e-6,
        format!(
            "0x{spec:08X}: alpha 144, RGB full white. A grey reading dims the \
             letters as well as fading them, which is what M52f shipped."
        ),
    );

    // The two fill colours, from their own literals. `Integer.MIN_VALUE` is
    // alpha 128 black; `553648127` is `0x20FFFFFF`, alpha 32 WHITE — so the
    // rows are lighter than the panel, not darker.
    c.record(
        "m3.the_bands_are_black_at_128_and_the_rows_white_at_32",
        tab_list::BAND_COLOR == (i32::MIN as u32)
            && tab_list::BAND_COLOR >> 24 == 128
            && tab_list::BAND_COLOR & 0x00FF_FFFF == 0
            && tab_list::DEFAULT_ROW_BACKGROUND == 553_648_127
            && tab_list::DEFAULT_ROW_BACKGROUND >> 24 == 32
            && tab_list::DEFAULT_ROW_BACKGROUND & 0x00FF_FFFF == 0x00FF_FFFF,
        "Integer.MIN_VALUE = 0x80000000 and 553648127 = 0x20FFFFFF",
    );
}

fn fake_entries(n: usize) -> Vec<tab_list::TabEntry> {
    (0..n)
        .map(|i| tab_list::TabEntry::new(i as u128, format!("p{i:03}"), Some(0)))
        .collect()
}

// ---------------------------------------------------------------------------
// Wire bodies — built here, independent of any writer under test.
// ---------------------------------------------------------------------------

fn varint(v: i32, out: &mut Vec<u8>) {
    let mut n = v as u32;
    loop {
        let b = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
}

fn mc_string(s: &str, out: &mut Vec<u8>) {
    varint(s.len() as i32, out);
    out.extend_from_slice(s.as_bytes());
}

fn nbt_string(s: &str, out: &mut Vec<u8>) {
    out.push(8); // TAG_String
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// One player as `player_info_update` writes them, under the mask
/// `ADD_PLAYER | GAME_MODE | LISTED | LATENCY | DISPLAY_NAME`.
struct WirePlayer {
    uuid: u128,
    name: &'static str,
    gamemode: i32,
    listed: bool,
    latency: i32,
    /// The `UPDATE_DISPLAY_NAME` field: `None` is the action's own null, which
    /// **clears** an override rather than meaning "unchanged".
    display: Option<String>,
}

const INFO_MASK: u8 = (1 << 0) | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5);

fn player_info_body(players: &[WirePlayer]) -> Vec<u8> {
    let mut b = vec![INFO_MASK];
    varint(players.len() as i32, &mut b);
    for p in players {
        b.extend_from_slice(&p.uuid.to_be_bytes());
        mc_string(p.name, &mut b);
        varint(0, &mut b); // no profile properties
        varint(p.gamemode, &mut b);
        b.push(u8::from(p.listed));
        varint(p.latency, &mut b);
        match &p.display {
            Some(s) => {
                b.push(1);
                nbt_string(s, &mut b);
            }
            None => b.push(0),
        }
    }
    b
}

/// The session state a tab list resolves from, built by driving the production
/// decoders with raw bodies.
struct Wire {
    /// **The production state type**, not a copy of its arithmetic
    /// (`rewo_net::play::TabListPlayers`). The first cut of this gate
    /// reimplemented the add/remove on `listed` inline, and M151's mutation
    /// battery reported "UPDATE_LISTED only ever ADDS" as SURVIVED against it
    /// — the gate was grading its own copy, which is M45's `install_shapes`
    /// shape and the reason that arithmetic moved out of `PlaySession`.
    players: rewo_net::play::TabListPlayers,
    names: std::collections::HashMap<u128, String>,
    pings: std::collections::HashMap<u128, i32>,
    spectators: std::collections::HashSet<u128>,
    text: rewo_net::tab_list_text::TabListText,
    scoreboard: rewo_net::scoreboard::Scoreboard,
}

impl Wire {
    /// Apply a `player_info_update` body exactly as `PlaySession` does — the
    /// production `parse_player_info` and the production `TabListPlayers`.
    fn apply(&mut self, body: &[u8]) -> bool {
        let (entries, res) = rewo_net::play::parse_player_info(body);
        for e in entries {
            self.players.apply(&e);
            if let Some(n) = e.name {
                self.names.insert(e.uuid, n);
            }
            if let Some(ms) = e.latency {
                self.pings.insert(e.uuid, ms);
            }
            if let Some(gm) = e.gamemode {
                if gm.is_spectator() {
                    self.spectators.insert(e.uuid);
                } else {
                    self.spectators.remove(&e.uuid);
                }
            }
        }
        res.is_ok()
    }

    fn resolve(&self, online_mode: bool, screen_width: i32, advance: Option<[u8; 256]>) -> TabListView {
        let width_of = move |t: &str, style: ChatStyle| match &advance {
            Some(a) => rewo_gpu::text::width_styled(t, a, style.bold),
            None => 0,
        };
        let name_of = |u: u128| self.names.get(&u).cloned();
        let ping_of = |u: u128| self.pings.get(&u).copied();
        let spectator_of = |u: u128| self.spectators.contains(&u);
        let order_of = |_: u128| 0;
        let team_of = |u: u128| {
            let n = name_of(u)?;
            self.scoreboard.teams.team_of_member(&n).map(str::to_string)
        };
        let display_name_of = |u: u128| self.players.display_name(u).cloned();
        let listed = self.players.listed_players();
        tab_list_view::resolve(&TabListLookups {
            listed: &listed,
            name_of: &name_of,
            ping_of: &ping_of,
            spectator_of: &spectator_of,
            order_of: &order_of,
            team_of: &team_of,
            display_name_of: &display_name_of,
            width_of: &width_of,
            lang: None,
            online_mode,
            screen_width,
            header: self.text.header.as_ref(),
            footer: self.text.footer.as_ref(),
            scoreboard: Some(&self.scoreboard),
        })
    }
}

/// The scene every pixel witness below is measured against: four players on the
/// wire, two of them listed.
fn scene() -> Wire {
    let mut w = Wire {
        players: rewo_net::play::TabListPlayers::default(),
        names: std::collections::HashMap::new(),
        pings: std::collections::HashMap::new(),
        spectators: std::collections::HashSet::new(),
        text: rewo_net::tab_list_text::TabListText::new(),
        scoreboard: rewo_net::scoreboard::Scoreboard::new(),
    };
    // The names are chosen so the SORT is observable: `Zulu` is the ordinary
    // player and `Alpha` is the spectator, so an implementation that sorted on
    // the name alone would put Alpha first. `Mid` sits between them
    // alphabetically and is listed, so the expected order is Mid, Zulu, Alpha.
    let ok = w.apply(&player_info_body(&[
        WirePlayer {
            uuid: 1,
            name: "TabZulu",
            gamemode: 0,
            listed: true,
            latency: 10,
            display: Some(format!("{MAGENTA_CODE}Aaa")),
        },
        WirePlayer {
            uuid: 2,
            name: "TabAlpha",
            gamemode: 3, // SPECTATOR
            listed: true,
            latency: 5000,
            display: Some(format!("{MAGENTA_CODE}Mmm")),
        },
        WirePlayer {
            uuid: 3,
            name: "TabMid",
            gamemode: 0,
            listed: true,
            latency: 200,
            display: Some(format!("{MAGENTA_CODE}Zzz")),
        },
        // Unlisted: in `playerInfoMap` with a name, a ping and a mode, and out
        // of `getListedOnlinePlayers()`.
        WirePlayer {
            uuid: 4,
            name: "TabHidden",
            gamemode: 0,
            listed: false,
            latency: 20,
            display: Some(format!("{MAGENTA_CODE}Hhh")),
        },
    ]));
    assert!(ok, "tablistshot: the scene's player_info body must decode");
    w
}

fn check_wire(c: &mut Checker) {
    let w = scene();
    c.record(
        "w0.the_unlisted_player_is_decoded_and_not_listed",
        w.players.listed_players().len() == 3
            && !w.players.listed.contains(&4)
            // …and everything else about them still resolved, which is what
            // makes this a LIST filter rather than a decode failure.
            && w.names.get(&4).map(String::as_str) == Some("TabHidden")
            && w.pings.get(&4) == Some(&20)
            && w.players.display_name(4).is_some(),
        format!(
            "{} listed of 4 on the wire; the unlisted one keeps its name, ping \
             and display override",
            w.players.listed_players().len()
        ),
    );

    // The case M151's first mutation battery found NOTHING covered: a player
    // who was listed and is then UNLISTED must leave. A client that only ever
    // added keeps a vanished player on the list for the rest of the session,
    // and every fixture up to here only ever sent `listed: false` for someone
    // who had never been added — under which the two readings agree exactly.
    let mut w3 = scene();
    w3.apply(&player_info_body(&[WirePlayer {
        uuid: 1,
        name: "TabZulu",
        gamemode: 0,
        listed: false,
        latency: 10,
        display: None,
    }]));
    c.record(
        "w3.a_player_unlisted_after_being_listed_leaves_the_list",
        w.players.listed.contains(&1)
            && !w3.players.listed.contains(&1)
            && w3.players.listed_players().len() == 2
            // …and they are still a known player, which is the whole point of
            // the flag: a vanish plugin keeps the profile and drops the row.
            && w3.names.get(&1).map(String::as_str) == Some("TabZulu"),
        format!(
            "{} listed after the unlist, from 3 — and the profile survives",
            w3.players.listed_players().len()
        ),
    );

    // `UPDATE_DISPLAY_NAME` with a null field CLEARS. The action bit is set in
    // both bodies, so a decoder that collapsed the two Options would leave the
    // override in place and the profile name unreachable.
    let mut w2 = scene();
    w2.apply(&player_info_body(&[WirePlayer {
        uuid: 1,
        name: "TabZulu",
        gamemode: 0,
        listed: true,
        latency: 10,
        display: None,
    }]));
    c.record(
        "w1.a_null_display_name_clears_the_override_rather_than_being_ignored",
        w.players.display_name(1).is_some() && w2.players.display_name(1).is_none(),
        "the same action bit, a present component then a null one",
    );

    // The header/footer packet: two components, and "no header" is a component
    // whose FLATTENED TEXT is empty rather than an absent field.
    let mut t = rewo_net::tab_list_text::TabListText::new();
    let mut body = Vec::new();
    nbt_string("Tab Head", &mut body);
    nbt_string("", &mut body);
    t.apply(&rewo_net::tab_list_text::parse_tab_list(&body).expect("tab_list"));
    c.record(
        "w2.an_empty_footer_component_means_no_footer_rather_than_an_empty_one",
        t.header.is_some() && t.footer.is_none(),
        "a header and a component that renders as no characters",
    );

    // ── M155: the health column and the faces ───────────────────────────────
    //
    // These grade the PRODUCTION emitters, not a re-derivation: `hearts` and
    // `faces` are the same functions the frame loop calls, so a regression in
    // either reaches this gate rather than only the unit tests one crate down.
    use rewo_gpu::tab_list::{heart_blits, heart_counts, HealthState, HeartSprite};

    // h0 — the two counts round in OPPOSITE directions, swept against
    // literals re-declared from PlayerTabOverlay:274-275.
    let mut counts_agree = true;
    let mut saw_full_exceed = false;
    for score in 0..=60 {
        for displayed in 0..=60 {
            let (full, render) = heart_counts(score, displayed);
            if full != -(-(score.max(displayed))).div_euclid(2)
                || render != score.max(displayed.max(20)) / 2
            {
                counts_agree = false;
            }
            if full > render {
                saw_full_exceed = true;
            }
        }
    }
    c.record(
        "h0.the_two_heart_counts_round_two_different_ways",
        counts_agree && saw_full_exceed,
        format!(
            "swept 61x61 (score, displayed); fullHearts CEILS and heartsToRender \
             FLOORS, and the sweep reached the case where full exceeds render \
             (e.g. 21 -> {:?}) — at which point the container-only loop runs zero times",
            heart_counts(21, 21)
        ),
    );

    // h1 — the text readout is reached by HIGH HEALTH, never by a narrow
    // window, because the column is a constant 90.
    let at = |s: i32| heart_blits(s, HealthState::new(s), 0, 0, rewo_gpu::tab_list::HEARTS_WIDTH);
    c.record(
        "h1.the_text_readout_is_reached_by_high_health_not_a_narrow_window",
        !at(42).is_empty() && at(44).is_empty(),
        format!(
            "score 42 emits {} heart blits and score 44 emits {} — the column is a \
             constant {}px, so heartsToRender is the only free variable",
            at(42).len(),
            at(44).len(),
            rewo_gpu::tab_list::HEARTS_WIDTH
        ),
    );

    // h2 — a filled heart is TWO layers, container then fill, in that order.
    let full20 = at(20);
    let first_two: Vec<HeartSprite> =
        full20.iter().filter(|b| b.dx == 0).map(|b| b.sprite).collect();
    c.record(
        "h2.a_filled_heart_sits_on_its_own_container",
        first_two == vec![HeartSprite::Container, HeartSprite::Full],
        format!("the blits at dx=0 are {first_two:?} — the order IS the draw order"),
    );

    // h3 — the ghost layer is the OLD value. Drives the real clock.
    let mut hurt = HealthState::new(20);
    hurt.update(10, 0);
    let ghosts = heart_blits(10, hurt, 3, 0, rewo_gpu::tab_list::HEARTS_WIDTH)
        .iter()
        .filter(|b| b.sprite == HeartSprite::FullBlinking)
        .count();
    c.record(
        "h3.the_blink_ghost_is_drawn_from_the_displayed_value",
        hurt.is_blinking(3) && ghosts == 10,
        format!(
            "after 20 -> 10 the ghost spans {ghosts} hearts (the OLD value), while the \
             live fill is five — and tick 3 is the FIRST lit tick, because the square \
             wave is measured from the END of the blink"
        ),
    );

    // h4 — a damage blink starts DARK and a heal blink starts LIT. The two
    // durations land on opposite phases of the six-tick cycle.
    let mut down = HealthState::new(20);
    down.update(19, 100);
    let mut up = HealthState::new(20);
    up.update(21, 100);
    c.record(
        "h4.a_damage_blink_starts_dark_and_a_heal_blink_starts_lit",
        !down.is_blinking(100) && up.is_blinking(100),
        "20 % 6 == 2 (below the 3 threshold) against 10 % 6 == 4 — an implementation \
         measuring from the START of the blink gets both backwards",
    );

    // h5 — zero health draws nothing rather than ten empty containers.
    c.record(
        "h5.zero_health_draws_nothing_rather_than_ten_containers",
        at(0).is_empty() && !at(1).is_empty(),
        "the whole body is gated on fullHearts > 0",
    );

    // h6 — the eleventh heart is absorption, and its sprite is a *_blinking
    // asset that is NOT the blink layer.
    let h22 = HealthState::new(22);
    let per = rewo_gpu::tab_list::width_per_heart(0, rewo_gpu::tab_list::HEARTS_WIDTH, 11);
    let blits22 = heart_blits(22, h22, 0, 0, rewo_gpu::tab_list::HEARTS_WIDTH);
    let tenth_is_gold = blits22
        .iter()
        .any(|b| b.dx == 10 * per && b.sprite == HeartSprite::AbsorbingFull);
    c.record(
        "h6.the_eleventh_heart_is_absorption_while_nothing_is_blinking",
        tenth_is_gold && !h22.is_blinking(0) && per == 7,
        format!(
            "at 22 health the pitch is {per} (11 hearts, not 10) and index 10 is \
             AbsorbingFull with no blink in flight — vanilla ships no non-blinking \
             absorbing sprite, so the *_blinking asset IS the ordinary gold heart"
        ),
    );

    // f0 — the flip is case sensitive and needs the player LOADED.
    c.record(
        "f0.only_two_exact_names_are_upside_down",
        tab_list_view::is_upside_down_name("Dinnerbone")
            && tab_list_view::is_upside_down_name("Grumm")
            && !tab_list_view::is_upside_down_name("dinnerbone"),
        "exact String.equals against two literals — lower case is not flipped",
    );
}


// ---------------------------------------------------------------------------
// Pixels.
// ---------------------------------------------------------------------------

fn srgb_to_linear(s: f32) -> f32 {
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

struct Frame {
    px: Vec<u8>,
}

impl Frame {
    fn at(&self, x: i32, y: i32) -> [u8; 4] {
        let i = ((y as usize) * W as usize + x as usize) * 4;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }
    /// The GUI pixel at `(gx, gy)`, sampled at its centre so a half-pixel
    /// rounding cannot decide the answer.
    fn gui(&self, gx: i32, gy: i32) -> [u8; 4] {
        self.at(gx * SCALE + SCALE / 2, gy * SCALE + SCALE / 2)
    }
    /// A magenta-hued texel at a name's own brightness.
    ///
    /// **The hue test is RELATIVE and the brightness test is on red alone**,
    /// and both halves were earned by a failing run. The first cut was
    /// `sidebarshot`'s absolute `r > 200 && b > 200 && g < 110`, which found
    /// only TWO of the three rows: the spectator's is drawn at alpha 144/255,
    /// and `0xFF55FF` composited at that alpha over the row's own background
    /// lands near `(234, 149, 234)` — its green is *above* 110 and its red is
    /// below 200. So the detector could not see the one row whose treatment
    /// this milestone is about, which is this project's recurring detector
    /// error in its exact shape.
    ///
    /// The floor of 150 is what still excludes the drop shadow, which
    /// `TextPass` darkens by 0.25 in LINEAR space — `encode_srgb(0.25)` is 137
    /// at full alpha. The bitmap font is nearest-sampled at an integer scale,
    /// so there is no antialiasing to land between the two.
    fn is_magenta(&self, gx: i32, gy: i32) -> bool {
        let p = self.gui(gx, gy);
        let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
        r > 150 && r - g > 40 && b - g > 40
    }
    fn magenta_bbox(&self) -> Option<(i32, i32, i32, i32)> {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for gy in 0..GUI_H {
            for gx in 0..GUI_W {
                if self.is_magenta(gx, gy) {
                    x0 = x0.min(gx);
                    y0 = y0.min(gy);
                    x1 = x1.max(gx);
                    y1 = y1.max(gy);
                }
            }
        }
        (x0 <= x1).then_some((x0, y0, x1, y1))
    }
    /// The contiguous GUI-row runs carrying magenta.
    fn magenta_row_bands(&self) -> Vec<(i32, i32)> {
        let mut bands = Vec::new();
        let mut start: Option<i32> = None;
        for gy in 0..GUI_H {
            let any = (0..GUI_W).any(|gx| self.is_magenta(gx, gy));
            match (any, start) {
                (true, None) => start = Some(gy),
                (false, Some(s)) => {
                    bands.push((s, gy - 1));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            bands.push((s, GUI_H - 1));
        }
        bands
    }
    /// The leftmost magenta GUI column over rows `y0..=y1`.
    fn magenta_left_in(&self, y0: i32, y1: i32) -> Option<i32> {
        (0..GUI_W).find(|gx| (y0..=y1).any(|gy| self.is_magenta(*gx, gy)))
    }
    /// The mean magenta intensity over a row band, as a fraction of full — the
    /// alpha probe. Averaged over the whole band rather than sampled at one
    /// texel because a glyph's ink is one bit and its coverage varies.
    fn magenta_peak_in(&self, y0: i32, y1: i32) -> f32 {
        let mut best = 0.0f32;
        for gy in y0..=y1 {
            for gx in 0..GUI_W {
                let p = self.gui(gx, gy);
                // A magenta-hued texel of any brightness: red and blue high
                // relative to green.
                if p[0] as i32 > p[1] as i32 + 20 && p[2] as i32 > p[1] as i32 + 20 {
                    best = best.max(p[0] as f32 / 255.0);
                }
            }
        }
        best
    }
}

/// Recover the alpha a fill was composited with, from the observed byte and the
/// observed clear byte. The attachment is SRGB, so the blend ran on linear
/// values.
fn recovered_alpha(band: [u8; 4], clear: [u8; 4], src: f32) -> f32 {
    let mut sum = 0.0;
    for ch in 0..3 {
        let b = srgb_to_linear(band[ch] as f32 / 255.0);
        let cl = srgb_to_linear(clear[ch] as f32 / 255.0);
        // `b = src * a + cl * (1 - a)`  ->  `a = (cl - b) / (cl - src)`.
        sum += (cl - b) / (cl - src).max(1e-6);
    }
    sum / 3.0
}

fn check_pixels(
    c: &mut Checker,
    args: &TablistshotArgs,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[tablistshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("tablistshot: Vulkan validation requested but not active".into());
    }
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let white: Vec<u8> = vec![255u8; 16 * 16 * 4];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
    wr.set_sky_mode(SkyMode::None);
    let sprites = crate::live_cmd::hud_sprites(baked).ok_or("hud sprites missing from the jar")?;
    wr.init_hud(&mut gpu, &sprites)?;
    let font = crate::live_cmd::font_data(baked).ok_or("no baked font")?;
    wr.init_text(&mut gpu, &font)?;
    let advance = baked.font.as_ref().ok_or("no baked font")?.advance;

    c.record(
        "p0.the_gui_scale_is_the_one_the_predictions_assume",
        rewo_gpu::hud::gui_scale(W as f32, H as f32) == SCALE as f32,
        format!("{W}x{H} -> scale {SCALE}, GUI space {GUI_W}x{GUI_H}"),
    );

    let ring = crate::stats::OverlayRing::default();
    let overlay_draw = rewo_gpu::overlay::OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    };
    let vp = glam::Mat4::IDENTITY.to_cols_array_2d();
    let clear = [1.0, 1.0, 1.0, 1.0];
    let px = SCALE as f32;
    let width_of = move |t: &str, style: ChatStyle| rewo_gpu::text::width_styled(t, &advance, style.bold);

    let mut shot = |gpu: &mut Gpu,
                    off: &mut Offscreen,
                    wr: &mut WorldRenderer,
                    fills: Vec<rewo_gpu::hud::HudFill>,
                    icons: Vec<rewo_gpu::hud::HudBlit>,
                    lines: Vec<rewo_gpu::world::OwnedTextLine>|
     -> Result<Frame, String> {
        // `WorldRenderer::draw` gates the HUD pass on `hud_state` being set, so
        // a gate that only pushed fills would render none of them — which is
        // how the first run of `sidebarshot` reported an alpha of exactly zero.
        // The hotbar it turns on sits at the bottom centre; the tab list is at
        // the top, and `p1` proves the control frame carries no magenta with all
        // of it drawn.
        wr.set_hud(0.0, 0, 0, rewo_gpu::hud::HudGauges::default());
        wr.set_hud_fills(fills);
        wr.set_hud_icons(icons);
        wr.set_text(lines);
        off.render(gpu, Some((&mut *wr, vp)), &overlay_draw, clear)?;
        Ok(Frame { px: off.read_rgba(gpu)? })
    };

    let w = scene();
    let view = w.resolve(false, GUI_W, Some(advance));
    let layout = tab_list::layout(&view.input, &view.entries);

    // -- the control: the key is up ------------------------------------------
    //
    // Everything but the subject held constant — same renderer, same clear,
    // same frame, same hotbar. Only the tab list is absent, which is exactly
    // what the key being up does.
    let control = shot(&mut gpu, &mut off, &mut wr, Vec::new(), Vec::new(), Vec::new())?;

    let fills = tab_list_view::fills(&layout);
    let icons = tab_list_view::icons(&view, &layout);
    let frame = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        fills.clone(),
        icons.clone(),
        tab_list_view::text(&view, &layout, px, &width_of),
    )?;

    c.record(
        "p1.the_key_up_and_the_hud_hidden_both_draw_nothing",
        !tab_list_view::visible(false, false)
            && !tab_list_view::visible(true, true)
            && tab_list_view::visible(true, false)
            && control.magenta_bbox().is_none(),
        "the control frame carries no magenta at all, which is what makes \
         every magenta measurement below the tab list's",
    );

    // -- the sort ------------------------------------------------------------
    //
    // Three listed players, and the spectator must be LAST despite sorting
    // first by name. Read off the resolved entries and then confirmed against
    // the drawn rows.
    let order: Vec<&str> = view.entries.iter().map(|e| e.name.as_str()).collect();
    c.record(
        "p2.the_spectator_sorts_last_and_the_unlisted_player_is_absent",
        order == ["TabMid", "TabZulu", "TabAlpha"] && view.rows.len() == 3,
        format!(
            "{order:?} — a name-only sort gives [TabAlpha, TabMid, TabZulu] \
             and a client that ignored `listed` gives four rows"
        ),
    );

    let bands = frame.magenta_row_bands();
    c.record(
        "p3.exactly_three_name_rows_reached_the_frame",
        bands.len() == 3,
        format!("{} magenta row bands at {bands:?}", bands.len()),
    );

    // -- the row pitch -------------------------------------------------------
    //
    // Nine, against a background eight tall — the one-pixel difference is the
    // gap between rows, and making them equal turns the list into one block.
    let pitch_ok = bands.len() == 3
        && bands[1].0 - bands[0].0 == 9
        && bands[2].0 - bands[1].0 == 9
        // …and the two ordinary rows are the same height, so the pitch is a
        // stride rather than an artefact of one row rendering taller.
        && bands[0].1 - bands[0].0 == bands[1].1 - bands[1].0;
    c.record(
        "p4.the_rows_are_nine_apart_over_backgrounds_eight_tall",
        pitch_ok
            && layout.entries[0].background.h == 8
            && layout.entries[1].background.y - layout.entries[0].background.y == 9,
        format!("bands {bands:?}; background height {}", layout.entries[0].background.h),
    );

    // -- the spectator's fade ------------------------------------------------
    //
    // The bottom band is the spectator. Its glyphs are the same colour at the
    // same scale on the same background, so the only thing that can differ is
    // the alpha.
    //
    // **Three rows, not two.** The two ordinary rows have to AGREE with each
    // other as well as differing from the spectator's — otherwise a client that
    // faded every row satisfies a two-row comparison just as well. Indexed
    // through `get` because `p3` may already have found fewer than three.
    let peak = |i: usize| {
        bands
            .get(i)
            .map_or(0.0, |b: &(i32, i32)| frame.magenta_peak_in(b.0, b.1))
    };
    let (bright, bright1, faded) = (peak(0), peak(1), peak(2));
    c.record(
        "p5.the_spectators_row_is_faded_rather_than_recoloured",
        bands.len() == 3
            && (bright - bright1).abs() < 0.02
            && faded < bright - 0.05
            // Still magenta: a GREY reading would drop the red channel with the
            // rest and the hue test in `magenta_peak_in` would find nothing.
            && faded > 0.3,
        format!(
            "peak red {bright:.3} / {bright1:.3} on the two ordinary rows vs \
             {faded:.3} on the spectator's — alpha {:.3} of the same colour, \
             not a grey and not applied to everything",
            tab_list_view::SPECTATOR_ALPHA
        ),
    );

    // -- the two fill colours ------------------------------------------------
    //
    // `band(y1, y2)` spans `screenWidth/2 ± maxLineWidth/2`, offset by ONE in
    // each direction — so with one column `maxLineWidth == slotWidth` and the
    // band's left edge is exactly `xxo - 1`. That makes `row0.x - 1` the one
    // column that is band-and-not-row, and `row0.x - 2` the first clear one.
    // The first cut of this probed `- 2` for both and recovered an alpha of
    // exactly zero, which is what an off-by-one against a band edge looks
    // like — and then overflowed `u8` comparing 255 + 8.
    let row0 = &layout.entries[0].background;
    let probe_band_y = layout.list_band.y + 1;
    let clear_px = control.gui(row0.x - 1, probe_band_y);
    let band_px = frame.gui(row0.x - 1, probe_band_y);
    let a_band = recovered_alpha(band_px, clear_px, 0.0);
    c.record(
        "p6.the_list_band_composited_at_alpha_128_black",
        (a_band - 128.0 / 255.0).abs() < 0.02
            // …and it stops there. One pixel further out is the untouched
            // clear, which is what says the `- 1` in `band()` is a one and not
            // a nought or a two.
            && frame.gui(row0.x - 2, probe_band_y) == control.gui(row0.x - 2, probe_band_y)
            && layout.list_band.x == row0.x - 1,
        format!(
            "recovered {a_band:.4} against 128/255 = {:.4}; column {} is \
             banded and {} is clear",
            128.0 / 255.0,
            row0.x - 1,
            row0.x - 2
        ),
    );
    // The rows are WHITE at alpha 32 over a white clear, which composites to…
    // white. So the row fill is probed against the BAND, where it is the only
    // thing that can lighten the pixel: a row sits on top of the band, so the
    // row's pixel is lighter than the band's one column to its left.
    let lum = |p: [u8; 4]| p[0] as i32 + p[1] as i32 + p[2] as i32;
    let in_row = frame.gui(row0.x + 2, row0.y + 4);
    let beside_row = frame.gui(row0.x - 1, row0.y + 4);
    c.record(
        "p7.a_rows_background_lightens_the_band_it_sits_on",
        lum(in_row) > lum(beside_row) + 24,
        format!(
            "inside the row {in_row:?} vs beside it {beside_row:?} — white at \
             alpha 32 over the band's black at 128. Reading the row's colour as \
             BLACK darkens it instead, which is the same delta with the sign \
             flipped."
        ),
    );

    // -- the ping icons ------------------------------------------------------
    //
    // Three blits, at the slot's right end, and each one the bucket its latency
    // maps to. The sprite index is asserted from the model and the *presence*
    // from pixels: the icons are grey-white bars, so a hue detector cannot see
    // them — what can is that the frame differs from one drawn without them at
    // exactly the icon rects and nowhere else.
    let no_icons = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        fills.clone(),
        Vec::new(),
        tab_list_view::text(&view, &layout, px, &width_of),
    )?;
    let mut changed_in_icon = 0usize;
    let mut changed_outside = 0usize;
    for gy in 0..GUI_H {
        for gx in 0..GUI_W {
            if frame.gui(gx, gy) != no_icons.gui(gx, gy) {
                let inside = icons.iter().any(|b| {
                    (gx as f32) >= b.x
                        && (gx as f32) < b.x + b.w
                        && (gy as f32) >= b.y
                        && (gy as f32) < b.y + b.h
                });
                if inside {
                    changed_in_icon += 1;
                } else {
                    changed_outside += 1;
                }
            }
        }
    }
    c.record(
        "p8.the_ping_icons_draw_and_draw_only_inside_their_own_rects",
        changed_in_icon > 0 && changed_outside == 0,
        format!(
            "{changed_in_icon} GUI pixels changed inside the three icon rects \
             and {changed_outside} outside — a wrong atlas placement would \
             sample a neighbouring sprite and still be inside"
        ),
    );
    c.record(
        "p9.each_rows_icon_is_the_bucket_its_latency_maps_to",
        icons.len() == 3
            && icons.iter().map(|b| b.icon).collect::<Vec<_>>()
                == vec![
                    // Sorted: TabMid (200ms), TabZulu (10ms), TabAlpha (5000ms).
                    rewo_gpu::hud::HudIcon::Ping(4),
                    rewo_gpu::hud::HudIcon::Ping(5),
                    rewo_gpu::hud::HudIcon::Ping(1),
                ]
            && icons.iter().all(|b| (b.w, b.h) == (10.0, 8.0)),
        format!("{:?}", icons.iter().map(|b| b.icon).collect::<Vec<_>>()),
    );
    // The icon's inset is measured from the SLOT origin, not from the text
    // origin past the face — vanilla passes `xo - (showHead ? 9 : 0)` back in
    // explicitly, which is the one place that subtraction appears.
    let head_view = w.resolve(true, GUI_W, Some(advance));
    let head_layout = tab_list::layout(&head_view.input, &head_view.entries);
    let head_icons = tab_list_view::icons(&head_view, &head_layout);
    c.record(
        "p10.the_ping_inset_is_from_the_slot_not_from_the_name",
        head_icons[0].x
            == (head_layout.entries[0].background.x + head_layout.slot_width - 11) as f32
            && head_layout.entries[0].name.0 == head_layout.entries[0].background.x + 9,
        format!(
            "with a face the name starts 9 in and the icon still ends 11 from \
             the SLOT's right edge (icon x {}, slot x {})",
            head_icons[0].x, head_layout.entries[0].background.x
        ),
    );

    // -- online mode -----------------------------------------------------
    c.record(
        "p11.online_mode_widens_every_slot_by_the_faces_nine_pixels",
        head_layout.slot_width - layout.slot_width == 9
            && layout.entries.iter().all(|e| e.face.is_none())
            && head_layout.entries.iter().all(|e| e.face.is_some())
            && tab_list_view::face_rects(&layout).is_empty()
            && tab_list_view::face_rects(&head_layout).len() == 3,
        format!(
            "{} -> {} px. This is the RESERVATION, which matters on its own \
             because getting it wrong moves every name; the faces themselves \
             are drawn and `f1`-`f3` grade them. (This detail string claimed \
             they were not drawn for three milestones after M155 drew them.)",
            layout.slot_width, head_layout.slot_width
        ),
    );

    // -- the header and footer ----------------------------------------------
    let mut hw = scene();
    let mut body = Vec::new();
    nbt_string(&format!("{MAGENTA_CODE}TabHeaderLine"), &mut body);
    nbt_string(&format!("{MAGENTA_CODE}TabFooterLine"), &mut body);
    hw.text
        .apply(&rewo_net::tab_list_text::parse_tab_list(&body).expect("tab_list packet"));
    let hview = hw.resolve(false, GUI_W, Some(advance));
    let hlayout = tab_list::layout(&hview.input, &hview.entries);
    let hframe = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        tab_list_view::fills(&hlayout),
        tab_list_view::icons(&hview, &hlayout),
        tab_list_view::text(&hview, &hlayout, px, &width_of),
    )?;
    let hbands = hframe.magenta_row_bands();
    c.record(
        "p12.a_header_and_footer_add_a_band_each_above_and_below_the_rows",
        hbands.len() == 5
            && hlayout.header_band.is_some()
            && hlayout.footer_band.is_some()
            // A header band, the list band, three rows and a footer band.
            && tab_list_view::fills(&hlayout).len() == 6,
        format!(
            "{} magenta bands at {hbands:?} (a header line, three rows, a \
             footer line) and {} fills",
            hbands.len(),
            tab_list_view::fills(&hlayout).len()
        ),
    );
    // Centred over the panel, both of them, and the panel is centred on the
    // SCREEN — so the two lines share a centre with each other.
    // The FIRST band is the header line: it sits above the list band, and the
    // three rows and the footer follow it.
    let header_band = hbands.first().copied().unwrap_or((0, 0));
    let hleft = hframe
        .magenta_left_in(header_band.0, header_band.1)
        .unwrap_or(-1);
    c.record(
        "p13.the_header_line_is_centred_on_the_screen",
        (hleft - (GUI_W / 2 - hview.input.header_lines[0] / 2)).abs() <= 1
            && hlayout.header_line_origins[0].0 == GUI_W / 2 - hview.input.header_lines[0] / 2,
        format!(
            "observed left {hleft}, predicted screenWidth/2 - lineWidth/2 = {}",
            GUI_W / 2 - hview.input.header_lines[0] / 2
        ),
    );

    // -- the wrap ------------------------------------------------------------
    //
    // `font.split(header, screenWidth - 50)`. Fifty in from each edge, so a
    // header wider than that wraps; a 320-wide window budgets 270.
    let long: String = format!("{MAGENTA_CODE}{}", "wrap ".repeat(40));
    let mut ww = scene();
    let mut b2 = Vec::new();
    nbt_string(&long, &mut b2);
    nbt_string("", &mut b2);
    ww.text
        .apply(&rewo_net::tab_list_text::parse_tab_list(&b2).expect("tab_list packet"));
    let wview = ww.resolve(false, GUI_W, Some(advance));
    // The claim is the GREEDY property, not a loose band. `StringSplitter`
    // breaks at the last space that fits, so every line is within the budget
    // AND one more word would exceed it — which is what pins the budget at
    // `screenWidth - 50` rather than at any other number a line happens to fall
    // under. A first cut asserted "widest > screenWidth - 60" and failed at 248
    // of 270, correctly: one more "wrap " is 26 px, so the greedy answer really
    // does sit 22 short.
    let widest = wview
        .header
        .iter()
        .map(|l| line_width(l, &width_of))
        .max()
        .unwrap_or(0);
    let one_more = width_of("wrap ", ChatStyle::plain([1.0, 1.0, 1.0]));
    c.record(
        "p14.the_header_wraps_greedily_at_fifty_pixels_in_from_each_edge",
        wview.header.len() > 1
            && widest <= GUI_W - 50
            && widest + one_more > GUI_W - 50
            // …and the budget is not the whole window: an unwrapped line would
            // be far wider than the screen.
            && one_more * 40 > GUI_W,
        format!(
            "{} wrapped lines, widest {widest} against a budget of {}; one \
             more word is {one_more} px, so {} would overflow — which is the \
             greedy break and is what pins the budget",
            wview.header.len(),
            GUI_W - 50,
            widest + one_more
        ),
    );

    // -- the LIST scoreboard objective --------------------------------------
    let sw = scored_scene();
    let sview = sw.resolve(false, GUI_W, Some(advance));
    let slayout = tab_list::layout(&sview.input, &sview.entries);
    let slines_len = tab_list_view::text(&sview, &slayout, px, &width_of).len();
    c.record(
        "p15.a_list_objective_adds_a_right_aligned_score_to_every_non_spectator",
        matches!(sview.input.score, ScoreColumn::Numeric { .. })
            // Three names + two scores: the spectator gets none.
            && slines_len == 5
            && slayout.entries[2].score_span.is_none(),
        format!(
            "{} text lines, score column {:?}; the spectator's span is {:?}",
            slines_len,
            sview.input.score,
            slayout.entries[2].score_span
        ),
    );
    let sframe = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        tab_list_view::fills(&slayout),
        tab_list_view::icons(&sview, &slayout),
        tab_list_view::text(&sview, &slayout, px, &width_of),
    )?;
    // The scores are YELLOW (`PLAYER_LIST_DEFAULT`), which is neither the
    // sidebar's red nor white — and a yellow texel is red+green high with blue
    // LOW, the inverse of the magenta the names use.
    let mut yellow = 0usize;
    for gy in 0..GUI_H {
        for gx in 0..GUI_W {
            let p = sframe.gui(gx, gy);
            if p[0] > 200 && p[1] > 200 && p[2] < 150 {
                yellow += 1;
            }
        }
    }
    c.record(
        "p16.an_unformatted_score_is_yellow_not_the_sidebars_red",
        yellow > 0
            && sview.rows[0].score[0].color[1] > 0.9
            && (sview.rows[0].score[0].color[2] - 85.0 / 255.0).abs() < 1e-6,
        format!(
            "{yellow} yellow GUI pixels; the span's colour is {:?} where \
             SIDEBAR_DEFAULT would be [1.0, 0.333, 0.333]",
            sview.rows[0].score[0].color
        ),
    );

    // -- the whole panel is inside the window --------------------------------
    //
    // The M135 shape: a producer that multiplied by the GUI scale before
    // handing over a `HudFill` puts every rect `scale` times too far down,
    // which is OFF THE SCREEN and therefore absent rather than misplaced. A
    // gate rendering at scale 1 cannot see it; this one is at scale 2.
    let all_fills = tab_list_view::fills(&hlayout);
    let inside = all_fills.iter().all(|f| {
        f.x >= -2.0 && f.y >= 0.0 && f.x + f.w <= GUI_W as f32 + 2.0 && f.y + f.h <= GUI_H as f32
    });
    c.record(
        "p17.every_fill_is_in_GUI_pixels_and_lands_inside_the_window",
        inside && SCALE == 2,
        format!(
            "{} fills, all within {GUI_W}x{GUI_H} GUI pixels at scale {SCALE}; \
             a producer that multiplied by the scale first would put the \
             bottom band at y {} of {GUI_H}",
            all_fills.len(),
            all_fills.last().map(|f| f.y * SCALE as f32).unwrap_or(0.0)
        ),
    );

    // -- M158: the health column and the faces, in PIXELS ---------------------
    //
    // M155 shipped `tab_list_view::hearts` and `tab_list_view::faces`. Each had
    // exactly ONE caller — the frame loop — and **nothing else in the tree
    // called either**: not this gate, which built its icon list from `icons()`
    // alone, and not a unit test, since neither name appears twice in
    // `tab_list_view.rs`. Measured rather than argued: replacing each body with
    // `return Vec::new()` left this gate at 34/34 (`tools/m158_gap.py`). That is
    // M45's `install_shapes` failure — a gate that reimplements a slice of the
    // app's setup misses whatever the app adds to it — so the list below is
    // built the way `live_cmd` builds it: `icons`, then `faces`, then `hearts`,
    // in that order, because each later group draws over its own row.
    //
    // **Every colour predicted here is a literal read out of the jar's own
    // PNGs**, not a constant the renderer shares (gotcha 0a). They arrive
    // unmodified: the readback carries `[255,19,19]` and `[212,175,55]` exactly.
    const HEART_FULL_RED: [u8; 3] = [255, 19, 19];
    const HEART_CONTAINER_GREY: [u8; 3] = [40, 40, 40];
    const HEART_ABSORB_GOLD: [u8; 3] = [212, 175, 55];

    let hw = hearts_scene();
    let hv = hw.resolve(true, GUI_W, Some(advance));
    let hl = tab_list::layout(&hv.input, &hv.entries);
    let face_slot = wr.upload_tab_face(&mut gpu, &face_strip());

    // The same three lists the frame loop concatenates, in the same order.
    let mut states = std::collections::HashMap::new();
    let hb = tab_list_view::hearts(&hv, &hl, &mut states, 0);
    let bare = tab_list_view::faces(&hv, &hl, &|_| face_slot, &|_| false, &|_| false);
    let mut with_hearts = tab_list_view::icons(&hv, &hl);
    with_hearts.extend(bare.clone());
    with_hearts.extend(hb.clone());
    let hframe2 = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        tab_list_view::fills(&hl),
        with_hearts.clone(),
        tab_list_view::text(&hv, &hl, px, &width_of),
    )?;
    // The control: the identical scene with the heart blits withheld. Only the
    // subject changes, which is what p8 established for the ping icons.
    let mut no_hearts = tab_list_view::icons(&hv, &hl);
    no_hearts.extend(bare.clone());
    let hcontrol = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        tab_list_view::fills(&hl),
        no_hearts,
        tab_list_view::text(&hv, &hl, px, &width_of),
    )?;

    let in_any = |bs: &[rewo_gpu::hud::HudBlit], gx: i32, gy: i32| {
        bs.iter().any(|b| {
            (gx as f32) >= b.x
                && (gx as f32) < b.x + b.w
                && (gy as f32) >= b.y
                && (gy as f32) < b.y + b.h
        })
    };
    let (mut heart_in, mut heart_out) = (0usize, 0usize);
    for gy in 0..GUI_H {
        for gx in 0..GUI_W {
            if hframe2.gui(gx, gy) != hcontrol.gui(gx, gy) {
                if in_any(&hb, gx, gy) {
                    heart_in += 1;
                } else {
                    heart_out += 1;
                }
            }
        }
    }
    c.record(
        "h7.the_hearts_reach_the_frame_and_only_inside_their_own_rects",
        heart_in > 0
            && heart_out == 0
            // 20 health is ten hearts and 22 is eleven, each two layers; the
            // spectator's row gets none, which is `score_span == None`.
            && hb.len() == 42
            && hl.entries.get(2).is_some_and(|e| e.score_span.is_none()),
        format!(
            "{heart_in} GUI pixels changed inside the {} heart rects and \
             {heart_out} outside. Before M158 this emitter could be replaced \
             by `return Vec::new()` with the gate still green.",
            hb.len()
        ),
    );

    // h8 — the two layers composite, and the evidence is a NEGATIVE.
    //
    // `container.png` is 34 interior texels of (40,40,40) plus 20 outline
    // texels of (0,0,0); `full.png` has exactly 34 opaque texels, and they
    // cover the interior. So a correct draw shows the container's OUTLINE and
    // the fill's red and **no (40,40,40) anywhere**. Drop the container and the
    // outline goes; drop the fill and the grey appears. Counting "some red
    // arrived" cannot see either.
    let count_of = |f: &Frame, want: [u8; 3]| {
        let mut n = 0usize;
        for gy in 0..GUI_H {
            for gx in 0..GUI_W {
                let p = f.gui(gx, gy);
                if [p[0], p[1], p[2]] == want {
                    n += 1;
                }
            }
        }
        n
    };
    // **Scoped to the heart rects, and the first cut of this was not** — which
    // is why it failed: (40,40,40) is also the ping bars' own grey, so a
    // whole-frame count of the container's interior colour is measuring the
    // icons one column over. The two halves of one witness have to share a
    // scope or they are not about the same thing.
    let in_hearts_count = |want: [u8; 3]| {
        let mut n = 0usize;
        for gy in 0..GUI_H {
            for gx in 0..GUI_W {
                let p = hframe2.gui(gx, gy);
                if [p[0], p[1], p[2]] == want && in_any(&hb, gx, gy) {
                    n += 1;
                }
            }
        }
        n
    };
    let red = in_hearts_count(HEART_FULL_RED);
    let grey = in_hearts_count(HEART_CONTAINER_GREY);
    let black_in_hearts = in_hearts_count([0, 0, 0]);
    c.record(
        "h8.a_filled_heart_shows_its_containers_outline_and_hides_its_interior",
        red > 0 && black_in_hearts > 0 && grey == 0,
        format!(
            "inside the heart rects: {red} texels of the fill's own \
             {HEART_FULL_RED:?}, {black_in_hearts} of the container's outline \
             black, and {grey} of the container's interior \
             {HEART_CONTAINER_GREY:?} — the fill's 34 opaque texels cover the \
             container's 34 interior ones exactly, so a missing container \
             loses the outline and a missing fill exposes the grey"
        ),
    );

    // h9 — the absorption heart is GOLD, and the detector is the art's own.
    //
    // Read out of the eight heart PNGs: every one of them is achromatic in the
    // blue axis (max |g - b| == 0 over every opaque texel) EXCEPT the absorbing
    // pair, where it is 162. So "g differs from b" identifies an absorption
    // heart and cannot be satisfied by a red heart, a container or a blink
    // ghost. The names are magenta (g < b) and under HEARTS no score digits are
    // drawn at all, so nothing else in the frame competes.
    // The absorbing rects alone — index 10 of the 22-health row. Every
    // gold-family texel must land in one of them, which is the claim that the
    // gold is at the ELEVENTH heart rather than smeared across the column.
    let absorbing: Vec<rewo_gpu::hud::HudBlit> = hb
        .iter()
        .filter(|b| {
            matches!(
                b.icon,
                rewo_gpu::hud::HudIcon::Heart(rewo_gpu::tab_list::HeartSprite::AbsorbingFull)
            )
        })
        .copied()
        .collect();
    let mut gold = 0usize;
    let mut gold_outside_its_rect = 0usize;
    for gy in 0..GUI_H {
        for gx in 0..GUI_W {
            if !in_any(&hb, gx, gy) {
                continue;
            }
            let p = hframe2.gui(gx, gy);
            // The art's own discriminator, read out of the eight PNGs: only the
            // absorbing pair is chromatic in the blue axis. A red heart, a
            // container and a blink ghost all have g == b exactly. This admits
            // the sprite's three shades — (212,175,55), (212,162,0) and
            // (212,192,126) — rather than only its dominant one, which is what
            // the first cut got wrong: it counted the sprite's own highlights
            // as evidence AGAINST itself.
            if p[1] as i32 - p[2] as i32 > 60 {
                gold += 1;
                if !in_any(&absorbing, gx, gy) {
                    gold_outside_its_rect += 1;
                }
            }
        }
    }
    c.record(
        "h9.the_eleventh_heart_draws_in_the_absorption_sprites_own_gold",
        gold > 0
            && gold_outside_its_rect == 0
            && absorbing.len() == 1
            && in_hearts_count(HEART_ABSORB_GOLD) > 0,
        format!(
            "{gold} gold-family texels, all inside the {} absorbing rect \
             ({gold_outside_its_rect} outside). `absorbing_full.png` is the \
             only heart sprite with g != b (162 against 0 for the other \
             seven), so this cannot be a red heart read loosely — and 22 \
             health is ELEVEN hearts, so index 10 exists at all",
            absorbing.len()
        ),
    );

    // h11 — the hearts are LAID OUT across their column, at a pitch narrower
    // than the sprite so they overlap.
    //
    // **Everything above this takes its rects from `hb`, which is the emitter
    // under test** — so moving every heart to the column's left edge, or
    // shrinking the sprite, moves the draw and the expectation together and all
    // four survive. Both did, in M158's first battery. That is gotcha 0a in the
    // shape M93q found it: a witness that computes its expectation from the
    // thing it is testing covers everything about the draw except the thing
    // they share.
    //
    // So this one predicts from the LAYOUT's `score_span` — which
    // `tab_list::layout` produces and `hearts` only reads — plus the pitch and
    // sprite size **re-declared as literals**, the way `check_transcription`
    // re-declares `ref_columns`. And it detects a heart by differing from the
    // no-hearts control, so it needs no rect from the emitter at all.
    //
    // `container.png` is opaque in all nine of its columns and rows (measured
    // from the jar), so a drawn row is a CONTIGUOUS band exactly
    // `pitch * (n - 1) + sprite` wide.
    const REF_SPRITE: i32 = 9;
    let ref_pitch = |span: i32, hearts: i32| ((span - 4) as f32 / hearts as f32).min(9.0) as i32;
    // Two rows, two pitches, in one frame: TabMid's 22 health is eleven hearts
    // at pitch 7 and TabZulu's 20 is ten at pitch 8. One fixture with a single
    // pitch cannot tell a wrong pitch from a wrong sprite size.
    let mut spans = Vec::new();
    let mut gaps = 0usize;
    for (i, hp) in [(0usize, 11i32), (1usize, 10i32)] {
        // `get`, not `[]` — gotcha 15: a witness that can panic is a witness
        // that can silently not run, and a harness scraping FAIL lines cannot
        // tell an abort from a pass. `spans.len() == 2` below is what turns a
        // missing row into a failure rather than a skip.
        let Some(slot) = hl.entries.get(i) else { continue };
        let Some((left, right)) = slot.score_span else {
            continue;
        };
        let y0 = slot.name.1;
        let occupied: Vec<i32> = (left..right + REF_SPRITE)
            .filter(|gx| {
                (y0..y0 + REF_SPRITE).any(|gy| hframe2.gui(*gx, gy) != hcontrol.gui(*gx, gy))
            })
            .collect();
        let (lo, hi) = (
            occupied.first().copied().unwrap_or(-1),
            occupied.last().copied().unwrap_or(-1),
        );
        // Contiguous: a sprite wider than its pitch leaves no gap. At pitch 8
        // with a 7-px sprite every heart would be one column short of its
        // neighbour.
        if hi >= lo && occupied.len() as i32 != hi - lo + 1 {
            gaps += 1;
        }
        let pitch = ref_pitch(right - left, hp);
        spans.push((lo - left, hi - left, pitch * (hp - 1) + REF_SPRITE - 1, pitch));
    }
    c.record(
        "h11.the_hearts_span_their_column_at_a_pitch_narrower_than_the_sprite",
        gaps == 0
            && spans.len() == 2
            && spans.iter().all(|(lo, hi, want, _)| *lo == 0 && hi == want),
        format!(
            "per row, (first, last, predicted last, pitch) offsets from the \
             column's own left edge: {spans:?}, with {gaps} rows carrying a \
             gap. Predicted independently as `pitch * (n - 1) + 9 - 1` from \
             the LAYOUT's score_span; a client that ignored each heart's own \
             dx would answer 8 for both rows, and a 7-px sprite would answer \
             two short and open a gap in the pitch-8 row"
        ),
    );

    // h10 — the blink clock is carried ACROSS frames.
    //
    // `hearts`'s own doc names the failure mode — "a caller that rebuilt the map
    // each frame would never blink at all, because a fresh `HealthState` is
    // seeded with its own value and has nothing to catch up to" — and nothing
    // graded it. This drives the real emitter three times down one map, which
    // is what the frame loop does, and once down a fresh one.
    let hurt_w = hearts_scene_at(10, 22, 12);
    let hurt_v = hurt_w.resolve(true, GUI_W, Some(advance));
    let hurt_l = tab_list::layout(&hurt_v.input, &hurt_v.entries);
    let mut carried = std::collections::HashMap::new();
    tab_list_view::hearts(&hv, &hl, &mut carried, 0); // seen at 20
    tab_list_view::hearts(&hurt_v, &hurt_l, &mut carried, 3); // the hit
    let after = tab_list_view::hearts(&hurt_v, &hurt_l, &mut carried, 6);
    let mut fresh = std::collections::HashMap::new();
    let never = tab_list_view::hearts(&hurt_v, &hurt_l, &mut fresh, 6);
    let ghosts = |bs: &[rewo_gpu::hud::HudBlit]| {
        bs.iter()
            .filter(|b| {
                matches!(
                    b.icon,
                    rewo_gpu::hud::HudIcon::Heart(rewo_gpu::tab_list::HeartSprite::FullBlinking)
                )
            })
            .count()
    };
    c.record(
        "h10.the_blink_clock_is_carried_across_frames_by_the_callers_map",
        ghosts(&after) > 0 && ghosts(&never) == 0 && after.len() != never.len(),
        format!(
            "down one map 20 -> 10 leaves {} ghost blits three ticks later ({} \
             blits in all); down a fresh map the same view leaves {} ({} \
             blits), because `HealthState::new(10)` has nothing to catch up to",
            ghosts(&after),
            after.len(),
            ghosts(&never),
            never.len()
        ),
    );

    // -- the faces, in pixels -------------------------------------------------
    //
    // The strip is four saturated quadrants, so three separate claims are
    // separable in one readback: WHICH HALF was sampled (head 0..8 vs hat
    // 8..16), which way UP (the flip inverts the source v and not the
    // destination), and that a face reached the frame at all.
    const HEAD_TOP: [u8; 3] = [0, 255, 0];
    const HEAD_BOTTOM: [u8; 3] = [0, 0, 255];
    const HAT_TOP: [u8; 3] = [255, 128, 0];
    const HAT_BOTTOM: [u8; 3] = [0, 255, 255];

    let face_frame = |wrr: &mut WorldRenderer,
                      g: &mut Gpu,
                      o: &mut Offscreen,
                      hat: bool,
                      flip: bool|
     -> Result<(Frame, Vec<rewo_gpu::hud::HudBlit>), String> {
        let fb = tab_list_view::faces(&hv, &hl, &|_| face_slot, &|_| hat, &|_| flip);
        let mut all = tab_list_view::icons(&hv, &hl);
        all.extend(fb.clone());
        let f = shot(
            g,
            o,
            wrr,
            tab_list_view::fills(&hl),
            all,
            tab_list_view::text(&hv, &hl, px, &width_of),
        )?;
        Ok((f, fb))
    };
    // `loaded_of` is the flip's gate, so a `true` there is a Dinnerbone in
    // render distance; these rows are not named Dinnerbone, so the flip only
    // fires in the frame built for `f3`.
    let (bare_f, bare_b) = face_frame(&mut wr, &mut gpu, &mut off, false, false)?;
    let head_seen = count_of(&bare_f, HEAD_TOP) + count_of(&bare_f, HEAD_BOTTOM);
    let hat_seen = count_of(&bare_f, HAT_TOP) + count_of(&bare_f, HAT_BOTTOM);
    let (mut face_in, mut face_out) = (0usize, 0usize);
    for gy in 0..GUI_H {
        for gx in 0..GUI_W {
            let p = bare_f.gui(gx, gy);
            if [p[0], p[1], p[2]] == HEAD_TOP || [p[0], p[1], p[2]] == HEAD_BOTTOM {
                if in_any(&bare_b, gx, gy) {
                    face_in += 1;
                } else {
                    face_out += 1;
                }
            }
        }
    }
    // **The rect is compared against the LAYOUT's, not the emitter's own.**
    // `f1` used to test containment inside `bare_b`, which `faces` produces —
    // so a blit displaced from the rect the layout placed moved the draw and
    // the expectation together and survived, the same hole `h11` closed for the
    // hearts. `hl.entries[i].face` is `tab_list::layout`'s output and `faces`
    // only reads it.
    let placed: Vec<rewo_gpu::tab_list::Rect> = hl.entries.iter().filter_map(|e| e.face).collect();
    let rects_are_the_layouts = bare_b.len() == placed.len()
        && bare_b.iter().zip(placed.iter()).all(|(b, r)| {
            (b.x, b.y, b.w, b.h) == (r.x as f32, r.y as f32, r.w as f32, r.h as f32)
        });
    c.record(
        "f1.a_face_draws_from_the_head_half_of_the_strip_into_the_layouts_rect",
        head_seen > 0
            && hat_seen == 0
            && face_out == 0
            && bare_b.len() == 3
            && rects_are_the_layouts,
        format!(
            "{head_seen} texels of the head's two colours, {hat_seen} of the \
             hat's, all {face_in} inside the {} face rects and {face_out} \
             outside — the head is source u 0..8 and a crop from the wrong \
             half would land the hat's colours here. The rects are the \
             LAYOUT's own ({} of them), which is what stops a displaced blit \
             moving the draw and the expectation together.",
            bare_b.len(),
            placed.len()
        ),
    );

    // f2 — the hat is a SECOND blit over the first, never a different sprite.
    // Two blits per player at the SAME rect, so the hat hides the head
    // completely — which is what makes "over the first" observable.
    let (hat_f, hat_b) = face_frame(&mut wr, &mut gpu, &mut off, true, false)?;
    let hat_now = count_of(&hat_f, HAT_TOP) + count_of(&hat_f, HAT_BOTTOM);
    let head_now = count_of(&hat_f, HEAD_TOP) + count_of(&hat_f, HEAD_BOTTOM);
    let same_rects = hat_b.len() == 6
        && hat_b
            .chunks(2)
            // `p.len() == 2` first: `chunks` hands back a short final chunk on
            // an odd length, and indexing it would abort the whole gate rather
            // than fail this witness (gotcha 15).
            .all(|p| {
                p.len() == 2 && (p[0].x, p[0].y, p[0].w, p[0].h) == (p[1].x, p[1].y, p[1].w, p[1].h)
            });
    c.record(
        "f2.the_hat_is_a_second_blit_over_the_first_at_the_same_rect",
        same_rects && hat_now > 0 && head_now == 0 && hat_b.len() == bare_b.len() * 2,
        format!(
            "{} blits for three hatted players against {} bare, each pair \
             sharing a rect; the frame carries {hat_now} hat texels and \
             {head_now} head ones, so the second blit COVERS the first rather \
             than sitting beside it",
            hat_b.len(),
            bare_b.len()
        ),
    );

    // f3 — the flip inverts the SOURCE v and leaves the destination alone.
    //
    // Driven from a scene whose player really IS called Dinnerbone, and with
    // `loaded_of` toggled rather than the name: the flip is
    // `loaded_of(uuid) && is_upside_down_name(name)`, so BOTH orderings of
    // "which input is held constant" have to be available or the witness cannot
    // separate the two conjuncts. The unflipped frame here is the same player
    // out of render distance, which is vanilla's own "on the list the right way
    // up" case.
    let dw = dinnerbone_scene();
    let dv = dw.resolve(true, GUI_W, Some(advance));
    let dl = tab_list::layout(&dv.input, &dv.entries);
    let d_face = |wrr: &mut WorldRenderer,
                  g: &mut Gpu,
                  o: &mut Offscreen,
                  loaded: bool|
     -> Result<(Frame, Vec<rewo_gpu::hud::HudBlit>), String> {
        let fb = tab_list_view::faces(&dv, &dl, &|_| face_slot, &|_| false, &|_| loaded);
        let mut all = tab_list_view::icons(&dv, &dl);
        all.extend(fb.clone());
        let f = shot(
            g,
            o,
            wrr,
            tab_list_view::fills(&dl),
            all,
            tab_list_view::text(&dv, &dl, px, &width_of),
        )?;
        Ok((f, fb))
    };
    let (up_f, up_b) = d_face(&mut wr, &mut gpu, &mut off, false)?;
    let (flip_f, flip_b) = d_face(&mut wr, &mut gpu, &mut off, true)?;
    let rects_unmoved = !up_b.is_empty()
        && flip_b.len() == up_b.len()
        && flip_b
            .iter()
            .zip(up_b.iter())
            .all(|(a, b)| (a.x, a.y, a.w, a.h) == (b.x, b.y, b.w, b.h));
    let r0 = up_b.first().copied();
    let sample = |f: &Frame, dy: i32| -> [u8; 3] {
        match r0 {
            Some(r) => {
                let p = f.gui(r.x as i32 + 4, r.y as i32 + dy);
                [p[0], p[1], p[2]]
            }
            None => [0, 0, 0],
        }
    };
    c.record(
        "f3.the_flip_inverts_the_source_v_and_leaves_the_destination_alone",
        rects_unmoved
            && sample(&up_f, 1) == HEAD_TOP
            && sample(&up_f, 6) == HEAD_BOTTOM
            && sample(&flip_f, 1) == HEAD_BOTTOM
            && sample(&flip_f, 6) == HEAD_TOP,
        format!(
            "the rects are unchanged and the content mirrors: {:?} over {:?} \
             becomes {:?} over {:?}. Inverting the DESTINATION v instead moves \
             the face off its row and leaves it the right way up; and the flip \
             needs the player LOADED, so the same Dinnerbone out of render \
             distance is the upright frame here.",
            sample(&up_f, 1),
            sample(&up_f, 6),
            sample(&flip_f, 1),
            sample(&flip_f, 6)
        ),
    );

    if let Some(dir) = args.out_dir.as_ref() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{e}"))?;
        wr.set_hud(0.0, 0, 0, rewo_gpu::hud::HudGauges::default());
        wr.set_hud_fills(tab_list_view::fills(&hlayout));
        wr.set_hud_icons(tab_list_view::icons(&hview, &hlayout));
        wr.set_text(tab_list_view::text(&hview, &hlayout, px, &width_of));
        off.render(&mut gpu, Some((&mut wr, vp)), &overlay_draw, clear)?;
        off.save_png(&gpu, &dir.join("tablist.png"))?;
        println!("[tablistshot] wrote {}", dir.join("tablist.png").display());
    }

    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}

fn line_width(line: &rewo_world::chat_style::ChatLine, width_of: &dyn Fn(&str, ChatStyle) -> i32) -> i32 {
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

/// [`scene`] plus a `LIST` display objective and two scores, through the
/// production scoreboard decoders.
fn scored_scene() -> Wire {
    use rewo_net::scoreboard::{
        parse_set_display_objective, parse_set_objective, parse_set_score, DisplaySlot,
    };
    let mut w = scene();
    let ids = rewo_data::number_formats::NumberFormatTypeIds::load(
        &DataPaths::for_version("26.2")
            .expect("no config dir")
            .registries_json(),
    )
    .expect("number format ids");

    let mut body = Vec::new();
    mc_string("tabobj", &mut body);
    body.push(0); // METHOD_ADD
    nbt_string("Obj", &mut body);
    varint(0, &mut body); // RenderType.INTEGER
    body.push(0); // no number format
    w.scoreboard
        .apply_set_objective(&parse_set_objective(&body, ids).expect("set_objective"));

    // Keyed by the PROFILE name — `ScoreHolder.fromGameProfile` — not by the
    // display override, which is what a server that renames players would
    // otherwise break.
    for (owner, value) in [("TabZulu", 42), ("TabMid", 7), ("TabAlpha", 99)] {
        let mut b = Vec::new();
        mc_string(owner, &mut b);
        mc_string("tabobj", &mut b);
        varint(value, &mut b);
        b.push(0); // no display override
        b.push(0); // no number format
        w.scoreboard
            .apply_set_score(&parse_set_score(&b, ids).expect("set_score"));
    }

    let mut b = Vec::new();
    varint(DisplaySlot::List.id(), &mut b);
    mc_string("tabobj", &mut b);
    w.scoreboard
        .apply_set_display_objective(&parse_set_display_objective(&b).expect("set_display"));
    w
}

/// [`scene`] plus a **HEARTS** display objective, through the same production
/// decoders (M158).
///
/// The scores are health points rather than the arbitrary integers
/// [`scored_scene`] uses, and they are chosen to reach three different sprite
/// families in one frame: **TabMid at 22** puts an absorption heart at index 10
/// (`fullHearts` is 11, not 10), and **TabZulu at 20** is ten ordinary red
/// ones. The spectator gets no score at all, which is vanilla's rule and what
/// `h7` measures the third row against.
fn hearts_scene() -> Wire {
    hearts_scene_at(20, 22, 12)
}

/// [`hearts_scene`] with the three health values chosen by the caller, so a
/// witness that needs a health *change* can build the same scene twice.
fn hearts_scene_at(zulu: i32, mid: i32, alpha: i32) -> Wire {
    use rewo_net::scoreboard::{
        parse_set_display_objective, parse_set_objective, parse_set_score, DisplaySlot,
    };
    let mut w = scene();
    let ids = rewo_data::number_formats::NumberFormatTypeIds::load(
        &DataPaths::for_version("26.2")
            .expect("no config dir")
            .registries_json(),
    )
    .expect("number format ids");

    let mut body = Vec::new();
    mc_string("hpobj", &mut body);
    body.push(0); // METHOD_ADD
    nbt_string("Health", &mut body);
    varint(1, &mut body); // RenderType.HEARTS — ordinal 1, not 0
    body.push(0); // no number format
    w.scoreboard
        .apply_set_objective(&parse_set_objective(&body, ids).expect("set_objective"));

    for (owner, value) in [("TabZulu", zulu), ("TabMid", mid), ("TabAlpha", alpha)] {
        let mut b = Vec::new();
        mc_string(owner, &mut b);
        mc_string("hpobj", &mut b);
        varint(value, &mut b);
        b.push(0); // no display override
        b.push(0); // no number format
        w.scoreboard
            .apply_set_score(&parse_set_score(&b, ids).expect("set_score"));
    }

    let mut b = Vec::new();
    varint(DisplaySlot::List.id(), &mut b);
    mc_string("hpobj", &mut b);
    w.scoreboard
        .apply_set_display_objective(&parse_set_display_objective(&b).expect("set_display"));
    w
}

/// One listed player, actually named **Dinnerbone** (M158).
///
/// `faces` gates the flip on `loaded_of(uuid) && is_upside_down_name(&e.name)`,
/// so a `loaded_of` that answers `true` for an ordinarily-named player flips
/// nothing. The first cut of `f3` did exactly that and reported the flipped
/// frame as identical to the unflipped one — a witness that could not fire,
/// which is this project's most repeated failure and the fourth documented
/// instance of it.
fn dinnerbone_scene() -> Wire {
    let mut w = Wire {
        players: rewo_net::play::TabListPlayers::default(),
        names: std::collections::HashMap::new(),
        pings: std::collections::HashMap::new(),
        spectators: std::collections::HashSet::new(),
        text: rewo_net::tab_list_text::TabListText::new(),
        scoreboard: rewo_net::scoreboard::Scoreboard::new(),
    };
    let ok = w.apply(&player_info_body(&[WirePlayer {
        uuid: 7,
        name: "Dinnerbone",
        gamemode: 0,
        listed: true,
        latency: 10,
        display: Some(format!("{MAGENTA_CODE}Ddd")),
    }]));
    assert!(ok, "tablistshot: the Dinnerbone body must decode");
    w
}

/// A synthetic 16x8 face strip: the 8x8 head beside the 8x8 hat, which is the
/// layout [`rewo_gpu::world::WorldRenderer::upload_tab_face`] documents.
///
/// **Every quadrant is a different, saturated colour**, because three separate
/// claims have to be told apart in one readback: which HALF was sampled (head
/// vs hat), which way up it was sampled (the flip inverts the source v and not
/// the destination), and that a face reached the frame at all. A single-colour
/// strip witnesses only the last of those, and this project's recurring
/// detector error is exactly a fixture that cannot express its own claim.
fn face_strip() -> Vec<u8> {
    // head top / head bottom / hat top / hat bottom.
    const QUADRANT: [[u8; 3]; 4] = [
        [0, 255, 0],   // head, top    — pure green
        [0, 0, 255],   // head, bottom — pure blue
        [255, 128, 0], // hat,  top    — orange
        [0, 255, 255],  // hat,  bottom — cyan
    ];
    let mut out = Vec::with_capacity(16 * 8 * 4);
    for y in 0..8u32 {
        for x in 0..16u32 {
            let q = usize::from(x >= 8) * 2 + usize::from(y >= 4);
            let c = QUADRANT[q];
            out.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    out
}
