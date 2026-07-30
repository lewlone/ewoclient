//! `rewo statshot --check` — the M84 `award_stats` + statistics-screen oracle.
//!
//! `award_stats` (3) is `REWO_PACKET_COVERAGE.md` class **B** and the last but
//! one screen in it. What this gate grades is three things at once: the
//! packet's two-level dispatch, the statistics screen's layout, and the two
//! framework pieces the second screen forced into `rewo_world::screen` — a
//! scrolling list and a tab bar.
//!
//! The path under test, end to end:
//!
//! ```text
//! a raw award_stats body (built here)
//!   -> rewo_net::route_award_stats          (with a REAL `Ids::resolve`d table)
//!   -> rewo_world::stats::StatsCounter      (setValue, never increment)
//!   -> rewo_world::stats_screen::StatsModel (the rows, the sort, the layout)
//!   -> rewo_world::screen::{Screen, ScrollList}
//!   -> stats_view::{chrome, lines}          (the SAME builders `LiveApp::frame` calls)
//!   -> WorldRenderer::{set_screen, set_text} -> ScreenPass / TextPass
//!   -> Offscreen::read_rgba                 (real pixels)
//! ```
//!
//! ## The rules this gate is built around
//!
//! **The gate drives the real emitter.** M45's `install_shapes` failure and
//! M41's rotted `swingshot` fixture were both gates that had quietly stopped
//! testing their subject, so `stats_view::chrome` and `stats_view::lines` are
//! the ones the frame path calls, and the router runs against a real `Ids`.
//!
//! **The detector must not share a colour with its background.** Fifteen
//! detector errors on this project are one shape. The statistics screen makes
//! it worse than the death screen did, because it paints two *translucent*
//! tiled sheets over the whole frame — so "is it darker" is not a measurement
//! here at all, and neither is "is it covered" (`p2` first asserted exactly
//! that and was wrong; see its comment). Every pixel witness below is either a
//! **difference between two frames that differ in exactly one input**, or a
//! comparison against the pure-green empty frame `p1` proves is uniform.
//!
//! **Sample on the boundary.** A scroll clamp, a row-hit edge and a unit
//! switch in `StatFormatter` are all the same shape, so each is sampled at the
//! boundary *and* one either side.
//!
//! **Predict from independent literals.** Every expectation in the model half
//! is written out from the decompile rather than read back from the code under
//! test — `healthbarshot`'s rule, and the shape M82's own `p2` was caught in.
//!
//! **Fail-closed** on a fixed [`EXPECTED_WITNESSES`] count.

use clap::Args as ClapArgs;

