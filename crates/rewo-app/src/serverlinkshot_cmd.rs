//! `rewo serverlinkshot --check` — the M85 oracle: `server_links` (137), the
//! two screens it renders on, and the framework they needed.
//!
//! The path under test, end to end:
//!
//! ```text
//! a raw server_links body (built here)
//!   -> rewo_net::route_session            (with a REAL `Ids::resolve`d table)
//!   -> rewo_net::session::apply           (the same fn the configuration loop calls)
//!   -> rewo_net::session::SessionState    (where the links live while the session does)
//!   -> rewo_world::{pause_screen, server_links_screen, disconnect_screen}::build
//!   -> rewo_world::layout                 (GridLayout / LinearLayout / FrameLayout)
//!   -> live_cmd::{screen_chrome, screen_text_lines}   (the SAME builders the frame path calls)
//!   -> WorldRenderer::{set_screen, set_text} -> ScreenPass / TextPass
//!   -> Offscreen::read_rgba               (real pixels)
//! ```
//!
//! ## The rules, each earned elsewhere
//!
//! **The gate drives the real emitter.** M45's `install_shapes` failure and
//! M41's rotted `swingshot` fixture were gates that had quietly stopped testing
//! their subject; M71 found the same shape one layer up, where a refactor left
//! `weathershot` grading a path the client no longer took. So the router here
//! is `route_session` with an `Ids` resolved from the datagen report, and the
//! two chrome builders are the ones `LiveApp` calls — including through
//! `LiveApp::render_screen_only`, the no-session frame this milestone added.
//!
//! **The detector must not share a colour with its subject.** Fifteen detector
//! errors on this project are all one shape. Every pixel witness here renders
//! over a **pure magenta** clear (`255, 0, 255`), which nothing on these
//! screens can produce: the three button sheets are 8-bit greyscale (so every
//! button texel has `r == g == b`), the menu background is a flat black wash
//! (which keeps `r == b` and `g == 0`), and the text is white or `0xA0A0A0`.
//! `p1` asserts an otherwise-identical frame with no screen is *uniformly*
//! magenta.
//!
//! **Predict, do not eyeball.** The menu background's composite over a known
//! destination is arithmetic, so `p6` computes it rather than asking whether
//! the screen "looks darker" — and it is the witness that separates *drawn
//! once* from *drawn twice*, which is the only failure a uniform tile has.
//!
//! ## What the menu background can and cannot witness
//!
//! Both `menu_background.png` and `inworld_menu_background.png` are 16×16 of a
//! single colour, `rgba(0, 0, 0, 64)`. So the tile size, the 2× magnification
//! and the `in_world` selector are all **unobservable in 26.2's own assets**,
//! and this gate does not pretend otherwise: what it grades is the composite
//! (once, at the right alpha, in the right colour space) and the *coverage*
//! (every pixel, including the partial tiles past the right and bottom edges).
//! Those are the two properties a uniform texture still has, and the second one
//! is what a `while tx + tile <= w` loop bound would break.
//!
//! **No witness may panic.** Every lookup below is `Option`-shaped — a missing
//! widget, a short list, an absent label are all *failures to report*, not
//! crashes. That is not tidiness: this gate's own mutation battery scored four
//! mutations as "survived (gate stayed green)" which had in fact aborted the
//! process at an `unwrap()`, so every witness after the crash never ran and the
//! harness saw no `FAIL` line. A panicking witness and a passing one are
//! indistinguishable to anything that reads a battery's output — which is the
//! one instrument whose whole job is to tell them apart.
//!
//! **Fail-closed** on a fixed [`EXPECTED_WITNESSES`] count.

use clap::Args as ClapArgs;

use rewo_data::assets;
use rewo_data::packets::Packets;
use rewo_data::DataPaths;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::{SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_net::ids::Ids;
use rewo_net::server_links::{self as sl, KnownLinkType, ServerLink, ServerLinkLabel, ServerLinks};
use rewo_proto::writer::PacketWriter;
use rewo_world::disconnect_screen as dc;
use rewo_world::pause_screen as ps;
use rewo_world::screen::{MouseResult, Screen, WidgetKind};
use rewo_world::server_links_screen as sls;

/// Total named properties this gate asserts. Locked so a skipped property
/// fails the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 37;

const W: u32 = 640;
const H: u32 = 480;
/// `gui_scale(640, 480)` = 2 — asserted by `p0`, not assumed.
const SCALE: i32 = 2;
const GUI_W: i32 = W as i32 / SCALE;
const GUI_H: i32 = H as i32 / SCALE;

#[derive(ClapArgs)]
pub struct ServerLinkshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the convention every other `*shot` uses.
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
            "[serverlinkshot] {}  {name}: {detail}",
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

pub fn run(args: ServerLinkshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[serverlinkshot] mode: {mode} (serverless; the oracle asserts \
         unconditionally). Model half against the 26.2 decompile; pixel half \
         against a pure-magenta clear no part of these screens can produce."
    );

    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_wire(&mut c, &ids, &baked);
    check_model(&mut c, &baked);
    check_pixels(&mut c, &args, &baked)?;

    println!(
        "[serverlinkshot] witnesses observed: {} / {}",
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
    println!("[serverlinkshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ---------------------------------------------------------------------------
// The wire. Every body below is written here rather than borrowed from the
// decoder, so the two cannot agree by construction.
// ---------------------------------------------------------------------------

/// One `UntrustedEntry` with a **known** type: the `Either` flag `true`, a
/// VarInt id, then the URL.
fn known_entry(w: &mut PacketWriter, id: i32, url: &str) {
    w.bool(true);
    w.varint(id);
    w.string(url);
}

/// One with a **custom** label: the flag `false`, then one NBT tag, then the URL.
fn custom_entry(w: &mut PacketWriter, label: &str, url: &str) {
    w.bool(false);
    // A `Component` that is a bare string is `Codec.STRING` → `literal`, which
    // on the wire is network NBT: a bare tag byte with no name (8 = String),
    // then the length-prefixed body.
    w.buf.push(8);
    w.buf.extend_from_slice(&(label.len() as u16).to_be_bytes());
    w.buf.extend_from_slice(label.as_bytes());
    w.string(url);
}

/// The fixture packet: a known BUG_REPORT, a known WEBSITE, a custom label, and
/// one entry whose URL must be dropped.
fn links_body() -> Vec<u8> {
    let mut w = PacketWriter::default();
    w.varint(4);
    known_entry(&mut w, 0, "https://bugs.example");
    known_entry(&mut w, 6, "https://web.example");
    custom_entry(&mut w, "Our Discord", "https://discord.example");
    known_entry(&mut w, 2, "javascript:alert(1)");
    w.buf
}

fn check_wire(c: &mut Checker, ids: &Ids, baked: &assets::BakedAssets) {
    // w1 — the packet exists in BOTH connection states, and the two ids differ.
    //
    // The third time (M69 `update_tags`, M78 `custom_payload`/`store_cookie`).
    // A play-only resolution looks like it works until a server advertises its
    // links during configuration, which is where a vanilla server sends them.
    c.record(
        "w1.the_packet_resolves_by_name_in_both_connection_states",
        ids.cb_play_server_links == 137 && ids.cb_config_server_links != ids.cb_play_server_links,
        format!(
            "play {} / configuration {} — different numbers for the same \
             `common` packet, resolved by name so a renumber moves both",
            ids.cb_play_server_links, ids.cb_config_server_links
        ),
    );

    // w2 — the whole packet through the production router, onto the production
    // state. Four entries in, three out.
    let mut state = rewo_net::session::SessionState::default();
    let handled = rewo_net::route_session(ids.cb_play_server_links, &links_body(), ids, &mut state);
    let links = state.server_links.clone();
    c.record(
        "w2.a_server_links_packet_reaches_the_session_state_through_route_session",
        handled && links.len() == 3,
        format!(
            "handled={handled} kept={} of 4 — one URL failed \
             `parseAndValidateUntrustedUri`",
            links.len()
        ),
    );

    // w3 — the `Either` flag is `true` for the LEFT, which is the enum.
    //
    // MUTATION: invert the boolean. The custom entry's NBT tag byte (8) would
    // then be read as a VarInt id — id 8 is `NEWS` — and the known entries
    // would try to read an NBT tag out of a VarInt. The tell is not "does it
    // parse" but *which* alternative comes out, so both are asserted.
    let label = |i: usize| links.entries().get(i).map(|e| e.label.clone());
    c.record(
        "w3.the_either_flag_is_true_for_the_known_type_and_false_for_the_custom_one",
        label(0) == Some(ServerLinkLabel::Known(KnownLinkType::BugReport))
            && label(1) == Some(ServerLinkLabel::Known(KnownLinkType::Website))
            && label(2)
                == Some(ServerLinkLabel::Custom(rewo_proto::nbt::Nbt::String(
                    "Our Discord".into()
                ))),
        format!("{:?}", links.entries().iter().map(|e| &e.label).collect::<Vec<_>>()),
    );

    // w4 — `ByIdMap.continuous(…, ZERO)`: out of range is BUG_REPORT, in both
    // directions, and it is not an error.
    //
    // MUTATION: `readEnum` (which throws) or `WRAP` (`floorMod`, which would
    // answer SUPPORT for 12 and ANNOUNCEMENTS for −1). Both conventions really
    // exist one field away in other packets — M65 found `readEnum` and `ZERO`
    // inside a single packet and M83 found `WRAP` and `readEnum` one byte
    // apart — so guessing is a coin flip between three.
    let mut over = PacketWriter::default();
    over.varint(2);
    known_entry(&mut over, 12, "https://a.example");
    known_entry(&mut over, -1, "https://b.example");
    let mut s2 = rewo_net::session::SessionState::default();
    rewo_net::route_session(ids.cb_play_server_links, &over.buf, ids, &mut s2);
    c.record(
        "w4.an_out_of_range_link_type_becomes_bug_report_rather_than_erroring_or_wrapping",
        s2.server_links.len() == 2
            && s2
                .server_links
                .entries()
                .iter()
                .all(|e| matches!(e.label, ServerLinkLabel::Known(KnownLinkType::BugReport)))
            && KnownLinkType::by_id(12) != KnownLinkType::Support
            && KnownLinkType::by_id(-1) != KnownLinkType::Announcements,
        "ids 12 and −1 both resolve to BUG_REPORT — WRAP would give SUPPORT \
         and ANNOUNCEMENTS, and `readEnum` would reject the packet outright",
    );

    // w5 — the trust filter drops per entry, not per packet.
    //
    // MUTATION: failing the whole packet on a bad URL. That is M41's instinct
    // for an untranscribed component and it is wrong here: `handleServerLinks`
    // catches *inside* the loop, so a server one typo away from a broken link
    // keeps every other link it advertised.
    c.record(
        "w5.one_invalid_url_drops_itself_and_the_rest_of_the_packet_survives",
        links.len() == 3
            && links.entries().iter().all(|e| e.link.starts_with("https://"))
            && sl::validate_untrusted_uri("javascript:alert(1)").is_none()
            && sl::validate_untrusted_uri("http://a.example").is_some()
            && sl::validate_untrusted_uri("example.com").is_none(),
        "`ALLOWED_UNTRUSTED_LINK_PROTOCOLS` is `{http, https}` and a missing \
         scheme is also a rejection — a colonless string has a *null* scheme in \
         Java, not an empty one",
    );

    // w6 — the list is assigned, not merged, so an empty packet retracts.
    //
    // MUTATION: extending. An empty list would then do nothing at all, and the
    // pause menu would keep a "Server Links..." button for links the server has
    // withdrawn.
    let mut s3 = state.clone();
    rewo_net::route_session(ids.cb_play_server_links, &[0x00], ids, &mut s3);
    c.record(
        "w6.a_second_packet_replaces_the_list_and_an_empty_one_retracts_it",
        s3.server_links.is_empty() && !state.server_links.is_empty(),
        "`handleServerLinks` builds a fresh `ServerLinks` from the packet",
    );

    // w7 — `findKnownType` matches the LEFT alternative only, and takes the
    // first. This is the whole of what reaches the disconnect screen.
    //
    // MUTATION: matching a custom label by its text. A server label reading
    // "Report a bug" is *not* a BUG_REPORT entry, and treating it as one puts
    // an arbitrary server string on the disconnect screen's report button.
    let decoy = ServerLinks::new(vec![
        ServerLink {
            label: ServerLinkLabel::Custom(rewo_proto::nbt::Nbt::String("Report a bug".into())),
            link: "https://decoy.example".into(),
        },
        ServerLink {
            label: ServerLinkLabel::Known(KnownLinkType::BugReport),
            link: "https://first.example".into(),
        },
        ServerLink {
            label: ServerLinkLabel::Known(KnownLinkType::BugReport),
            link: "https://second.example".into(),
        },
    ]);
    c.record(
        "w7.find_known_type_ignores_custom_labels_and_takes_the_first_match",
        decoy
            .find_known_type(KnownLinkType::BugReport)
            .is_some_and(|e| e.link == "https://first.example")
            && decoy.find_known_type(KnownLinkType::Status).is_none(),
        "`e.type.map(l -> l == type, r -> false)` then `findFirst`",
    );

    // w8 — the ten lang keys against the REAL jar, including the transposed one.
    //
    // MUTATION: deriving the name from the constant. `BUG_REPORT`'s string is
    // `report_bug`, and it is the only entry whose name is not its constant
    // lowercased — so a derived key gives `known_server_link.bug_report`, which
    // the jar does not have, and the button renders its own key.
    let lang = &baked.lang;
    let all = KnownLinkType::VALUES
        .iter()
        .all(|t| lang.get(&t.lang_key()).is_some());
    c.record(
        "w8.all_ten_known_link_types_resolve_against_the_real_jars_language_map",
        all && lang.get("known_server_link.report_bug") == Some("Report Server Bug")
            && lang.get("known_server_link.bug_report").is_none(),
        format!(
            "10/10 resolve; BUG_REPORT's key is `report_bug` → {:?}, and the \
             constant-derived `bug_report` does not exist",
            lang.get("known_server_link.report_bug")
        ),
    );

    // w9 — the pause menu's button label is the dialog's **externalTitle**.
    //
    // MUTATION: `computeExternalTitle` reading `title` first. Both keys exist
    // and both are plausible English, so the wrong one renders "Server Links"
    // where vanilla renders "Server Links..." and nothing errors.
    c.record(
        "w9.the_pause_buttons_label_is_the_dialogs_external_title",
        lang.get(ps::KEY_SERVER_LINKS_BUTTON) == Some("Server Links...")
            && lang.get(sls::KEY_TITLE) == Some("Server Links")
            && ps::KEY_SERVER_LINKS_BUTTON != sls::KEY_TITLE,
        "`externalTitle.orElse(title)` — the dialog registers both, and the \
         screen's own header uses the other one",
    );
}

// ---------------------------------------------------------------------------
// The model. Every expectation is an independent literal from the decompile,
// never a call into the code under test.
// ---------------------------------------------------------------------------

fn pause_labels() -> ps::PauseLabels {
    ps::PauseLabels {
        title: "Game Menu".into(),
        return_to_game: "Back to Game".into(),
        advancements: "Advancements".into(),
        stats: "Statistics".into(),
        options: "Options...".into(),
        disconnect: "Disconnect".into(),
        server_links: "Server Links...".into(),
    }
}

fn links_labels(n: usize) -> sls::ServerLinksLabels {
    sls::ServerLinksLabels {
        title: "Server Links".into(),
        back: "Back".into(),
        links: (0..n).map(|i| format!("Link {i}")).collect(),
    }
}

fn disconnect_labels() -> dc::DisconnectLabels {
    dc::DisconnectLabels {
        title: "Connection Lost".into(),
        back: "Back to Server List".into(),
        report: "Report to Server".into(),
    }
}

fn check_model(c: &mut Checker, baked: &assets::BakedAssets) {
    let with = ps::build(&pause_labels(), true, 54, GUI_W, GUI_H);
    let without = ps::build(&pause_labels(), false, 54, GUI_W, GUI_H);

    // m1 — the pause grid, from `createPauseMenu`'s literals.
    //
    // `padding(4, 4, 4, 0)` is the FOUR-argument overload: left 4, top 4, right
    // 4, **bottom 0**. Reading it as symmetric adds 4 px to every row gap, so
    // `Disconnect` moves 20 px and `Back to Game` does not — a shift no
    // single-widget assertion sees.
    // `Option`-shaped throughout: a missing widget is a failure to report, not
    // a panic. See the module docs.
    let y = |s: &Screen, id| s.widget(id).map(|w| w.y);
    let wx = |s: &Screen, id| s.widget(id).map(|w| w.x);
    let ww = |s: &Screen, id| s.widget(id).map(|w| w.width);
    let sub2 = |a: Option<i32>, b: Option<i32>| match (a, b) {
        (Some(a), Some(b)) => Some(a - b),
        _ => None,
    };
    c.record(
        "m1.the_pause_rows_stack_at_twenty_four_and_the_first_is_pushed_down_fifty",
        sub2(y(&without, ps::ADVANCEMENTS), y(&without, ps::RETURN_TO_GAME)) == Some(24)
            && sub2(y(&without, ps::ICON_ROW), y(&without, ps::ADVANCEMENTS)) == Some(24)
            && sub2(y(&without, ps::DISCONNECT), y(&without, ps::OPTIONS)) == Some(24)
            && ww(&without, ps::RETURN_TO_GAME) == Some(204)
            && ww(&without, ps::ADVANCEMENTS) == Some(98),
        format!(
            "204 / 98 / 92, rows 24 apart — `BUTTON_PADDING` 4 top and 0 \
             bottom, and the first row's `paddingTop(50)` on top of that \
             (Back to Game at y={:?})",
            y(&without, ps::RETURN_TO_GAME)
        ),
    );

    // m2 — `alignInRectangle(grid, 0, 0, w, h, 0.5F, 0.25F)`: centred
    // horizontally, a QUARTER of the way down.
    //
    // MUTATION: 0.5 vertically. At 320x240 that moves the menu 33 px down, and
    // "the menu is in the middle" is exactly what a screenshot would suggest.
    let grid_h = (ps::MENU_PADDING_TOP + 20) + 4 * (ps::BUTTON_PADDING + 20);
    // The grid's own width is the spanning child's **outer** width — 204 plus
    // the cell's 4 px each side — and the child then sits `paddingLeft` inside
    // it, because the default cell's `xAlignment` is 0. So the button's left
    // edge is `(320 - 212) / 2 + 4`, not `(320 - 204) / 2`: the two differ by
    // 4 px, which is exactly the amount a reading that forgot the padding is
    // out by.
    let expected_top = (0.25f32 * (GUI_H - grid_h) as f32) as i32 + ps::MENU_PADDING_TOP;
    let expected_x = (GUI_W - (204 + 2 * ps::BUTTON_PADDING)) / 2 + ps::BUTTON_PADDING;
    c.record(
        "m2.the_pause_menu_is_a_quarter_of_the_way_down_and_not_centred",
        y(&without, ps::RETURN_TO_GAME) == Some(expected_top)
            && wx(&without, ps::RETURN_TO_GAME) == Some(expected_x),
        format!(
            "top {:?} (0.25 of the leftover, not {} at 0.5); left {:?} = (320 - 212) / 2 + 4",
            y(&without, ps::RETURN_TO_GAME),
            (0.5f32 * (GUI_H - grid_h) as f32) as i32 + ps::MENU_PADDING_TOP,
            wx(&without, ps::RETURN_TO_GAME),
        ),
    );

    // m3 — **the packet's whole effect on this screen.**
    //
    // MUTATION: always adding the button (which claims links on a server that
    // sent none), or adding it after Options (where a reader who skimmed
    // `createPauseMenu` would put it).
    c.record(
        "m3.the_server_links_button_appears_only_with_links_and_pushes_the_rows_below",
        without.widget(ps::SERVER_LINKS).is_none()
            && with.widget(ps::SERVER_LINKS).is_some_and(|b| {
                b.width == 204
                    && b.message == "Server Links..."
                    && Some(b.y) > y(&with, ps::ICON_ROW)
                    && Some(b.y) < y(&with, ps::OPTIONS)
            })
            && y(&with, ps::RETURN_TO_GAME) < y(&without, ps::RETURN_TO_GAME),
        "one extra row between the icon row and Options — and it makes the \
         grid taller, so the 0.25 alignment moves the whole menu *up*",
    );

    // m4 — the icon row is reserved geometry: right size, right place,
    // click-through.
    //
    // MUTATION: dropping it. Every widget below moves up 24 px and nothing
    // about a rendered frame says which layout is vanilla's — which is why the
    // row is graded by geometry rather than by being drawn.
    let mut clickable = with.clone();
    let icon_row_ok = match (with.widget(ps::ICON_ROW), with.widget(ps::RETURN_TO_GAME)) {
        (Some(row), Some(full)) => {
            let (rx, ry) = (row.x, row.y);
            let geometry = row.kind == WidgetKind::Reserved
                && (row.width, row.height) == (92, 20)
                && row.x + row.width / 2 == full.x + full.width / 2;
            geometry
                && clickable.mouse_clicked(rx as f64 + 2.0, ry as f64 + 2.0, 0)
                    == MouseResult::Ignored
        }
        _ => false,
    };
    c.record(
        "m4.the_pause_icon_row_reserves_a_ninety_two_pixel_cell_and_swallows_no_clicks",
        icon_row_ok,
        "4 * 20 + 3 * 4 = 92, centred across the spanning cell — four dead \
         buttons would be worse and omitting the row would move five widgets",
    );

    // m5 — the dialog: one 310-wide button per link, in WIRE order, 2 px apart.
    //
    // MUTATION: `packControlsIntoColumns`' last-row `LinearLayout` branch. With
    // `columns == 1` it is unreachable (`count / 1 * 1 == count`), and taking it
    // would put every button on one horizontal row.
    let dlg = sls::build(&links_labels(3), 66, GUI_W, GUI_H);
    let link = |i: u32| dlg.widget(sls::LINK_BASE + i).cloned();
    let links_ok = (0..3)
        .all(|i| link(i).is_some_and(|b| b.width == 310 && b.message == format!("Link {i}")))
        && sub2(link(1).map(|b| b.y), link(0).map(|b| b.y)) == Some(22)
        && sub2(link(2).map(|b| b.y), link(1).map(|b| b.y)) == Some(22)
        && link(0).map(|b| b.x) == link(1).map(|b| b.x);
    c.record(
        "m5.each_link_is_a_three_hundred_and_ten_wide_button_two_pixels_below_the_last",
        links_ok,
        format!(
            "`buttonWidth` 310, `rowSpacing(2)`, one column — first at {:?}",
            link(0).map(|b| (b.x, b.y))
        ),
    );

    // m6 — the header's warning button displaces the title left of centre.
    //
    // MUTATION: dropping the warning button from the header layout. The title
    // centres, 15 px right of where vanilla puts it — a shift no witness that
    // only looks at the *buttons* would ever see.
    let header_ok = match (dlg.widget(sls::TITLE), dlg.widget(sls::WARNING)) {
        (Some(title), Some(warn)) => {
            warn.kind == WidgetKind::Reserved
                && (warn.width, warn.height) == (20, 20)
                && title.x == (GUI_W - (66 + 10 + 20)) / 2
                && warn.x == title.x + 66 + 10
                && title.x + title.width / 2 < GUI_W / 2
        }
        _ => false,
    };
    c.record(
        "m6.the_reserved_warning_button_pushes_the_dialog_title_left_of_centre",
        header_ok,
        format!(
            "title at x={:?} in a `[title | 10 | warning]` horizontal row, \
             centred as a group; warning at x={:?}",
            dlg.widget(sls::TITLE).map(|w| w.x),
            dlg.widget(sls::WARNING).map(|w| w.x)
        ),
    );

    // m7 — the footer keeps its 33-px band because the exit action is present.
    //
    // MUTATION: `ifPresentOrElse`'s else branch (`setFooterHeight(5)`), which
    // moves the button 14 px down and lets the body grow into the space.
    // The `+ 7` is `AbstractChildWrapper.setY`'s **rounding** on a 13-px
    // leftover, not the 6 an integer division gives.
    let back = dlg.widget(sls::BACK).cloned();
    c.record(
        "m7.the_dialogs_back_button_sits_in_a_thirty_three_pixel_footer",
        back.as_ref()
            .is_some_and(|b| b.width == 200 && b.y == GUI_H - 33 + 7 && b.x == (GUI_W - 200) / 2),
        format!(
            "y={:?} = {} - 33 + round(13/2); an integer division would give {}",
            back.as_ref().map(|b| b.y),
            GUI_H,
            GUI_H - 33 + 6
        ),
    );

    // m8 — **the milestone's other half.** Only a client-side error offers the
    // server's bug-report link.
    //
    // MUTATION: filling it for every cause. A server that kicks you would then
    // show a "Report to Server" button vanilla does not draw, and it would
    // *look* right — the link is real and the label is real. The distinction
    // lives entirely in which of `DisconnectionDetails`' two constructors ran.
    const BUG: Option<&str> = Some("https://bugs.example");
    let noisy = dc::DisconnectDetails::new(dc::DisconnectCause::ClientError, "boom", BUG);
    let quiet = dc::DisconnectDetails::new(dc::DisconnectCause::ServerRequested, "banned", BUG);
    let eos = dc::DisconnectDetails::new(dc::DisconnectCause::EndOfStream, "gone", BUG);
    c.record(
        "m8.only_a_client_side_error_offers_the_servers_bug_report_link",
        noisy.bug_report_link.as_deref() == BUG
            && quiet.bug_report_link.is_none()
            && eos.bug_report_link.is_none()
            && dc::DisconnectDetails::new(dc::DisconnectCause::ClientError, "x", None)
                .bug_report_link
                .is_none(),
        "`createDisconnectionInfo` fills it; `new DisconnectionDetails(reason)` \
         leaves both optionals empty",
    );

    // m9 — the report button appears with the link and pushes Back down.
    let adv = baked.font.as_ref().map(|f| f.advance).unwrap_or([6u8; 256]);
    let w6 = move |t: &str| rewo_gpu::text::width(t, &adv);
    let a = dc::build(&disconnect_labels(), &quiet, GUI_W, GUI_H, &w6);
    let b = dc::build(&disconnect_labels(), &noisy, GUI_W, GUI_H, &w6);
    c.record(
        "m9.the_report_button_appears_with_the_link_and_moves_the_back_button",
        a.widget(dc::REPORT).is_none()
            && b.widget(dc::REPORT).is_some_and(|r| r.width == 200)
            && sub2(y(&b, dc::BACK), y(&b, dc::REPORT)) == Some(24)
            && y(&b, dc::TITLE) < y(&a, dc::TITLE),
        "20 + 2 + 2 between the two buttons, and the taller stack re-centres \
         upward",
    );

    // m10 — the disconnect screen's two paddings, and that the mid-build
    // `padding(2)` does not reach back to the two text cells.
    //
    // MUTATION: applying `padding(2)` to every cell — the reading where
    // `defaultCellSetting()` is consulted at arrange time rather than copied at
    // add time. The title/reason gap collapses from 20 to 4.
    c.record(
        "m10.the_disconnect_title_and_reason_carry_ten_pixel_padding_and_the_buttons_two",
        sub2(y(&a, dc::REASON), y(&a, dc::TITLE)) == Some(9 + 10 * 2)
            && sub2(y(&a, dc::BACK), y(&a, dc::REASON)) == Some(9 + 10 + 2),
        format!(
            "title→reason {:?} (9 + 10 + 10), reason→button {:?} (9 + 10 + 2)",
            sub2(y(&a, dc::REASON), y(&a, dc::TITLE)),
            sub2(y(&a, dc::BACK), y(&a, dc::REASON))
        ),
    );

    // m11 — the wrap's overflow test is `width > maxWidth` measured AFTER
    // adding the character, so a line exactly `maxWidth` wide still fits.
    //
    // MUTATION: `>=`. The sample sits on the boundary in a font where three
    // characters are exactly the limit.
    let six = |t: &str| t.chars().count() as i32 * 6;
    c.record(
        "m11.the_wrap_breaks_at_the_last_space_and_a_line_of_exactly_max_width_fits",
        dc::split_lines("abc def", 18, &six) == vec!["abc", "def"]
            && dc::split_lines("abcdef", 18, &six) == vec!["abc", "def"]
            && dc::split_lines("abc", 18, &six) == vec!["abc"]
            && dc::split_lines("abc", 1, &six) == vec!["a", "b", "c"],
        "`hadNonZeroWidthChar` also guarantees progress, so a 1-px box makes \
         one-character lines rather than looping",
    );

    // m12 — the layout module's two corrections, asserted where they bite.
    //
    // `Divisor` gives the remainder to the LATER part — the transposed reading
    // is what this milestone's own unit test asserted until the transcription
    // disagreed with it. And `setX` truncates where `setY` rounds, four lines
    // apart in `AbstractChildWrapper`.
    let mut d = rewo_world::layout::Divisor::new(5, 2);
    let parts: Vec<i32> = std::iter::from_fn(|| d.next_int()).collect();
    c.record(
        "m12.the_divisor_carries_its_remainder_forward_to_the_later_part",
        parts == vec![2, 3],
        format!("Divisor(5, 2) = {parts:?}, not [3, 2] and not [2, 2]"),
    );
    c.record(
        "m13.frame_alignment_truncates_where_grid_vertical_alignment_rounds",
        rewo_world::layout::align_in_dimension(0, 21, 20, 0.5) == 0
            && back.as_ref().is_some_and(|b| b.y == GUI_H - 33 + 7),
        "`(int)Mth.lerp(...)` in `alignInDimension` against \
         `Math.round(Mth.lerp(...))` in `AbstractChildWrapper.setY` — one \
         pixel, on every odd leftover, in opposite directions",
    );

    // m14 — Esc: the pause screen closes, the disconnect screen does not.
    //
    // MUTATION: copying the death screen's `with_close_on_esc(false)` onto the
    // pause screen. It would then be inescapable except through its own
    // buttons, which is a close relative of a hung client.
    use rewo_world::screen::KeyResult;
    c.record(
        "m14.esc_closes_the_pause_screen_and_cannot_dismiss_the_disconnect_one",
        with.clone().key_pressed(256, false) == KeyResult::Close
            && a.clone().key_pressed(256, false) == KeyResult::Ignored
            && dlg.clone().key_pressed(256, false) == KeyResult::Close,
        "`shouldCloseOnEsc()` — default true, overridden false on \
         `DisconnectedScreen`, and `common().canCloseWithEscape()` on the dialog",
    );

    // m15 — Tab reaches every button and skips the reserved widgets and labels.
    let mut tab = with.clone();
    let mut seen = Vec::new();
    for _ in 0..6 {
        tab.key_pressed(258, false);
        seen.push(tab.focused());
    }
    let seen: Vec<u32> = seen.into_iter().flatten().collect();
    c.record(
        "m15.tab_reaches_every_pause_button_and_skips_the_labels_and_reserved_cells",
        seen == vec![
            ps::RETURN_TO_GAME,
            ps::ADVANCEMENTS,
            ps::STATS,
            ps::SERVER_LINKS,
            ps::OPTIONS,
            ps::DISCONNECT,
        ],
        format!(
            "{seen:?} — `StringWidget` and the reserved cells are `active = \
             false`, which `nextFocusPath` skips with no special case"
        ),
    );
}

// ---------------------------------------------------------------------------
// The pixels.
// ---------------------------------------------------------------------------

fn srgb_decode(b: u8) -> f32 {
    let s = b as f32 / 255.0;
    if s <= 0.040_45 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb_encode(l: f32) -> u8 {
    let s = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

fn px(buf: &[u8], x: u32, y: u32) -> [u8; 3] {
    let i = ((y * W + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2]]
}

fn check_pixels(
    c: &mut Checker,
    args: &ServerLinkshotArgs,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[serverlinkshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("serverlinkshot: Vulkan validation requested but not active".into());
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
    // Pure magenta. The button sheets are 8-bit greyscale (`r == g == b`), the
    // menu background is a flat black wash (which leaves `r == b`, `g == 0`),
    // and the text is white or 0xA0A0A0 — so nothing on these screens can
    // produce a colour with `r != b`, and `r == g == b` means *button*.
    const CLEAR_LINEAR: [f32; 3] = [1.0, 0.0, 1.0];
    let clear = [CLEAR_LINEAR[0], CLEAR_LINEAR[1], CLEAR_LINEAR[2], 1.0];

    let mut shot = |gpu: &mut Gpu,
                    off: &mut Offscreen,
                    wr: &mut WorldRenderer,
                    chrome: rewo_gpu::screen::ScreenDraw,
                    lines: Vec<rewo_gpu::world::OwnedTextLine>|
     -> Result<Vec<u8>, String> {
        wr.set_screen(chrome);
        wr.set_text(lines);
        off.render(gpu, Some((&mut *wr, vp)), &overlay_draw, clear)?;
        off.read_rgba(gpu)
    };

    let empty = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        rewo_gpu::screen::ScreenDraw::default(),
        Vec::new(),
    )?;
    c.record(
        "p1.a_frame_with_no_screen_is_uniformly_the_clear_colour",
        (0..W * H).all(|i| {
            let p = &empty[(i * 4) as usize..];
            p[0] == 255 && p[1] == 0 && p[2] == 255
        }),
        "so every count below is the screen and nothing else — `handshot`'s \
         rule after fifteen detector errors of the same shape",
    );

    // The three screens, through the SAME builders `LiveApp` calls.
    let pause = ps::build(&pause_labels(), true, 54, GUI_W, GUI_H);
    let dialog = sls::build(&links_labels(3), 66, GUI_W, GUI_H);
    let disconnected = {
        let w = |t: &str| rewo_gpu::text::width(t, &advance);
        let details = dc::DisconnectDetails::new(
            dc::DisconnectCause::ClientError,
            "the server closed the connection because of a very long administrative reason",
            Some("https://bugs.example"),
        );
        dc::build(&disconnect_labels(), &details, GUI_W, GUI_H, &w)
    };

    // p2 — the menu background is wired at all. Asserted before it is switched
    // off below, so the suppression cannot hide a missing wire.
    let pause_chrome = crate::live_cmd::screen_chrome(&pause, None);
    let dc_chrome = crate::live_cmd::screen_chrome(&disconnected, None);
    c.record(
        "p2.the_two_new_screens_ask_for_the_menu_background_and_no_gradient",
        pause_chrome.backdrop.is_none()
            && pause_chrome
                .menu_background
                .is_some_and(|b| b.in_world)
            && dc_chrome
                .menu_background
                .is_some_and(|b| !b.in_world)
            && pause_chrome.buttons.len() == 6,
        format!(
            "`extractBackground`'s two branches are an if/else: these take the \
             `extractMenuBackground` one. {} buttons (the labels and the \
             reserved cells are not buttons)",
            pause_chrome.buttons.len()
        ),
    );

    // p3 — the menu background composites EXACTLY ONCE, predicted arithmetically.
    //
    // The sheet is a uniform `rgba(0, 0, 0, 64)`, so this is
    // `dst * (1 - 64/255)` in linear space. It separates: not drawn (the clear
    // survives), drawn twice (the square of the factor), the wrong alpha, and
    // a gamma-space blend.
    let bg_only = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        rewo_gpu::screen::ScreenDraw {
            menu_background: Some(rewo_gpu::screen::MenuBackgroundDraw { in_world: true }),
            ..Default::default()
        },
        Vec::new(),
    )?;
    let a = 64.0 / 255.0;
    let once: [u8; 3] = std::array::from_fn(|i| srgb_encode(CLEAR_LINEAR[i] * (1.0 - a)));
    let twice: [u8; 3] =
        std::array::from_fn(|i| srgb_encode(CLEAR_LINEAR[i] * (1.0 - a) * (1.0 - a)));
    let seen = px(&bg_only, W / 2, H / 2);
    let near = |x: [u8; 3], y: [u8; 3]| x.iter().zip(y).all(|(p, q)| p.abs_diff(q) <= 2);
    c.record(
        "p3.the_menu_background_composites_exactly_once_at_the_sheets_own_alpha",
        near(seen, once) && !near(seen, twice),
        format!(
            "{seen:?} vs once {once:?} (twice would be {twice:?}) — the sheet \
             is a flat rgba(0,0,0,64), so a doubled tile is the one failure a \
             uniform texture still shows"
        ),
    );

    // p4 — and it covers **every** pixel, including the strip past the last
    // whole tile.
    //
    // MUTATION: `while tx + tile <= w`. At 640x480 with a 64-screen-pixel tile
    // the width divides evenly, so the bottom edge is what tells: 480 / 64 =
    // 7.5, leaving a 32-px strip that a `<=` bound would leave clear.
    let uncovered = (0..W * H)
        .filter(|i| {
            let p = &bg_only[(*i * 4) as usize..];
            p[0] == 255 && p[1] == 0 && p[2] == 255
        })
        .count();
    c.record(
        "p4.the_menu_background_covers_the_partial_tiles_past_the_screen_edge",
        uncovered == 0,
        format!(
            "{uncovered} pixels still the clear colour; 480 / (32 * {SCALE}) = \
             7.5, so a `tx + tile <= w` loop bound would leave a 32-px strip"
        ),
    );

    // From here on the background is suppressed, so a button's rect can be
    // measured against the clear rather than against a wash. Stated rather
    // than silent: `p2` already proved the wire.
    let geometry = |screen: &Screen| {
        let mut chrome = crate::live_cmd::screen_chrome(screen, None);
        chrome.menu_background = None;
        chrome
    };

    let dialog_frame = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        geometry(&dialog),
        crate::live_cmd::screen_text_lines(&dialog, &advance, SCALE as f32),
    )?;
    // The buttons are the only greyscale thing on screen.
    let is_button = |p: [u8; 3]| p[0] == p[1] && p[1] == p[2];
    // A missing widget is a failure to *report*: fall back to a rect that is
    // guaranteed not to contain a button, so `p5` fails rather than aborting.
    let link0 = dialog
        .widget(sls::LINK_BASE)
        .cloned()
        .unwrap_or_else(|| rewo_world::screen::Widget::button(0, 0, 0, 1, 1, ""));
    let (bx0, by0) = ((link0.x * SCALE) as u32, (link0.y * SCALE) as u32);
    let (bx1, by1) = (
        ((link0.x + link0.width) * SCALE) as u32,
        ((link0.y + link0.height) * SCALE) as u32,
    );

    // p5 — **the headline.** A 310-wide button draws, and covers exactly its
    // rect. M82's `ScreenPass` would have skipped it entirely.
    c.record(
        "p5.a_three_hundred_and_ten_wide_button_draws_and_covers_exactly_its_rect",
        is_button(px(&dialog_frame, bx0, by0))
            && is_button(px(&dialog_frame, bx1 - 1, by1 - 1))
            && !is_button(px(&dialog_frame, bx0 - 1, by0 + 4))
            && !is_button(px(&dialog_frame, bx1, by0 + 4))
            && !is_button(px(&dialog_frame, bx0 + 4, by0 - 1))
            && !is_button(px(&dialog_frame, bx0 + 4, by1)),
        format!(
            "({bx0},{by0})..({bx1},{by1}) in screen px — greyscale inside, \
             clear one pixel outside every edge"
        ),
    );

    // p6/p7 — the inner segment **tiles**, it does not stretch.
    //
    // The comparison is against a 200-wide button of the same sprite drawn at
    // the same y: a nine-slice at exactly the sheet's width is the 1:1 blit, so
    // the first `3 + 194 = 197` GUI px of a 310-wide button must be
    // byte-identical to the same span of the 200-wide one. A stretched inner
    // segment cannot be — it maps 304 px of destination onto the sheet's 194,
    // so every column past the third moves.
    //
    // MUTATION: replace `push_tiled` with one quad spanning the inner segment.
    // p6 fails and p7 fails with it.
    let control = rewo_gpu::screen::ScreenDraw {
        buttons: vec![
            rewo_gpu::screen::ButtonDraw {
                x: link0.x,
                y: link0.y,
                width: 310,
                height: 20,
                sprite: rewo_gpu::screen::ButtonSprite::Enabled,
            },
            rewo_gpu::screen::ButtonDraw {
                x: link0.x,
                y: link0.y + 40,
                width: 200,
                height: 20,
                sprite: rewo_gpu::screen::ButtonSprite::Enabled,
            },
        ],
        ..Default::default()
    };
    let pair = shot(&mut gpu, &mut off, &mut wr, control, Vec::new())?;
    let wide_row = |gx: i32| px(&pair, (link0.x + gx) as u32 * SCALE as u32, by0 + 8);
    let narrow_row =
        |gx: i32| px(&pair, (link0.x + gx) as u32 * SCALE as u32, by0 as u32 + 40 * 2 + 8);
    let first_tile_matches = (0..197).all(|gx| wide_row(gx) == narrow_row(gx));
    c.record(
        "p6.the_nine_slices_first_tile_is_byte_identical_to_the_one_to_one_blit",
        first_tile_matches,
        "the left border plus one whole 194-px tile of a 310-wide button match \
         the same span of a 200-wide one column for column — a stretched inner \
         segment moves every column past the third",
    );
    // And the tile really does repeat: GUI column 197 (the start of the second,
    // partial tile) is the same texel column as 3 (the start of the first).
    c.record(
        "p7.the_partial_second_tile_restarts_the_pattern_rather_than_continuing_it",
        wide_row(197) == wide_row(3) && wide_row(198) == wide_row(4),
        format!(
            "column 197 {:?} == column 3 {:?} — `TiledBlitRenderState` clips \
             the last tile by UV (`Mth.lerp(remaining / tileWidth, u0, u1)`), \
             it does not scale it",
            wide_row(197),
            wide_row(3)
        ),
    );
    // p8 — the right border lands on the right edge, from the sheet's own
    // right border and not from the middle.
    c.record(
        "p8.the_right_border_column_comes_from_the_sheets_right_border",
        wide_row(309) == narrow_row(199) && wide_row(307) == narrow_row(197),
        "the last three columns of a 310-wide button are the sheet's own last \
         three, which is what `blitSprite(…, nineSlice.width() - borderRight, …)` \
         asks for",
    );

    // p9 — a reserved widget draws nothing at all.
    //
    // MUTATION: rendering `Reserved` as a button. The frame would gain a 20×20
    // sprite in the dialog header and a 92×20 one on the pause menu, and both
    // would look entirely plausible.
    let mut without_reserved = dialog.clone();
    without_reserved.widgets.retain(|w| w.kind != WidgetKind::Reserved);
    let stripped = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        geometry(&without_reserved),
        crate::live_cmd::screen_text_lines(&without_reserved, &advance, SCALE as f32),
    )?;
    c.record(
        "p9.a_reserved_widget_renders_nothing_at_all",
        stripped == dialog_frame,
        "removing every `Reserved` widget from the screen is a byte-identical \
         frame — the geometry is real and the rendering is deliberately absent",
    );

    // p10 — the dialog's title is a `StringWidget`: drawn at its own x, and
    // vertically centred in its own 9-px height (which is a no-op, and is
    // asserted so a `(height - 9) / 2` typo on a taller label would show).
    let title = dialog
        .widget(sls::TITLE)
        .cloned()
        .unwrap_or_else(|| rewo_world::screen::Widget::label(0, 0, 0, 1, ""));
    let title_band = |buf: &[u8]| {
        let y0 = (title.y * SCALE) as u32;
        (y0..y0 + 9 * SCALE as u32)
            .flat_map(|y| (0..W).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let p = px(buf, x, y);
                p[0] > 200 && p[1] > 200 && p[2] > 200
            })
            .map(|(x, _)| x)
            .min()
    };
    c.record(
        "p10.the_dialog_title_is_drawn_at_the_string_widgets_own_x",
        title_band(&dialog_frame)
            .is_some_and(|x| x >= (title.x * SCALE) as u32 && x < ((title.x + 4) * SCALE) as u32),
        format!(
            "leftmost white pixel in the title band at x={:?}, widget x={} \
             (×{SCALE})",
            title_band(&dialog_frame),
            title.x
        ),
    );

    // p11 — the disconnect screen renders with **no session anywhere**: the
    // wrapped reason really is more than one line, and every line is inside
    // `width - 50`.
    let dc_frame = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        geometry(&disconnected),
        crate::live_cmd::screen_text_lines(&disconnected, &advance, SCALE as f32),
    )?;
    let reason = disconnected
        .widget(dc::REASON)
        .cloned()
        .unwrap_or_else(|| rewo_world::screen::Widget::multi_label(0, 0, 0, 1, Vec::new(), false));
    let (lines, centered) = match &reason.kind {
        WidgetKind::MultiLabel { lines, centered } => (lines.clone(), *centered),
        _ => (Vec::new(), false),
    };
    let text_rows = |buf: &[u8], y0: u32, y1: u32| {
        (y0..y1)
            .filter(|&y| {
                (0..W).any(|x| {
                    let p = px(buf, x, y);
                    p[0] > 200 && p[1] > 200 && p[2] > 200
                })
            })
            .count()
    };
    let rows = text_rows(
        &dc_frame,
        (reason.y * SCALE) as u32,
        ((reason.y + reason.height) * SCALE) as u32,
    );
    c.record(
        "p11.the_disconnect_reason_wraps_and_every_line_renders",
        lines.len() >= 2
            && centered
            && rows >= 8 * lines.len()
            && lines
                .iter()
                .all(|l| rewo_gpu::text::width(l, &advance) <= GUI_W - dc::REASON_MARGIN),
        format!(
            "{} wrapped line(s), {rows} rows of glyphs in a {}-px band, every \
             line within {} px",
            lines.len(),
            reason.height * SCALE,
            GUI_W - dc::REASON_MARGIN
        ),
    );

    // p12 — the pause screen's own frame: six buttons drawn, and the icon row's
    // cell is empty.
    let pause_frame = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        geometry(&pause),
        crate::live_cmd::screen_text_lines(&pause, &advance, SCALE as f32),
    )?;
    let pause_ok = match (pause.widget(ps::ICON_ROW), pause.widget(ps::SERVER_LINKS)) {
        (Some(row), Some(links_btn)) => {
            let cell_clear = (0..row.height)
                .flat_map(|dy| (0..row.width).map(move |dx| (dx, dy)))
                .all(|(dx, dy)| {
                    !is_button(px(
                        &pause_frame,
                        ((row.x + dx) * SCALE) as u32,
                        ((row.y + dy) * SCALE) as u32,
                    ))
                });
            is_button(px(
                &pause_frame,
                (links_btn.x * SCALE) as u32 + 2,
                (links_btn.y * SCALE) as u32 + 2,
            )) && cell_clear
        }
        _ => false,
    };
    c.record(
        "p12.the_pause_screen_draws_its_server_links_button_and_leaves_the_icon_cell_empty",
        pause_ok,
        "the packet's visible consequence, and the reserved cell beside it",
    );

    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("out dir: {e}"))?;
        for (name, buf) in [
            ("pause.png", &pause_frame),
            ("server_links.png", &dialog_frame),
            ("disconnected.png", &dc_frame),
            ("menu_background.png", &bg_only),
        ] {
            write_png(&dir.join(name), buf)?;
        }
        println!("[serverlinkshot] wrote 4 frames to {}", dir.display());
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