use rewo_data::assets;
use rewo_data::packets::Packets;
use rewo_data::stats::{StatRegistries, ValueRegistry};
use rewo_data::DataPaths;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::{SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_net::ids::Ids;
use rewo_proto::writer::PacketWriter;
use rewo_world::screen::{ScrollList, Sprite, WidgetSprites};
use rewo_world::stats::{format_stat, StatKey, StatsCounter};
use rewo_world::stats_screen::{self as ss, StatsLabels, StatsTab};

use crate::stats_view::StatsView;

/// Total named properties this gate asserts. Locked so a skipped property
/// fails the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 47;

const W: u32 = 640;
const H: u32 = 480;
/// `gui_scale(640, 480)` = 2, asserted by `p0` rather than assumed.
const SCALE: i32 = 2;
const GUI_W: i32 = W as i32 / SCALE;
const GUI_H: i32 = H as i32 / SCALE;

#[derive(ClapArgs)]
pub struct StatshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally, so
    /// this only labels the run.
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
            "[statshot] {}  {name}: {detail}",
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

pub fn run(args: StatshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[statshot] mode: {mode} (serverless; the oracle asserts \
         unconditionally). Model half against the 26.2 decompile; pixel half \
         by frame difference over a pure-green clear."
    );

    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;
    let reg = StatRegistries::load(&paths.registries_json())?;
    let items = rewo_data::items::Items::load(&paths.registries_json())?;
    let types = rewo_data::entity_types::EntityTypes::load(&paths.registries_json())?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_wire(&mut c, &ids, &reg);
    check_model(&mut c, &reg, &items, &types, &baked);
    check_pixels(&mut c, &args, &reg, &items, &types, &baked)?;

    println!(
        "[statshot] witnesses observed: {} / {}",
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
    println!("[statshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ---------------------------------------------------------------------------
// The wire.
// ---------------------------------------------------------------------------

/// `ByteBufCodecs.map(…, Stat.STREAM_CODEC, VAR_INT)` — a count then
/// `(statType, value, amount)` triples. Built here so the encoder and the
/// decoder cannot agree by construction.
fn body(entries: &[(i32, i32, i32)]) -> Vec<u8> {
    let mut w = PacketWriter::default();
    w.varint(entries.len() as i32);
    for (t, v, a) in entries {
        w.varint(*t);
        w.varint(*v);
        w.varint(*a);
    }
    w.buf
}

fn check_wire(c: &mut Checker, ids: &Ids, reg: &StatRegistries) {
    c.record(
        "w1.the_packet_resolves_by_name_from_the_datagen_report",
        ids.cb_play_award_stats == 3,
        format!(
            "award_stats -> {} (informational; the resolution is by name, so a \
             renumber moves this and still works)",
            ids.cb_play_award_stats
        ),
    );

    // w2: the whole production path with **no entity table anywhere**.
    //
    // The gotcha-13 witness, in its benign form. `handleAwardStats` writes
    // straight into `minecraft.player.getStats()` and the body carries no
    // entity id at all, so a table-based lookup could not even be written —
    // the packet that cannot be got wrong the way M81's `hurt_animation` was.
    let mut out = None;
    let handled = rewo_net::route_award_stats(
        ids.cb_play_award_stats,
        &body(&[(8, 1, 1200), (6, 54, 3)]),
        ids,
        &mut out,
    );
    c.record(
        "w2.the_map_decodes_through_the_real_router_with_no_entity_table",
        handled
            && out.as_deref() == Some(&[(StatKey::new(8, 1), 1200), (StatKey::new(6, 54), 3)][..]),
        format!("handled={handled} pairs={:?}", out.as_ref().map(|v| v.len())),
    );

    // w3: the milestone's central finding, and w2's mutation partner.
    //
    // Every `StatType`'s second level is `ByteBufCodecs.registry(...)` — one
    // VarInt — so an unknown *first* level cannot desync the walk. The witness
    // is the entry **after** it. A `DataComponentPatch`-shaped dispatch would
    // park mid-value here and the second pair would be garbage.
    let mut mixed = None;
    rewo_net::route_award_stats(
        ids.cb_play_award_stats,
        &body(&[(9999, 7, 1), (8, 2, 5)]),
        ids,
        &mut mixed,
    );
    c.record(
        "w3.an_unknown_stat_type_leaves_the_walk_in_step",
        mixed.as_deref() == Some(&[(StatKey::new(9999, 7), 1), (StatKey::new(8, 2), 5)][..]),
        "the *second* pair is the witness — the dispatch is uniform because \
         all nine `StatType.streamCodec`s come from one constructor",
    );

    let mut wrong = None;
    let handled_wrong = rewo_net::route_award_stats(
        ids.cb_play_award_stats + 1000,
        &body(&[(8, 0, 1)]),
        ids,
        &mut wrong,
    );
    c.record(
        "w4.another_packets_id_is_not_handled_here",
        !handled_wrong && wrong.is_none(),
        "the router matches one id and returns false for everything else",
    );

    let full = body(&[(8, 1, 1200), (8, 2, 42)]);
    let mut trunc = None;
    let handled_trunc = rewo_net::route_award_stats(
        ids.cb_play_award_stats,
        &full[..full.len() - 1],
        ids,
        &mut trunc,
    );
    c.record(
        "w5.a_short_read_keeps_what_decoded_rather_than_dropping_the_packet",
        handled_trunc && trunc.as_deref() == Some(&[(StatKey::new(8, 1), 1200)][..]),
        "a truncated statistics list is worth more than none, and the walk \
         cannot recover a partial entry",
    );

    c.record(
        "w6.the_statistics_request_is_ordinal_one_and_not_a_respawn",
        rewo_net::client_command_body(rewo_net::ClientCommand::RequestStats) == vec![1u8]
            && rewo_net::client_command_body(rewo_net::ClientCommand::PerformRespawn) == vec![0u8],
        "`writeEnum` is the ordinal as a VarInt; asking for ordinal 0 instead \
         respawns you and fetches nothing",
    );

    // w7: the registry that makes the second level resolvable. Expected ids
    // written out rather than read back.
    let nine = [
        ("minecraft:mined", 0, ValueRegistry::Block),
        ("minecraft:crafted", 1, ValueRegistry::Item),
        ("minecraft:used", 2, ValueRegistry::Item),
        ("minecraft:broken", 3, ValueRegistry::Item),
        ("minecraft:picked_up", 4, ValueRegistry::Item),
        ("minecraft:dropped", 5, ValueRegistry::Item),
        ("minecraft:killed", 6, ValueRegistry::EntityType),
        ("minecraft:killed_by", 7, ValueRegistry::EntityType),
        ("minecraft:custom", 8, ValueRegistry::CustomStat),
    ];
    let ok = nine.iter().all(|(name, id, vr)| {
        reg.stat_type.id_of(name) == Some(*id) && reg.value_registry_of(*id) == Some(*vr)
    }) && reg.stat_type.len() == 9;
    c.record(
        "w7.the_nine_stat_types_resolve_to_their_own_value_registries",
        ok,
        format!(
            "{} stat types; five share `minecraft:item` and two share \
             `minecraft:entity_type`, so the *type* is not the registry",
            reg.stat_type.len()
        ),
    );

    c.record(
        "w8.the_custom_stat_registry_is_the_second_levels_own_table",
        reg.custom_stat.len() == 77
            && reg.custom_stat.id_of("minecraft:play_time") == Some(1)
            && reg.custom_stat.name(0) == Some("minecraft:leave_game"),
        format!(
            "{} custom stats; read by `protocol_id` and never by iteration \
             order, which serde_json sorts (M64's trap)",
            reg.custom_stat.len()
        ),
    );

    // w9: the counter's own semantics.
    //
    // **The same key twice, and that is the whole witness.** The first version
    // applied two *different* keys, so `increment` and `setValue` agreed on
    // every value and a counter that accumulated survived the battery. The
    // second `apply` here must overwrite 5 with 3, not reach 8.
    let mut counter = StatsCounter::default();
    counter.apply(&[(StatKey::new(8, 1), 5), (StatKey::new(8, 2), 9)]);
    counter.apply(&[(StatKey::new(8, 1), 3)]);
    c.record(
        "w9.the_counter_replaces_and_never_clears",
        counter.value(StatKey::new(8, 1)) == 3
            && counter.value(StatKey::new(8, 2)) == 9
            && counter.value(StatKey::new(8, 3)) == 0
            && counter.updates == 2,
        "`setValue`, not `increment`; the map is not cleared between packets, \
         and an absent stat reads as a zero rather than as missing",
    );
}

// ---------------------------------------------------------------------------
// The model. Every expectation is an independent decompile literal.
// ---------------------------------------------------------------------------

fn check_model(
    c: &mut Checker,
    reg: &StatRegistries,
    items: &rewo_data::items::Items,
    types: &rewo_data::entity_types::EntityTypes,
    baked: &assets::BakedAssets,
) {
    // m1: the two layout heights, and only one of them is the constant its
    // name suggests.
    c.record(
        "m1.the_header_is_the_tab_bars_height_and_the_footer_is_the_constant",
        ss::HEADER_HEIGHT == 24
            && ss::FOOTER_HEIGHT == 33
            && ss::content_band(GUI_H) == (24, GUI_H - 57),
        format!(
            "content band {:?} at {GUI_H} GUI px — `repositionElements` \
             overwrites the header with `tabNavigationBar.getRectangle().bottom()`",
            ss::content_band(GUI_H)
        ),
    );

    // m2: `Mth.roundToward(v, 2)` is ceil-to-even, not round-to-nearest.
    // 320 GUI px: tabsWidth = min(400,320) - 28 = 292; 292/3 = 97 -> 98;
    // x = roundToward((320-292)/2, 2) = roundToward(14, 2) = 14.
    let (tx, tw) = ss::tab_bar_layout(GUI_W, 3);
    // The second sample is where the mutation partner lives. For
    // `multiple == 2`, ceil-to-multiple and round-to-nearest-multiple are the
    // **same function** on every integer, so a mutation between them is an
    // equivalent mutant and the battery proved it. What a *truncating* divide
    // would break is a band that divides odd: at 260 GUI px the band is 232,
    // `232 / 3 = 77`, and ceil gives 78 where a floor gives 76.
    let (_, narrow) = ss::tab_bar_layout(260, 3);
    c.record(
        "m2.the_tab_bar_rounds_up_and_the_three_tabs_overrun_their_band",
        (tx, tw) == (14, 98) && tw * 3 > 292 && narrow == 78,
        format!(
            "x={tx} w={tw}; 3 x 98 = 294 against a 292-wide band. At 260 GUI px \
             the tab is {narrow}, where a truncating divide gives 76"
        ),
    );

    // m3: `alignInDimension` truncates a float lerp.
    // x = (int)(0.5 * (320 - 200)) = 60 ; y = 240 - 33 + (int)(0.5 * 13) = 213
    c.record(
        "m3.the_done_button_is_centred_by_a_truncating_lerp",
        ss::done_bounds(GUI_W, GUI_H) == (60, GUI_H - 33 + 6),
        format!(
            "{:?} — `(int) Mth.lerp(0.5f, 0, 13)` is 6, and rounding would put \
             it a pixel low",
            ss::done_bounds(GUI_W, GUI_H)
        ),
    );

    // m4..m7: `StatFormatter`, each ladder sampled ON its boundary.
    use rewo_data::stats::Formatter;
    c.record(
        "m4.distance_switches_units_on_a_strict_half_and_ends_in_a_bare_int",
        format_stat(Formatter::Distance, 50) == "50 cm"
            && format_stat(Formatter::Distance, 51) == "0.51 m"
            && format_stat(Formatter::Distance, 50_000) == "500.00 m"
            && format_stat(Formatter::Distance, 50_001) == "0.50 km",
        "`meters > 0.5`, not `>=`; and the last branch is `cm + \" cm\"` on an \
         **int**, so it neither groups nor takes two decimals",
    );
    c.record(
        "m5.the_seconds_branch_is_java_double_tostring",
        format_stat(Formatter::Time, 0) == "0.0 s"
            && format_stat(Formatter::Time, 20) == "1.0 s"
            && format_stat(Formatter::Time, 1) == "0.05 s"
            && format_stat(Formatter::Time, 600) == "30.0 s"
            && format_stat(Formatter::Time, 601) == "0.50 min",
        "`seconds + \" s\"` on a **double**, so 20 ticks is `1.0 s` and never \
         `1 s`; the rung above it is `DECIMAL_FORMAT` and always two decimals",
    );
    c.record(
        "m6.the_default_formatter_groups_where_the_decimal_one_does_not",
        format_stat(Formatter::Default, 12345) == "12,345"
            && format_stat(Formatter::DivideByTen, 123456) == "12345.60",
        "`getIntegerInstance(Locale.US)` groups; `\"########0.00\"` has no `,` \
         in its pattern, and the two sit in the same list",
    );
    c.record(
        "m7.the_formatter_is_per_stat_and_sneak_time_is_the_odd_name",
        rewo_data::stats::custom_formatter("minecraft:play_time") == Formatter::Time
            && rewo_data::stats::custom_formatter("minecraft:sneak_time") == Formatter::Time
            && rewo_data::stats::custom_formatter("minecraft:crouch_time") == Formatter::Default
            && rewo_data::stats::custom_formatter("minecraft:walk_one_cm") == Formatter::Distance
            && rewo_data::stats::custom_formatter("minecraft:damage_dealt")
                == Formatter::DivideByTen
            && rewo_data::stats::custom_formatter("minecraft:jump") == Formatter::Default,
        "`CROUCH_TIME = makeCustomStat(\"sneak_time\", TIME)` — reading the Java \
         constant names a stat that does not exist and formats the real one as \
         raw ticks",
    );

    // m8: the items tab's column order is not the registry's.
    let registry_order: Vec<&str> = (0..6).filter_map(|i| reg.stat_type.name(i)).collect();
    c.record(
        "m8.the_columns_are_not_in_registry_order",
        ss::COLUMNS[1] == "minecraft:broken"
            && registry_order[1] == "minecraft:crafted"
            && ss::column_x(0) == 75
            && ss::column_x(5) == 275,
        format!(
            "columns {:?} against registry {:?}",
            &ss::COLUMNS[..3],
            &registry_order[..3]
        ),
    );

    // m9: `WidgetSprites`' two call sites disagree about the same slot.
    let tab = WidgetSprites::four(
        Sprite::TabSelected,
        Sprite::Tab,
        Sprite::TabSelectedHighlighted,
        Sprite::TabHighlighted,
    );
    let sort = WidgetSprites::two(Sprite::StatHeader, Sprite::Slot);
    c.record(
        "m9.the_same_record_puts_a_highlight_in_disabled_focused_for_a_tab",
        tab.get(false, true) == Sprite::TabHighlighted
            && tab.get(true, false) == Sprite::TabSelected
            && sort.get(true, true) == Sprite::Slot
            && sort.get(false, false) == Sprite::StatHeader,
        "the death screen's three-argument form maps `disabledFocused` back \
         onto `disabled`; the tab's four-argument form does not, and the sort \
         button's two-argument form makes the *hover* the plainer sheet",
    );

    // m10..m14: the scroll model.
    let mut list = ScrollList::new(320, 100, 24, 14, 280);
    list.rows = vec![14; 20];
    c.record(
        "m10.the_scroll_rate_is_half_the_row_height_truncated",
        list.scroll_rate == 7
            && ScrollList::new(320, 100, 24, 22, 280).scroll_rate == 11
            && ScrollList::new(320, 100, 24, 9, 280).scroll_rate == 4,
        "`AbstractScrollArea.defaultSettings(defaultEntryHeight / 2)`, an \
         integer division — 9/2 is 4, not 4.5",
    );
    let max = list.max_scroll();
    list.set_scroll(max as f64 + 1.0);
    let clamped = list.scroll();
    list.set_scroll(7.9);
    let truncated = list.row_top(0);
    c.record(
        "m11.the_scroll_clamps_at_max_and_the_row_position_truncates",
        max == 20 * 14 + 4 - 100 && clamped == max as f64 && truncated == 24 + 2 - 7,
        format!(
            "max {max} (rows + 4 - height); one past the end is the end; \
             `(int) 7.9` is 7, so row 0's top is {truncated}"
        ),
    );
    list.set_scroll(0.0);
    c.record(
        "m12.the_row_hit_rect_is_half_open_and_the_list_gates_it",
        list.row_at(20.0, 26.0) == Some(0)
            && list.row_at(19.0, 26.0).is_none()
            && list.row_at(299.0, 26.0) == Some(0)
            && list.row_at(300.0, 26.0).is_none()
            && list.row_at(160.0, 40.0) == Some(1),
        "`getRowLeft() = width/2 - rowWidth/2` = 20, and `containsPoint` is \
         half-open on the far edges like every other hit rect",
    );
    let mut scrolled = list.clone();
    scrolled.set_scroll(20.0);
    c.record(
        "m13.a_row_scrolled_above_the_list_is_still_its_own_rect_but_not_hovered",
        scrolled.row_at_unclipped(160.0, 10.0) == Some(0)
            && scrolled.row_at(160.0, 10.0).is_none(),
        "`extractWidgetRenderState` gates `getEntryAtPosition` on the list's \
         own `isMouseOver`; without the gate a scrolled-off row still hits",
    );
    c.record(
        "m14.the_scrollbar_sits_beside_the_rows_and_not_at_the_edge",
        list.scroll_bar_x() == 300 + 6 + 2 && list.scroll_bar_x() != list.width - 6,
        format!(
            "{} — `AbstractSelectionList.scrollBarX()` overrides \
             `AbstractScrollArea`'s `getRight() - scrollbarWidth()`",
            list.scroll_bar_x()
        ),
    );

    // m15: the general list, against the real jar's language map.
    let lang = &baked.lang;
    let mut counter = StatsCounter::default();
    let custom = reg.stat_type.id_of("minecraft:custom").unwrap();
    let jump = reg.custom_stat.id_of("minecraft:jump").unwrap();
    let play_time = reg.custom_stat.id_of("minecraft:play_time").unwrap();
    let deaths = reg.custom_stat.id_of("minecraft:deaths").unwrap();
    counter.apply(&[
        (StatKey::new(custom, jump), 12),
        (StatKey::new(custom, play_time), 40),
        (StatKey::new(custom, deaths), 3),
    ]);
    let general = ss::build_general(&counter, reg, lang);
    let labels: Vec<&str> = general.iter().map(|r| r.label.as_str()).collect();
    let sorted = labels.windows(2).all(|w| w[0] <= w[1]);
    let time_row = general.iter().find(|r| r.label == "Time Played");
    c.record(
        "m15.the_general_list_holds_every_custom_stat_sorted_by_translated_name",
        general.len() == 77 && sorted && time_row.map(|r| r.value.as_str()) == Some("2.0 s"),
        format!(
            "{} rows, sorted={sorted}, `Time Played` = {:?} for 40 ticks — the \
             sort key is `I18n.get(...)`, so it is not registry order",
            general.len(),
            time_row.map(|r| &r.value)
        ),
    );

    // m16/m17: the mob rows' two templates, against the jar's own strings.
    let (killed, killed_by) = (
        reg.stat_type.id_of("minecraft:killed").unwrap(),
        reg.stat_type.id_of("minecraft:killed_by").unwrap(),
    );
    let zombie = types.id_of("minecraft:zombie").unwrap();
    let creeper = types.id_of("minecraft:creeper").unwrap();
    let mut mob_counter = StatsCounter::default();
    mob_counter.apply(&[
        (StatKey::new(killed, zombie), 3),
        (StatKey::new(killed_by, zombie), 2),
        (StatKey::new(killed_by, creeper), 1),
    ]);
    let mobs = ss::build_mobs(&mob_counter, reg, types, lang);
    let z = mobs.iter().find(|m| m.name == "Zombie");
    let cr = mobs.iter().find(|m| m.name == "Creeper");
    c.record(
        "m16.the_two_mob_templates_take_their_arguments_in_opposite_orders",
        mobs.len() == 2
            && z.map(|m| m.kills.as_str()) == Some("You killed 3 Zombie")
            && z.map(|m| m.killed_by.as_str()) == Some("Zombie killed you 2 time(s)"),
        format!(
            "killed={:?} killed_by={:?} — `translatable(key, kills, name)` \
             against `translatable(key, name, killedBy)`",
            z.map(|m| &m.kills),
            z.map(|m| &m.killed_by)
        ),
    );
    c.record(
        "m17.a_mob_with_one_zero_side_takes_the_none_template_and_a_third_colour",
        cr.map(|m| m.kills.as_str()) == Some("You have never killed Creeper")
            && cr.map(|m| !m.has_kills && m.was_killed_by) == Some(true)
            && ss::ROW_GREY != ss::ROW_DIM,
        "`-8355712` for a zero line against `-4539718` for a live one — two \
         greys, and taking one for both loses the distinction entirely",
    );

    // m18/m19: the items rows and the sort cycle.
    let stone_item = items.id("minecraft:stone");
    let stone_block = reg.block.id_of("minecraft:stone");
    let dirt_item = items.id("minecraft:dirt");
    let mut item_counter = StatsCounter::default();
    let mined = reg.stat_type.id_of("minecraft:mined").unwrap();
    let used = reg.stat_type.id_of("minecraft:used").unwrap();
    if let (Some(si), Some(sb), Some(di)) = (stone_item, stone_block, dirt_item) {
        item_counter.apply(&[
            (StatKey::new(mined, sb), 40),
            (StatKey::new(used, si), 7),
            (StatKey::new(used, di), 7),
        ]);
    }
    let rows = ss::build_items(&item_counter, reg, items);
    let stone = rows.iter().find(|r| r.name == "minecraft:stone");
    c.record(
        "m18.an_items_row_joins_a_blocks_mined_count_onto_the_same_name",
        rows.len() == 2
            && stone.map(|r| r.counts[0]) == Some(Some(40))
            && stone.map(|r| r.counts[3]) == Some(Some(7)),
        format!(
            "{} rows; stone's mined column is {:?} and its used column {:?} — \
             one row, both registries",
            rows.len(),
            stone.map(|r| r.counts[0]),
            stone.map(|r| r.counts[3])
        ),
    );

    let mut model = ss::StatsModel {
        items: rows,
        ..Default::default()
    };
    model.sort_by_column(3);
    let first_desc = (model.sort_column, model.sort_order);
    model.sort_by_column(3);
    let second = (model.sort_column, model.sort_order);
    model.sort_by_column(3);
    let third = (model.sort_column, model.sort_order);
    c.record(
        "m19.the_first_click_on_a_column_sorts_descending_and_the_third_clears",
        first_desc == (Some(3), -1) && second == (Some(3), 1) && third == (None, 0),
        format!("{first_desc:?} -> {second:?} -> {third:?}"),
    );

    // m20: the language keys, against the real jar.
    c.record(
        "m20.every_lang_key_the_screen_needs_resolves_against_the_real_jar",
        lang.get(ss::KEY_TITLE) == Some("Statistics")
            && lang.get(ss::KEY_GENERAL) == Some("General")
            && lang.get(ss::KEY_ITEMS) == Some("Items")
            && lang.get(ss::KEY_MOBS) == Some("Mobs")
            && lang.get(ss::KEY_PENDING) == Some("Retrieving statistics...")
            && lang.get(ss::KEY_NONE) == Some("-")
            && lang.get(ss::KEY_NONE_FOUND) == Some("No statistics found.")
            && lang.get(ss::KEY_DONE) == Some("Done"),
        "resolved through `rewo_data::lang`, which applies `deprecated.json` — \
         a rename lands here rather than rendering the raw key",
    );

    // m21..m23: the sheets this screen needs, and their nine-slice borders.
    let widgets = baked.widgets.as_ref();
    c.record(
        "m21.the_tab_sheets_are_a_hundred_and_thirty_wide_with_an_asymmetric_border",
        widgets.is_some_and(|w| w.tabs.iter().all(|t| t.w == 130 && t.h == 24))
            && rewo_gpu::screen::TAB_BORDER == [2, 2, 2, 0]
            && rewo_gpu::screen::BUTTON_BORDER == [3, 3, 3, 3],
        "130x24 drawn at 98 wide is what forced `blitNineSlicedSprite`; the \
         tab's bottom border is **0**, so a single-number border is wrong",
    );
    c.record(
        "m22.the_statistics_sheets_are_all_eighteen_square",
        widgets.is_some_and(|w| {
            w.slot.w == 18
                && w.stat_header.w == 18
                && w.stat_columns.iter().all(|s| s.w == 18 && s.h == 18)
                && w.sort_up.w == 18
                && w.sort_down.w == 18
        }),
        "no `.mcmeta` on any of them, so they are `Stretch`-scaled — and every \
         one is drawn at 18x18, where that is a 1:1 blit",
    );
    c.record(
        "m23.the_two_tiled_backgrounds_are_both_sixteen_square_on_disk",
        widgets.is_some_and(|w| {
            w.tab_header_background.w == 16
                && w.inworld_menu_background.w == 16
                && w.inworld_header_separator.w == 32
                && w.inworld_header_separator.h == 2
        }),
        "both 16x16, and the *call sites* declare 16x16 and 32x32 — the 2x \
         difference between them is in the blit, not in the files",
    );
}

// ---------------------------------------------------------------------------
// The pixels.
// ---------------------------------------------------------------------------

fn px(buf: &[u8], x: u32, y: u32) -> [u8; 3] {
    let i = ((y * W + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2]]
}

fn differs(a: &[u8], b: &[u8], x0: u32, y0: u32, x1: u32, y1: u32) -> usize {
    let mut n = 0;
    for y in y0..y1.min(H) {
        for x in x0..x1.min(W) {
            if px(a, x, y) != px(b, x, y) {
                n += 1;
            }
        }
    }
    n
}

fn check_pixels(
    c: &mut Checker,
    args: &StatshotArgs,
    reg: &StatRegistries,
    items: &rewo_data::items::Items,
    types: &rewo_data::entity_types::EntityTypes,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[statshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("statshot: Vulkan validation requested but not active".into());
    }
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let white: Vec<u8> = vec![255u8; 16 * 16 * 4];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
    wr.set_sky_mode(SkyMode::None);
    let sprites =
        crate::live_cmd::widget_sprites(baked).ok_or("widget sprites missing from the jar")?;
    wr.init_screen(&mut gpu, &sprites)?;
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
    // A pure green destination. The statistics screen covers the frame with
    // two tiled sheets, so this is not a contrast reference the way the death
    // screen's was — it is the **control** `p1` proves is uniform, so a count
    // of "still green" is a count of what the screen failed to cover.
    const CLEAR: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

    let shot_empty = |gpu: &mut Gpu,
                          off: &mut Offscreen,
                          wr: &mut WorldRenderer|
     -> Result<Vec<u8>, String> {
        wr.set_screen(rewo_gpu::screen::ScreenDraw::default());
        wr.set_text(Vec::new());
        off.render(gpu, Some((&mut *wr, vp)), &overlay_draw, CLEAR)?;
        off.read_rgba(gpu)
    };

    let empty = shot_empty(&mut gpu, &mut off, &mut wr)?;
    let all_green = (0..W * H).all(|i| {
        let p = &empty[(i * 4) as usize..];
        p[0] == 0 && p[1] == 255 && p[2] == 0
    });
    c.record(
        "p1.a_frame_with_no_screen_is_uniformly_the_clear_colour",
        all_green,
        "so every difference below is the screen and nothing else — the rule \
         after fifteen detector errors of one shape",
    );

    // A populated counter: every custom stat (a scrollable general list), two
    // mobs, and two item rows.
    let mut counter = StatsCounter::default();
    let custom = reg.stat_type.id_of("minecraft:custom").unwrap();
    let mut pairs: Vec<(StatKey, i32)> = reg
        .custom_stat
        .iter()
        .map(|(id, _)| (StatKey::new(custom, id), 7))
        .collect();
    let killed = reg.stat_type.id_of("minecraft:killed").unwrap();
    let mined = reg.stat_type.id_of("minecraft:mined").unwrap();
    let used = reg.stat_type.id_of("minecraft:used").unwrap();
    if let Some(z) = types.id_of("minecraft:zombie") {
        pairs.push((StatKey::new(killed, z), 3));
    }
    if let Some(b) = reg.block.id_of("minecraft:stone") {
        pairs.push((StatKey::new(mined, b), 40));
    }
    if let Some(i) = items.id("minecraft:dirt") {
        pairs.push((StatKey::new(used, i), 5));
    }
    counter.apply(&pairs);

    let labels = StatsLabels::resolve(&baked.lang);
    let build = |counter: &StatsCounter, tab: StatsTab, sort: (Option<usize>, i32)| {
        StatsView::build(
            counter,
            reg,
            items,
            types,
            &baked.lang,
            labels.clone(),
            tab,
            sort,
            GUI_W,
            GUI_H,
        )
    };

    // The frame path's own builders.
    macro_rules! render {
        ($view:expr, $screen:expr, $mouse:expr) => {{
            wr.set_screen(crate::stats_view::chrome(
                $view,
                $screen,
                $mouse,
                Some(&advance),
            ));
            wr.set_text(crate::stats_view::lines(
                $view,
                $screen,
                &advance,
                SCALE as f32,
            ));
            off.render(&mut gpu, Some((&mut wr, vp)), &overlay_draw, CLEAR)?;
            off.read_rgba(&mut gpu)?
        }};
    }

    let (view, screen) = build(&counter, StatsTab::General, (None, 0));
    let frame = render!(&view, &screen, None);

    // p2: the background is a **tiled texture**, not a gradient.
    //
    // The first version of this witness asserted the screen *covers* the frame,
    // on the reflex that the menu branch is the opaque one. It is not: both
    // sheets carry alpha (`inworld_menu_background` is a 64/255 grey, and
    // `tab_header_background` is transparent along its own top row), and vanilla
    // relies on the blurred panorama beneath rather than on opacity. Over a
    // green clear that left 5,424 green pixels and the witness failed —
    // correctly, because the claim was wrong.
    //
    // What actually separates the two `extractBackground` branches is that one
    // is `fillGradient(0xC0101010, 0xD0101010)` and the other is a repeat: two
    // samples **one tile period apart** must be byte-identical, which a
    // vertical alpha ramp could not manage.
    let tile = 32u32 * SCALE as u32;
    let probe_y = (ss::HEADER_HEIGHT + 8) as u32 * SCALE as u32;
    let top_sample = px(&frame, 4, probe_y);
    let next_sample = px(&frame, 4, probe_y + tile);
    let backdrop_none = crate::stats_view::chrome(&view, &screen, None, Some(&advance))
        .backdrop
        .is_none();
    c.record(
        "p2.the_body_background_repeats_rather_than_ramping_down_the_screen",
        top_sample == next_sample && backdrop_none,
        format!(
            "{top_sample:?} at y={probe_y} and {next_sample:?} one 32-px tile \
             below, and the draw carries no backdrop at all — `isInGameUi()` \
             is false, so `extractBackground` takes the menu branch and never \
             the gradient"
        ),
    );

    // p3: the header strip and the body are different sheets. Sampled at the
    // same x, one row inside each band, so nothing but the sheet differs.
    let head_y = (ss::HEADER_HEIGHT - 4) as u32 * SCALE as u32;
    let body_y = (ss::HEADER_HEIGHT + 4) as u32 * SCALE as u32;
    c.record(
        "p3.the_header_strip_and_the_body_are_painted_by_different_sheets",
        px(&frame, 8, head_y) != px(&frame, 8, body_y),
        format!(
            "header {:?} at y={head_y} against body {:?} at y={body_y} — \
             `tab_header_background` then `inworld_menu_background`",
            px(&frame, 8, head_y),
            px(&frame, 8, body_y)
        ),
    );

    // p4: the selected tab differs from an unselected one. Two frames that
    // differ in exactly one input — which tab is selected.
    let (view_items, screen_items) = build(&counter, StatsTab::Items, (None, 0));
    let items_frame = render!(&view_items, &screen_items, None);
    let (tx, tw) = ss::tab_bar_layout(GUI_W, 3);
    let tab_rect = |i: i32| {
        (
            (tx + tw * i) as u32 * SCALE as u32,
            0u32,
            (tx + tw * (i + 1)) as u32 * SCALE as u32,
            ss::HEADER_HEIGHT as u32 * SCALE as u32,
        )
    };
    let (t0x0, t0y0, t0x1, t0y1) = tab_rect(0);
    let (t1x0, _, t1x1, _) = tab_rect(1);
    // **Stop four rows short of the bottom.** The focus underline is a 1-px
    // fill at `getY() + getHeight() - 2` and it moves with the selected tab, so
    // a band that includes it changes whichever *sheet* is drawn — the battery
    // caught exactly that: a mutant marking every tab selected left this
    // witness green, because the underline had moved anyway.
    let sheet_y1 = (ss::HEADER_HEIGHT - 4) as u32 * SCALE as u32;
    let tab0_changed = differs(&frame, &items_frame, t0x0, t0y0, t0x1, sheet_y1);
    let tab1_changed = differs(&frame, &items_frame, t1x0, t0y0, t1x1, sheet_y1);
    c.record(
        "p4.selecting_a_tab_repaints_both_the_old_and_the_new_one",
        tab0_changed > 0 && tab1_changed > 0,
        format!(
            "tab 0 changed in {tab0_changed} px, tab 1 in {tab1_changed} — \
             `SPRITES.get(isSelected(), …)` swaps `tab` for `tab_selected` on \
             each"
        ),
    );

    // p5: **the nine-slice, in pixels.** The tab is a 130-wide sheet drawn at
    // 98, so its 2-px left border must survive unscaled. The control is the
    // same tab drawn at its own 130, where nine-slice degenerates to a 1:1
    // blit — a stretch would resample the border column and not match.
    // The control widens tab 0 to its sheet's own 130 and **hides the other
    // two**. Hiding them is load-bearing and the first version did not: at 130
    // wide, tab 0 spans 14..144 and tab 1 starts at 112, so tab 1 painted over
    // exactly the right border this witness reads. (At the real 98 they abut
    // at 112 and there is no overlap, so only the control needed fixing.)
    let mut wide = screen.clone();
    for w in wide.widgets.iter_mut() {
        match StatsTab::from_widget(w.id) {
            Some(StatsTab::General) => w.width = 130,
            Some(_) => w.visible = false,
            None => {}
        }
    }
    let wide_frame = render!(&view, &wide, None);
    let border = 2 * SCALE as u32;
    let col = |buf: &[u8], x: u32| -> Vec<[u8; 3]> { (t0y0..t0y1).map(|y| px(buf, x, y)).collect() };
    // **The left border alone does not discriminate, and the first version of
    // this witness assumed it did.** A stretch maps dest `x` to sheet
    // `x * 130 / 98`, which for `x < 2` still lands on sheet columns 0 and 1 —
    // the two agree there by arithmetic, so the check passed for the wrong
    // reason and its companion (an inner column that must *differ*) failed,
    // because the tiled middle starts at the same texel in both. The **right**
    // border is where the two part: nine-slice puts sheet columns 128 and 129
    // at dest 96 and 97, a stretch samples 127 and 128 into the same two.
    let narrow_right = (tx + tw) as u32 * SCALE as u32;
    let wide_right = (tx + 130) as u32 * SCALE as u32;
    let left_same = (0..border).all(|d| col(&frame, t0x0 + d) == col(&wide_frame, t0x0 + d));
    let right_same =
        (1..=border).all(|d| col(&frame, narrow_right - d) == col(&wide_frame, wide_right - d));
    c.record(
        "p5.a_tab_drawn_narrower_than_its_sheet_keeps_its_border_unscaled",
        left_same && right_same,
        "98 px out of a 130-px sheet: `blitNineSlicedSprite` takes the 2-px \
         corner columns verbatim at *both* ends, and a stretch would resample \
         the right pair one texel across",
    );

    // p6: hovering an unselected tab swaps its sheet. Two frames differing in
    // the mouse position alone.
    // In **GUI** space: every widget rect is, and passing screen pixels here is
    // exactly the live-path bug this witness found (see `LiveApp::mouse_gui`).
    let hover_x = (tx + tw + tw / 2) as f64;
    let hovered = render!(&view, &screen, Some((hover_x, 10.0)));
    let hover_changed = differs(&frame, &hovered, t1x0, t0y0, t1x1, t0y1);
    let elsewhere = differs(&frame, &hovered, t0x0, t0y0, t0x1, t0y1);
    c.record(
        "p6.hovering_an_unselected_tab_repaints_only_that_tab",
        hover_changed > 0 && elsewhere == 0,
        format!(
            "hovered tab changed in {hover_changed} px, its neighbour in \
             {elsewhere} — `tab` -> `tab_highlighted`, which is \
             `disabledFocused` on that record"
        ),
    );

    // p13: **the live-path bug, pinned in the gate.**
    //
    // Every widget rect is in GUI pixels and `LiveApp` holds the cursor in
    // *screen* pixels. M82 passed it straight through, M85 wrote three more
    // screens against the same call, and every gate kept passing because each
    // divides by the GUI scale before calling the builders. `LiveApp::mouse_gui`
    // is the fix; this is the witness that stops it being undone.
    //
    // The same point in screen pixels is the GUI point times the scale, which
    // at 640x480 is off the right-hand end of the 320-wide tab bar — so it must
    // hover nothing at all. If someone "simplifies" the gate to feed screen
    // pixels, `p6` above goes green-for-the-wrong-reason and this one goes red.
    let screen_px_hover = render!(
        &view,
        &screen,
        Some((hover_x * SCALE as f64, 10.0 * SCALE as f64))
    );
    c.record(
        "p13.the_cursor_is_in_gui_pixels_and_screen_pixels_hover_nothing",
        screen_px_hover == frame,
        format!(
            "GUI ({hover_x}, 10) hovers tab 1; the same point in screen pixels \
             ({}, 20) is past the tab bar's own {GUI_W}-px width and produces a \
             frame byte-identical to the unhovered one",
            hover_x * SCALE as f64
        ),
    );

    // p7: scrolling moves the list and nothing else.
    let (mut scrolled_view, scrolled_screen) = build(&counter, StatsTab::General, (None, 0));
    scrolled_view.model.list_mut().mouse_scrolled(-3.0);
    let scroll_now = scrolled_view.model.list().scroll();
    let scrolled = render!(&scrolled_view, &scrolled_screen, None);
    let (band_y, band_h) = ss::content_band(GUI_H);
    let in_band = differs(
        &frame,
        &scrolled,
        0,
        band_y as u32 * SCALE as u32,
        W,
        (band_y + band_h) as u32 * SCALE as u32,
    );
    let in_header = differs(
        &frame,
        &scrolled,
        0,
        0,
        W,
        ss::HEADER_HEIGHT as u32 * SCALE as u32,
    );
    c.record(
        "p7.scrolling_moves_the_rows_and_leaves_the_chrome_alone",
        in_band > 0 && in_header == 0 && scroll_now == 21.0,
        format!(
            "{in_band} px changed inside the band, {in_header} in the header; \
             scroll = {scroll_now} after three notches of 7"
        ),
    );

    // p8: the scrollbar is drawn only when `scrollable()`.
    let (mobs_view, mobs_screen) = build(&counter, StatsTab::Mobs, (None, 0));
    let mobs_scrollable = mobs_view.model.list().scrollable();
    let general_scrollable = view.model.list().scrollable();
    let bar_x = mobs_view.model.list().scroll_bar_x() as u32 * SCALE as u32;
    let mobs_frame = render!(&mobs_view, &mobs_screen, None);
    // The control is **the same frame, one tile period to the left**.
    //
    // Two earlier versions were both contaminated, and the battery caught each
    // in turn. The first compared the mobs frame against the general one and
    // asked only that they differ — which two *different* bars satisfy as well
    // as one bar and none, so a mutant drawing the bar unconditionally survived
    // (the two scroller heights are 32 and 175). The second used the loading
    // frame as the control, and the same mutant drew an *identical* bar there,
    // so the difference went back to zero. A control inside the frame under
    // test cannot be contaminated by a mutation of the frame's own contents.
    //
    // The background tiles every 32 GUI px from x = 0, so the column 32 px left
    // of the bar samples the same texels. In the lower half of the band the
    // mobs list has no row (its one 36-px row ends at y = 62), so both columns
    // are background alone unless a bar is drawn.
    let bar_probe_y0 = (band_y + 60) as u32 * SCALE as u32;
    let bar_probe_y1 = (band_y + band_h - 8) as u32 * SCALE as u32;
    let bar_col = |buf: &[u8]| {
        let mut n = 0;
        for y in bar_probe_y0..bar_probe_y1 {
            for d in 0..6 * SCALE as u32 {
                if px(buf, bar_x + d, y) != px(buf, bar_x + d - 32 * SCALE as u32, y) {
                    n += 1;
                }
            }
        }
        n
    };
    let (loading_view, loading_screen) =
        build(&StatsCounter::default(), StatsTab::General, (None, 0));
    let is_loading = loading_view.model.loading;
    let loading_general_rows = loading_view.model.general.len();
    let loading_frame = render!(&loading_view, &loading_screen, None);
    let (bar_mobs, bar_general) = (bar_col(&mobs_frame), bar_col(&frame));
    c.record(
        "p8.a_list_that_fits_draws_no_scrollbar_where_a_long_one_does",
        !mobs_scrollable && general_scrollable && bar_mobs == 0 && bar_general > 0,
        format!(
            "mobs list scrollable={mobs_scrollable} (1 row in a {band_h}-px \
             band) and its bar column matches the tile 32 px left of it in \
             {bar_mobs} px of disagreement; the general list is \
             scrollable={general_scrollable} and disagrees in {bar_general}"
        ),
    );

    // p14: **the nine-slice's vertical branches, in pixels.**
    //
    // The battery found the gap this closes. `widget/scroller` is 6×32 and the
    // general list's scroller comes out at exactly 32 (its `clamp(h²/content,
    // 32, h - 8)` floors), so it takes the 1:1 branch and proves nothing about
    // a vertical resize — a mutant restricted to M85's horizontal-only form
    // still drew it, and `p8` stayed green. The **background** track does not:
    // it is the same 32-tall sheet drawn at the list's whole 183-px height.
    //
    // So the sample is the bar column *below* the scroller, where only the
    // track can be, and the control is the **mobs** frame — whose list is not
    // scrollable, so it draws no bar at all under any implementation.
    //
    // `p8`'s own control (the tile 32 px to the left) is unusable here: that
    // column is GUI x 276, which is *inside* the 20..300 row span, and the
    // general list's 77 rows put text in it. The first version used it and
    // stayed green under a vertical-skip mutant for exactly that reason — a
    // contaminated control for the third time in this gate, and the third
    // distinct contamination.
    let list = view.model.list();
    let track_y0 = (list.scroll_bar_y() + list.scroller_height() + 4) as u32 * SCALE as u32;
    let track_y1 = (list.y + list.height - 4) as u32 * SCALE as u32;
    let track_seen = differs(
        &frame,
        &mobs_frame,
        bar_x,
        track_y0,
        bar_x + 6 * SCALE as u32,
        track_y1,
    );
    c.record(
        "p14.the_scrollbar_track_is_nine_sliced_vertically_past_its_own_sheet",
        list.scroller_height() == 32 && track_seen > 0,
        format!(
            "the scroller is {} px — exactly its sheet's height, so it is the \
             1:1 blit and proves nothing — and the track below it differs from \
             the bar-free mobs frame in {track_seen} px over rows \
             {track_y0}..{track_y1}. A horizontal-only nine-slice draws no \
             track at all and the two frames match",
            list.scroller_height()
        ),
    );

    // p9: the sort arrow appears only when a column is sorted, at
    // `getColumnX(col) - 36`.
    let (sorted_view, sorted_screen) = build(&counter, StatsTab::Items, (Some(1), -1));
    let (cx, cy, ..) = sorted_view.model.lists[StatsTab::Items.index()].content_rect(0);
    let sorted_frame = render!(&sorted_view, &sorted_screen, None);
    let arrow_x = (cx + ss::column_x(1) - 36) as u32 * SCALE as u32;
    let arrow_y = (cy + 1) as u32 * SCALE as u32;
    let at_arrow = differs(
        &items_frame,
        &sorted_frame,
        arrow_x,
        arrow_y,
        arrow_x + 18 * SCALE as u32,
        arrow_y + 18 * SCALE as u32,
    );
    let left_of_arrow = differs(
        &items_frame,
        &sorted_frame,
        0,
        arrow_y,
        arrow_x - 2,
        arrow_y + 18 * SCALE as u32,
    );
    c.record(
        "p9.the_sort_arrow_lands_at_the_column_less_thirty_six",
        at_arrow > 0 && left_of_arrow == 0,
        format!(
            "{at_arrow} px changed in the arrow's own 18x18 and {left_of_arrow} \
             to the left of it — `getColumnX(index) - 36`, one slot left of the \
             sort button itself"
        ),
    );

    // p10: the loading state. An empty counter draws the pending string and no
    // rows at all; the frame itself is `p8`'s control, built above.
    let general_rows = loading_general_rows;
    let rows_changed = differs(
        &frame,
        &loading_frame,
        0,
        (band_y + 20) as u32 * SCALE as u32,
        W,
        (band_y + band_h - 20) as u32 * SCALE as u32,
    );
    c.record(
        "p10.an_unanswered_request_renders_the_pending_state_and_no_rows",
        is_loading && general_rows == 0 && rows_changed > 0,
        format!(
            "loading={is_loading}, {general_rows} rows, {rows_changed} px \
             differ from the populated frame — `StatsScreen.isLoading` starts \
             true and only the first `award_stats` clears it"
        ),
    );

    // p11: the Done button lands on its predicted rect. Its sheet is 8-bit
    // greyscale, so `r == g == b` inside it — and the tiled backgrounds behind
    // it are a grey **with alpha** over green, which never is.
    //
    // **Predicted from literals, because the first version read
    // `ss::done_bounds` — the very function it was grading.** The battery moved
    // the button nine pixels right and this witness followed it and stayed
    // green: the self-calibrating shape M82's own `p2` was caught in.
    // `(320 - 200) / 2 = 60`, and `240 - 33 + (int) 6.5 = 213`.
    let (dx, dy) = (60, 213);
    let (bx0, by0) = ((dx * SCALE) as u32, (dy * SCALE) as u32);
    let (bx1, by1) = (((dx + 200) * SCALE) as u32, ((dy + 20) * SCALE) as u32);
    let grey = |p: [u8; 3]| p[0] == p[1] && p[1] == p[2];
    c.record(
        "p11.the_done_button_covers_exactly_the_rect_the_footer_predicted",
        grey(px(&frame, bx0, by0))
            && grey(px(&frame, bx1 - 1, by1 - 1))
            && !grey(px(&frame, bx0 - 1, by0 + 4))
            && !grey(px(&frame, bx1, by0 + 4)),
        format!(
            "({bx0},{by0})..({bx1},{by1}) in screen px — greyscale inside, and \
             the background's grey-over-green outside is never r==g==b"
        ),
    );

    // p12: the six sort buttons exist on the items tab and nowhere else.
    let header_band = differs(
        &frame,
        &items_frame,
        0,
        (band_y + 2) as u32 * SCALE as u32,
        W,
        (band_y + 24) as u32 * SCALE as u32,
    );
    let sort_widgets = screen_items
        .widgets
        .iter()
        .filter(|w| w.visible && (ss::SORT_FIRST..ss::SORT_FIRST + 6).contains(&w.id))
        .count();
    let sort_on_general = screen
        .widgets
        .iter()
        .filter(|w| w.visible && (ss::SORT_FIRST..ss::SORT_FIRST + 6).contains(&w.id))
        .count();
    c.record(
        "p12.the_six_sort_buttons_are_visible_on_the_items_tab_alone",
        sort_widgets == 6 && sort_on_general == 0 && header_band > 0,
        format!(
            "{sort_widgets} visible on items, {sort_on_general} on general, \
             and the header row differs in {header_band} px"
        ),
    );

    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("out dir: {e}"))?;
        for (name, buf) in [
            ("stats_general.png", &frame),
            ("stats_items.png", &items_frame),
            ("stats_items_sorted.png", &sorted_frame),
            ("stats_mobs.png", &mobs_frame),
            ("stats_loading.png", &loading_frame),
            ("stats_hovered.png", &hovered),
        ] {
            write_png(&dir.join(name), buf)?;
        }
        println!("[statshot] wrote 6 frames to {}", dir.display());
    }

    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}

/// Eyeball output only — never read back.
fn write_png(path: &std::path::Path, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("create {path:?}: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), W, H);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(rgba))
        .map_err(|e| format!("write {path:?}: {e}"))
}
