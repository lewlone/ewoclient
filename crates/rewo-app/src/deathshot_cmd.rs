//! `rewo deathshot --check` — the M82 screen-framework + death-screen oracle.
//!
//! `player_combat_kill` (68) is `REWO_PACKET_COVERAGE.md` class **B**, and the
//! third of the four *screens* it lists. What this gate grades is therefore
//! two things at once: the packet, and the framework the other two screens
//! (`award_stats`, `server_links`) will land on.
//!
//! The path under test, end to end:
//!
//! ```text
//! a raw player_combat_kill body (built here)
//!   -> rewo_net::route_player_combat_kill   (with a REAL `Ids::resolve`d table)
//!   -> rewo_net::death_action               (handlePlayerCombatKill's own branch)
//!   -> rewo_world::death_screen::DeathScreen::build   (== `init()`)
//!   -> rewo_world::screen::Screen            (the routing + focus + tick model)
//!   -> live_cmd::{death_screen_chrome, death_screen_lines}  (the SAME builders
//!                                                            the frame path calls)
//!   -> WorldRenderer::{set_screen, set_text} -> ScreenPass / TextPass
//!   -> Offscreen::read_rgba                  (real pixels)
//! ```
//!
//! ## The rules this gate is built around, each earned elsewhere
//!
//! **The gate drives the real emitter.** M45's `install_shapes` failure and
//! M41's rotted `swingshot` fixture were both gates that had quietly stopped
//! testing their subject. So the router is called with a real `Ids` resolved
//! from the datagen report, `death_action` is the same function `PlaySession`
//! calls, and the two builders are the ones `LiveApp::frame` calls.
//!
//! **The detector must not share a colour with its background.** Fourteen
//! detector errors on this project are all one shape — measuring a signal
//! against a background that already contains it. A full-screen dark backdrop
//! makes that *worse*, because "is it dimmer" stops being a measurement. So
//! every pixel witness here renders over a **pure green** clear
//! (`0, 255, 0`), which nothing the death screen draws can produce: the
//! backdrop is red (`0x500000` → `0x803030`), the buttons are neutral greys,
//! the text is white and yellow. `p1` asserts an otherwise-identical frame
//! with no screen is *uniformly* that green, so every count below is the
//! screen and nothing else.
//!
//! **Predict, do not eyeball.** The backdrop's composite over a known
//! destination is arithmetic (`src·a + dst·(1−a)`, in linear space, because
//! the shader linearises the vertex colour and the attachment encodes on
//! store), so `p2` computes both stops and compares bytes rather than asking
//! whether the screen "looks red".
//!
//! **Sample on the boundary.** A click-delay threshold and a half-open hit
//! rect are exactly the shape M76's reordered clamp hid in, so `m3`/`m4`
//! sample at 19 *and* 20 ticks and `m5` samples at `right − 1` *and* `right`.
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
use rewo_proto::writer::PacketWriter;
use rewo_world::death_screen::{self as ds, DeathScreen};
use rewo_world::screen::{ButtonSprite, KeyResult, MouseResult, Screen, ScreenKind, Widget};

/// Total named properties this gate asserts. Locked so a skipped property
/// fails the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 40;

const W: u32 = 640;
const H: u32 = 480;
/// `gui_scale(640, 480)` = `min(480/240, 640/320)` = 2. Asserted by `p0`, not
/// assumed — every predicted rect below is multiplied by it.
const SCALE: i32 = 2;
const GUI_W: i32 = W as i32 / SCALE;
const GUI_H: i32 = H as i32 / SCALE;

/// The local player's entity id in every fixture. Deliberately not 0, so a
/// handler that compares against a default rather than the real id fails.
const LOCAL_ID: i32 = 4242;

#[derive(ClapArgs)]
pub struct DeathshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the convention `titleshot`/`eventshot`/`danceshot` use.
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
            "[deathshot] {}  {name}: {detail}",
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

pub fn run(args: DeathshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[deathshot] mode: {mode} (serverless; the oracle asserts \
         unconditionally). Model half against the 26.2 decompile; pixel half \
         against a pure-green clear no part of the screen can produce."
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
    check_wire(&mut c, &ids);
    check_model(&mut c, &baked);
    check_pixels(&mut c, &args, &baked)?;

    println!(
        "[deathshot] witnesses observed: {} / {}",
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
    println!("[deathshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ---------------------------------------------------------------------------
// The wire.
// ---------------------------------------------------------------------------

/// `ClientboundPlayerCombatKillPacket.STREAM_CODEC` — `VAR_INT playerId` then
/// `ComponentSerialization.TRUSTED_STREAM_CODEC message`, which is one NBT tag.
///
/// Built here rather than borrowed from the decoder, so the two cannot agree
/// by construction.
fn kill_body(player_id: i32, message: &str) -> Vec<u8> {
    let mut w = PacketWriter::default();
    w.varint(player_id);
    // A `Component` that is a bare string is `Codec.STRING` → `literal`, which
    // on the wire is network NBT: a bare tag byte with no name, then the
    // length-prefixed body. Tag 8 is String.
    w.buf.push(8);
    w.buf
        .extend_from_slice(&(message.len() as u16).to_be_bytes());
    w.buf.extend_from_slice(message.as_bytes());
    w.buf
}

fn check_wire(c: &mut Checker, ids: &Ids) {
    c.record(
        "w1.the_packet_resolves_by_name_from_the_datagen_report",
        ids.cb_play_player_combat_kill == 68,
        format!(
            "player_combat_kill -> {} (informational; the resolution is by \
             name, so a renumber moves this and still works)",
            ids.cb_play_player_combat_kill
        ),
    );

    // w2/w3: the whole production path, with an entity table that does not
    // exist at all.
    //
    // **This is the milestone's gotcha-13 witness.** `handlePlayerCombatKill`
    // is `level.getEntity(packet.playerId()) == minecraft.player`, and Rewo's
    // `EntityTable` never holds the local player — so the obvious
    // transcription resolves to `None` every time and does nothing, with every
    // other witness still green. M81's `hurt_animation` shipped exactly that
    // and was caught by a live server, not by its own twenty witnesses. This
    // one drives the router with **no table in scope whatsoever**, so a
    // table-based lookup could not even compile.
    let body = kill_body(LOCAL_ID, "Steve was slain by Zombie");
    let mut out = None;
    let handled = rewo_net::route_player_combat_kill(
        ids.cb_play_player_combat_kill,
        &body,
        ids,
        Some(LOCAL_ID),
        &mut out,
    );
    c.record(
        "w2.the_local_players_own_death_decodes_with_no_entity_table_at_all",
        handled
            && out.as_ref().is_some_and(|k| {
                k.player_id == LOCAL_ID && k.message.to_plain_text() == "Steve was slain by Zombie"
            }),
        format!(
            "handled={handled} kill={:?} — the id is resolved against the \
             local-player door, never a table (gotcha 13)",
            out.as_ref().map(|k| k.message.to_plain_text())
        ),
    );

    let mut out_no_door = None;
    let handled_no_door = rewo_net::route_player_combat_kill(
        ids.cb_play_player_combat_kill,
        &body,
        ids,
        None,
        &mut out_no_door,
    );
    c.record(
        "w3.dropping_the_local_player_door_makes_the_handler_do_nothing",
        handled_no_door && out_no_door.is_none(),
        "the mutation partner for w2: with no local id the packet is still \
         consumed and produces no death — which is precisely the silent \
         failure this witness exists to catch",
    );

    let mut other = None;
    let handled_other = rewo_net::route_player_combat_kill(
        ids.cb_play_player_combat_kill,
        &kill_body(LOCAL_ID + 1, "Someone else died"),
        ids,
        Some(LOCAL_ID),
        &mut other,
    );
    c.record(
        "w4.a_death_naming_another_player_is_consumed_and_produces_nothing",
        handled_other && other.is_none(),
        "vanilla's `if (player == this.minecraft.player)` drops it silently — \
         *handled* and *produced a kill* are separate answers, or a stray \
         combat-kill would fall through the rest of the dispatch chain",
    );

    let mut wrong = None;
    let handled_wrong = rewo_net::route_player_combat_kill(
        ids.cb_play_player_combat_kill + 1000,
        &body,
        ids,
        Some(LOCAL_ID),
        &mut wrong,
    );
    c.record(
        "w5.another_packets_id_is_not_handled_here",
        !handled_wrong && wrong.is_none(),
        "the router matches one id and returns false for everything else",
    );

    let mut trunc = None;
    let handled_trunc = rewo_net::route_player_combat_kill(
        ids.cb_play_player_combat_kill,
        &body[..2],
        ids,
        Some(LOCAL_ID),
        &mut trunc,
    );
    c.record(
        "w6.a_truncated_body_yields_no_death_rather_than_a_panic",
        handled_trunc && trunc.is_none(),
        "the packet is `isSkippable()` in vanilla; a short read is a dropped \
         screen, not a killed client",
    );

    // w7: `handlePlayerCombatKill`'s own branch, through the production
    // function `PlaySession` calls.
    let kill = out.clone().expect("w2 decoded");
    c.record(
        "w7.the_death_screen_is_shown_only_when_the_gamerule_allows_it",
        matches!(
            rewo_net::death_action(Some(kill.clone()), true),
            rewo_net::DeathAction::ShowScreen(_)
        ) && rewo_net::death_action(Some(kill), false) == rewo_net::DeathAction::RespawnNow
            && rewo_net::death_action(None, true) == rewo_net::DeathAction::None,
        "`shouldShowDeathScreen()` false takes `player.respawn()` and records \
         **nothing**, so no state downstream has to know the rule",
    );

    // w8: the two login booleans that were being read into a discard.
    //
    // Written against neighbours of the opposite value, so reading either off
    // the wrong byte fails here. This drives the same `read_login_prefix` the
    // session does — see `rewo_net::spawn_info`'s own test for the walk's
    // landing position.
    // w9: the outbound half. `LocalPlayer.respawn()` is
    // `send(new ServerboundClientCommandPacket(PERFORM_RESPAWN))`, whose codec
    // is `writeEnum` — the enum's **ordinal** as a VarInt.
    c.record(
        "w9.the_respawn_action_is_ordinal_zero_and_not_the_statistics_request",
        rewo_net::client_command_body(rewo_net::ClientCommand::PerformRespawn) == vec![0u8]
            && rewo_net::client_command_body(rewo_net::ClientCommand::RequestStats) == vec![1u8],
        "`PERFORM_RESPAWN, REQUEST_STATS, REQUEST_GAMERULE_VALUES` in that \
         order — ordinal 1 is a well-formed packet that opens the statistics \
         screen and does not respawn you, so the wrong one fails silently",
    );

    c.record(
        "w8.hardcore_and_show_death_screen_come_off_the_login_packet",
        rewo_net::login_flags(&login_prefix_body(true, false)) == Some((true, false))
            && rewo_net::login_flags(&login_prefix_body(false, true)) == Some((false, true)),
        "both were `r.bool()?;` discards before M82 — the third time this one \
         walk turned out to be reading something nothing consumed (after \
         M52c's latency and M67's two view distances)",
    );
}

/// A `ClientboundLoginPacket` prefix with the two flags this milestone reads.
fn login_prefix_body(hardcore: bool, show_death_screen: bool) -> Vec<u8> {
    let mut w = PacketWriter::default();
    w.i32(LOCAL_ID); // playerId — a big-endian int, not a VarInt
    w.bool(hardcore);
    w.varint(1); // levels
    w.string("minecraft:overworld");
    w.varint(20); // maxPlayers
    w.varint(12); // chunkRadius
    w.varint(10); // simulationDistance
    w.bool(!show_death_screen); // reducedDebugInfo — the opposite value, on purpose
    w.bool(show_death_screen);
    w.bool(!show_death_screen); // doLimitedCrafting — likewise
    w.buf
}

// ---------------------------------------------------------------------------
// The model. Every expectation below is an independent literal from the
// decompile, never a call into the code under test — a witness that asks the
// implementation what to expect asserts only that the implementation equals
// itself (`healthbarshot`'s rule, and M79's cooldown span is the worked
// example of getting it wrong).
// ---------------------------------------------------------------------------

fn labels() -> ds::DeathLabels {
    ds::DeathLabels {
        title: "You Died!".into(),
        respawn: "Respawn".into(),
        title_screen: "Title Screen".into(),
        score_template: "Score: %s".into(),
    }
}

fn built(hardcore: bool) -> (DeathScreen, ds::DeathLabels, Screen) {
    let model = DeathScreen {
        cause_of_death: Some("Steve was slain by Zombie".into()),
        hardcore,
        score: 0,
    };
    let labels = labels();
    let screen = model.build(&labels, GUI_W, GUI_H);
    (model, labels, screen)
}

fn check_model(c: &mut Checker, baked: &assets::BakedAssets) {
    let (_, _, screen) = built(false);

    // `this.width / 2 - 100`, `this.height / 4 + 72` and `+ 96`, 200x20.
    let r = screen.widget(ds::RESPAWN).unwrap();
    let t = screen.widget(ds::TITLE_SCREEN).unwrap();
    c.record(
        "m1.the_two_buttons_sit_where_the_decompile_puts_them",
        (r.x, r.y, r.width, r.height) == (60, 132, 200, 20)
            && (t.x, t.y, t.width, t.height) == (60, 156, 200, 20),
        format!(
            "respawn ({},{}) title ({},{}) at {GUI_W}x{GUI_H} GUI px",
            r.x, r.y, t.x, t.y
        ),
    );

    c.record(
        "m2.both_buttons_start_dead",
        screen.widgets.iter().all(|w| !w.active && w.visible),
        "`init()` ends with `setButtonsActive(false)` — an anti-misclick guard, \
         and without it the click that killed you respawns you",
    );

    // The boundary itself: `if (this.delayTicker == 20)`, incremented before
    // the test, so the twentieth tick arms them.
    let mut s19 = built(false).2;
    for _ in 0..19 {
        DeathScreen::tick(&mut s19);
    }
    let mut s20 = built(false).2;
    for _ in 0..20 {
        DeathScreen::tick(&mut s20);
    }
    c.record(
        "m3.the_guard_expires_on_the_twentieth_tick_and_not_the_nineteenth",
        s19.widgets.iter().all(|w| !w.active) && s20.widgets.iter().all(|w| w.active),
        format!(
            "ticks 19 -> active={:?}, ticks 20 -> active={:?} (sampled ON the \
             boundary: an off-by-one in either direction fails)",
            s19.widgets.iter().map(|w| w.active).collect::<Vec<_>>(),
            s20.widgets.iter().map(|w| w.active).collect::<Vec<_>>(),
        ),
    );

    // The click routes through `ContainerEventHandler.mouseClicked` ->
    // `getChildAt` -> `isMouseOver`, which is gated on `isActive()`.
    let mut a = s19.clone();
    let mut b = s20.clone();
    let inside = (r.x as f64 + 10.0, r.y as f64 + 10.0);
    c.record(
        "m4.a_click_before_the_guard_expires_is_not_even_consumed",
        a.mouse_clicked(inside.0, inside.1, 0) == MouseResult::Ignored
            && b.mouse_clicked(inside.0, inside.1, 0) == MouseResult::Pressed(ds::RESPAWN),
        "`getChildAt` uses `isMouseOver`, which tests `isActive()` — so a dead \
         button is invisible to the router rather than a no-op inside it",
    );

    // `areCoordinatesInRectangle` is half-open on the far edges.
    let mut e1 = s20.clone();
    let mut e2 = s20.clone();
    let mut e3 = s20.clone();
    let right = (r.x + r.width) as f64;
    let bottom = (r.y + r.height) as f64;
    c.record(
        "m5.the_hit_rectangle_is_half_open_on_the_far_edges",
        e1.mouse_clicked(right - 1.0, bottom - 1.0, 0) == MouseResult::Pressed(ds::RESPAWN)
            && e2.mouse_clicked(right, bottom - 1.0, 0) == MouseResult::Ignored
            && e3.mouse_clicked(right - 1.0, bottom, 0) == MouseResult::Ignored,
        format!(
            "x<{right} and y<{bottom} hit, == misses — sampled ON the bound, \
             which is where a `<=` would hide"
        ),
    );

    let mut rc = s20.clone();
    c.record(
        "m6.a_right_click_on_a_button_is_consumed_but_presses_nothing",
        rc.mouse_clicked(inside.0, inside.1, 1) == MouseResult::Consumed,
        "`ContainerEventHandler.mouseClicked` returns true whenever \
         `getChildAt` found something, whatever the child answered — so the \
         click never reaches the world",
    );

    // `WidgetSprites(button, button_disabled, button_highlighted)` is the
    // THREE-argument overload, which sets `disabledFocused = disabled`.
    let mut on = Widget::button(0, 0, 0, 200, 20, "x");
    let mut off = on.clone();
    off.active = false;
    on.active = true;
    c.record(
        "m7.a_disabled_button_shows_the_same_sprite_hovered_or_not",
        on.sprite(false, false) == ButtonSprite::Enabled
            && on.sprite(true, false) == ButtonSprite::Highlighted
            && on.sprite(false, true) == ButtonSprite::Highlighted
            && off.sprite(false, false) == ButtonSprite::Disabled
            && off.sprite(true, false) == ButtonSprite::Disabled
            && off.sprite(false, true) == ButtonSprite::Disabled,
        "the three-argument `WidgetSprites` constructor maps `disabledFocused` \
         onto `disabled`, so hovering a dead button changes nothing at all",
    );

    c.record(
        "m8.an_inactive_label_is_the_grey_withinactivemessage_gives_it",
        off.label_color() == [160.0 / 255.0; 3] && on.label_color() == [1.0; 3],
        "`Style.EMPTY.withColor(-6250336)` is 0xFFA0A0A0 — the label is a \
         second, independent tell that a button is dead",
    );

    // `defaultScrollingHelper`: `(top + bottom - 9) / 2 + 1`, centred between
    // `x + 2` and `x + width - 2`.
    let anchor = r.label_anchor(30);
    c.record(
        "m9.the_button_label_sits_one_pixel_below_the_true_vertical_centre",
        anchor == ((r.x + 2 + r.right() - 2) / 2, (r.y + r.bottom() - 9) / 2 + 1),
        format!(
            "({}, {}) — the `+ 1` is not decorative: without it a 9-px line in \
             a 20-px button sits a pixel high",
            anchor.0, anchor.1
        ),
    );

    let mut esc = s20.clone();
    let mut esc_ok = Screen::new(ScreenKind::Inventory, GUI_W, GUI_H);
    c.record(
        "m10.esc_cannot_dismiss_the_death_screen",
        esc.key_pressed(256, false) == KeyResult::Ignored
            && esc_ok.key_pressed(256, false) == KeyResult::Close
            && !screen.pause,
        "`shouldCloseOnEsc()` is false and `isPauseScreen()` is false — the \
         screen cannot be left except through its own buttons, and the world \
         keeps ticking behind it",
    );

    let mut tab = s20.clone();
    let mut order = Vec::new();
    for _ in 0..3 {
        tab.key_pressed(258, false);
        order.push(tab.focused());
    }
    c.record(
        "m11.tab_walks_the_widgets_and_wraps_through_a_cleared_focus",
        order == vec![Some(ds::RESPAWN), Some(ds::TITLE_SCREEN), Some(ds::RESPAWN)],
        format!(
            "{order:?} — the wrap is `Screen.keyPressed` retrying after \
             `clearFocus()`, not `handleTabNavigation` itself"
        ),
    );

    let mut sel = s20.clone();
    sel.focus(ds::TITLE_SCREEN);
    let mut dead_sel = s19.clone();
    dead_sel.focus(ds::TITLE_SCREEN);
    c.record(
        "m12.enter_space_and_keypad_enter_press_the_focused_button",
        [257, 32, 335].iter().all(|&k| {
            let mut s = sel.clone();
            s.key_pressed(k, false) == KeyResult::Pressed(ds::TITLE_SCREEN)
        }) && sel.clone().key_pressed(69, false) == KeyResult::Ignored
            && dead_sel.key_pressed(257, false) == KeyResult::Ignored,
        "`isSelection()` is ENTER || SPACE || KP_ENTER (not `isConfirmation`, \
         which drops SPACE), and `AbstractButton.keyPressed` tests \
         `isActive()` first",
    );

    let (_, _, hard) = built(true);
    let hm = DeathScreen {
        hardcore: true,
        ..Default::default()
    };
    c.record(
        "m13.the_two_hardcore_pairs_run_in_opposite_directions",
        hm.title_key() == "deathScreen.title.hardcore"
            && hm.respawn_key() == "deathScreen.spectate"
            && DeathScreen::default().title_key() == "deathScreen.title"
            && DeathScreen::default().respawn_key() == "deathScreen.respawn"
            && hard.widgets.len() == 2,
        "the hardcore *title* is the second key and the hardcore *button* is \
         the first — reading `hardcore ? a : b` off one and applying it to the \
         other swaps both",
    );

    // The two divisions in `output.accept(CENTER, middleLine / 2, 30, title)`
    // under a `withScale(2.0F)` pose.
    c.record(
        "m14.the_titles_anchor_truncates_twice_before_the_pose_doubles_it",
        ds::title_left(320, 51) == 2 * (320 / 2 / 2 - 51 / 2)
            && ds::title_left(320, 51) != 320 / 2 - 51
            && ds::title_top() == 60,
        format!(
            "left={} for an odd 51-px title, where the tidy `w/2 - width` \
             would give {}",
            ds::title_left(320, 51),
            320 / 2 - 51
        ),
    );

    c.record(
        "m15.the_backdrop_is_the_two_argb_literals_and_its_alpha_ramps",
        ds::BACKDROP.top == [80.0 / 255.0, 0.0, 0.0, 96.0 / 255.0]
            && ds::BACKDROP.bottom
                == [128.0 / 255.0, 48.0 / 255.0, 48.0 / 255.0, 160.0 / 255.0],
        "1615855616 = 0x60500000 and -1602211792 = 0xA0803030 — the alpha \
         changes down the screen as well as the colour, which a `tint it red` \
         reading would miss",
    );

    // `Screen.resize` -> `repositionElements` -> `rebuildWidgets` -> `init()`,
    // and `init()`'s first line is `this.delayTicker = 0`.
    let (model, labels, _) = built(false);
    let mut resized = s20.clone();
    resized = model.build(&labels, GUI_W * 2, GUI_H);
    c.record(
        "m16.resizing_while_dead_restarts_the_one_second_guard",
        resized.ticks == 0
            && resized.widgets.iter().all(|w| !w.active)
            && resized.widget(ds::RESPAWN).unwrap().x == GUI_W * 2 / 2 - 100,
        "vanilla's `init()` opens with `this.delayTicker = 0` and closes with \
         `setButtonsActive(false)`, so a window resize really does cost you \
         another second",
    );

    // The score line's two spans. `Component.translatable(key, literal(score)
    // .withStyle(YELLOW))` nests a styled literal inside an unstyled template.
    let spans = crate::live_cmd::score_spans("Score: %s", 7);
    c.record(
        "m17.the_score_value_is_yellow_inside_an_unstyled_template",
        spans.len() == 2
            && spans[0].text == "Score: "
            && spans[0].color == [1.0, 1.0, 1.0]
            && spans[1].text == "7"
            && spans[1].color == [1.0, 1.0, 85.0 / 255.0],
        format!(
            "{:?} — `ChatFormatting.YELLOW` is 0xFFFF55, and the word `Score:` \
             is not styled at all",
            spans.iter().map(|s| (&s.text, s.color)).collect::<Vec<_>>()
        ),
    );

    // The four keys against the real jar's language map, so a version that
    // renames one fails here rather than rendering the raw key.
    let lang = &baked.lang;
    c.record(
        "m18.the_four_lang_keys_resolve_against_the_real_jar",
        lang.get(ds::KEY_TITLE) == Some("You Died!")
            && lang.get(ds::KEY_TITLE_HARDCORE) == Some("Game Over!")
            && lang.get(ds::KEY_RESPAWN) == Some("Respawn")
            && lang.get(ds::KEY_SPECTATE) == Some("Spectate World")
            && lang.get(ds::KEY_TITLE_SCREEN) == Some("Title Screen")
            && lang.get(ds::KEY_SCORE_VALUE) == Some("Score: %s"),
        "resolved through `rewo_data::lang`, which applies `deprecated.json` — \
         a rename would land here",
    );

    // `WidgetSprites`' three sheets, as the bake found them.
    let widgets = baked.widgets.as_ref();
    c.record(
        "m19.the_three_button_sheets_are_two_hundred_by_twenty_and_opaque",
        widgets.is_some_and(|w| {
            [&w.button, &w.button_disabled, &w.button_highlighted]
                .iter()
                .all(|s| s.w == 200 && s.h == 20 && s.rgba.chunks(4).all(|p| p[3] == 255))
        }),
        "eight-bit greyscale with no alpha channel, so a button covers its \
         whole rect — and the nine-slice `.mcmeta` (border 3) degenerates to a \
         1:1 blit at exactly `Button.BIG_WIDTH`",
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

/// The exact composite of one backdrop stop over the clear colour.
///
/// The pass linearises its sRGB vertex colour in the fragment shader, the
/// fixed-function blend is `SRC_ALPHA / ONE_MINUS_SRC_ALPHA` in linear space,
/// and the sRGB attachment encodes on store. So the prediction is arithmetic
/// rather than an eyeball, and it is `container`'s convention exactly — this
/// pass shares that pipeline.
fn over_clear(stop: [f32; 4], clear: [f32; 3]) -> [u8; 3] {
    let a = stop[3];
    std::array::from_fn(|i| {
        let src = srgb_decode((stop[i] * 255.0 + 0.5) as u8);
        srgb_encode(src * a + clear[i] * (1.0 - a))
    })
}

fn check_pixels(
    c: &mut Checker,
    args: &DeathshotArgs,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[deathshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("deathshot: Vulkan validation requested but not active".into());
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
        // Pushed off-screen: this gate measures the screen, and a strip chart
        // in the corner would sit inside the backdrop.
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    };
    let vp = glam::Mat4::IDENTITY.to_cols_array_2d();
    // A pure green destination. Nothing the death screen draws is green: the
    // backdrop is red, the buttons are neutral greys, the text is white and
    // yellow. `clear` is a float32 clear on an sRGB attachment, so 1.0 stores
    // as 255 exactly.
    const CLEAR_LINEAR: [f32; 3] = [0.0, 1.0, 0.0];
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
    let all_green = (0..W * H).all(|i| {
        let p = &empty[(i * 4) as usize..];
        p[0] == 0 && p[1] == 255 && p[2] == 0
    });
    c.record(
        "p1.a_frame_with_no_screen_is_uniformly_the_clear_colour",
        all_green,
        "so every count below is the screen and nothing else — `handshot`'s \
         rule after fourteen detector errors of the same shape",
    );

    // The live view: the same builders the frame path calls, on a screen
    // whose guard has expired.
    let (model, dlabels, mut screen) = built(false);
    for _ in 0..ds::BUTTON_DELAY_TICKS {
        DeathScreen::tick(&mut screen);
    }
    let view = crate::live_cmd::DeathView {
        model: model.clone(),
        labels: dlabels.clone(),
        cause: Some(vec![rewo_net::chat_style::ChatSpan {
            text: "Steve was slain by Zombie".into(),
            color: [1.0, 1.0, 1.0],
            bold: false,
            italic: false,
            underlined: false,
            strikethrough: false,
            obfuscated: false,
        }]),
        respawn_epoch: 0,
    };
    let screen_px = (W as f32, H as f32);
    let live = |screen: &Screen, mouse: Option<(f64, f64)>| {
        (
            crate::live_cmd::screen_chrome(screen, mouse),
            crate::live_cmd::death_screen_lines(
                &view,
                screen,
                &advance,
                SCALE as f32,
                screen_px,
            ),
        )
    };

    let (chrome, lines) = live(&screen, None);
    let frame = shot(&mut gpu, &mut off, &mut wr, chrome, lines)?;

    // p2: the backdrop's two stops, predicted exactly. Row 0 is the top stop
    // and row H-1 is the bottom, and the sample column is clear of both
    // buttons and all four text runs.
    //
    // **The two stops are re-declared here rather than read from
    // `ds::BACKDROP`,** and the mutation battery is what made that necessary:
    // the first version predicted from the very constant it was grading, so
    // swapping `col1` and `col2` swapped the prediction *and* the render and
    // the witness stayed green. That is `healthbarshot`'s rule — a witness
    // that asks the implementation what to expect asserts only that the
    // implementation equals itself — and it is the self-calibrating shape M80
    // records.
    const DEATH_TOP: [f32; 4] = [80.0 / 255.0, 0.0, 0.0, 96.0 / 255.0];
    const DEATH_BOTTOM: [f32; 4] = [128.0 / 255.0, 48.0 / 255.0, 48.0 / 255.0, 160.0 / 255.0];
    let top_pred = over_clear(DEATH_TOP, CLEAR_LINEAR);
    let bot_pred = over_clear(DEATH_BOTTOM, CLEAR_LINEAR);
    let top_seen = px(&frame, 8, 0);
    let bot_seen = px(&frame, 8, H - 1);
    let near = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 2);
    c.record(
        "p2.the_backdrops_two_stops_composite_exactly_as_predicted",
        near(top_seen, top_pred) && near(bot_seen, bot_pred),
        format!(
            "top {top_seen:?} vs {top_pred:?}, bottom {bot_seen:?} vs \
             {bot_pred:?} — arithmetic over a known destination, not `is it \
             red`; swapping col1/col2 fails both"
        ),
    );

    c.record(
        "p3.the_backdrop_grows_more_opaque_downward",
        top_seen[1] > bot_seen[1] && top_seen[0] < bot_seen[0],
        format!(
            "green {} -> {} and red {} -> {}: alpha 96/255 at the top and \
             160/255 at the bottom, so more of the destination shows through \
             above",
            top_seen[1], bot_seen[1], top_seen[0], bot_seen[0]
        ),
    );

    // p4: the buttons land exactly on their predicted rects. Sampled ON the
    // boundary — one pixel inside each edge must be button, one pixel outside
    // must be backdrop.
    let r = screen.widget(ds::RESPAWN).unwrap();
    let (bx0, by0) = ((r.x * SCALE) as u32, (r.y * SCALE) as u32);
    let (bx1, by1) = (
        ((r.x + r.width) * SCALE) as u32,
        ((r.y + r.height) * SCALE) as u32,
    );
    // The buttons are the only **greyscale** thing on screen: all three sheets
    // are 8-bit greyscale, so every button texel has `r == g == b`, and the
    // backdrop over a green destination never does (red runs 49→103, green
    // 207→168 and blue 0→37 down the screen, and the three never coincide).
    //
    // The first version of this predicate asked for `r > g` outside, on the
    // reflex that "the backdrop is red". It is red *over black*; over a green
    // destination it is green-dominant, and the witness failed. A fifteenth
    // instance of the same detector error — measuring against an assumed
    // background rather than the one actually behind the subject.
    let is_button = |p: [u8; 3]| p[0] == p[1] && p[1] == p[2];
    let is_backdrop = |p: [u8; 3]| !is_button(p);
    c.record(
        "p4.the_button_covers_exactly_the_rect_the_layout_predicted",
        is_button(px(&frame, bx0, by0))
            && is_button(px(&frame, bx1 - 1, by1 - 1))
            && is_backdrop(px(&frame, bx0 - 1, by0 + 4))
            && is_backdrop(px(&frame, bx1, by0 + 4))
            && is_backdrop(px(&frame, bx0 + 4, by0 - 1))
            && is_backdrop(px(&frame, bx0 + 4, by1)),
        format!(
            "({bx0},{by0})..({bx1},{by1}) in screen px — greyscale inside, \
             non-grey backdrop one pixel outside every edge"
        ),
    );

    // p5..p7: the sprite table, in pixels.
    //
    // `button.png`'s top row is greyscale 0 and `button_highlighted.png`'s is
    // 255, so the border alone tells the two apart — and `button_disabled`'s
    // interior tops out at 52 where `button`'s sits at ~111.
    let border_row = |buf: &[u8]| px(buf, bx0 + 20, by0);
    let interior_max = |buf: &[u8], x0: u32, y0: u32, x1: u32, y1: u32| {
        let mut m = 0u8;
        for y in y0..y1 {
            for x in x0..x1 {
                m = m.max(px(buf, x, y).iter().copied().max().unwrap());
            }
        }
        m
    };

    let hover = (
        (r.x as f64 + 20.0) * SCALE as f64,
        (r.y as f64 + 10.0) * SCALE as f64,
    );
    let (chrome_h, lines_h) = live(&screen, Some((hover.0 / SCALE as f64, hover.1 / SCALE as f64)));
    let hovered = shot(&mut gpu, &mut off, &mut wr, chrome_h, lines_h)?;
    c.record(
        "p5.hovering_an_enabled_button_swaps_it_to_the_white_bordered_sprite",
        border_row(&frame) == [0, 0, 0] && border_row(&hovered) == [255, 255, 255],
        format!(
            "unhovered border {:?}, hovered {:?} — `widget/button`'s frame is \
             greyscale 0 and `widget/button_highlighted`'s is 255",
            border_row(&frame),
            border_row(&hovered)
        ),
    );

    // The same two frames on a screen whose guard has NOT expired.
    let dead = built(false).2;
    let (chrome_d, lines_d) = live(&dead, None);
    let dead_frame = shot(&mut gpu, &mut off, &mut wr, chrome_d, lines_d)?;
    let (chrome_dh, lines_dh) = live(
        &dead,
        Some((hover.0 / SCALE as f64, hover.1 / SCALE as f64)),
    );
    let dead_hovered = shot(&mut gpu, &mut off, &mut wr, chrome_dh, lines_dh)?;
    c.record(
        "p6.a_hovered_disabled_button_is_byte_identical_to_an_unhovered_one",
        dead_frame == dead_hovered,
        "the pixel form of the three-argument `WidgetSprites` constructor: \
         `disabledFocused == disabled`. Mapping (inactive, hovered) to \
         `button_highlighted` instead fails this and only this",
    );

    // Interior, excluding the 3-px border, and excluding the label band.
    let inner = |buf: &[u8]| {
        interior_max(
            buf,
            bx0 + 8,
            by0 + 4,
            bx0 + 30,
            by1 - 4,
        )
    };
    c.record(
        "p7.the_disabled_sprite_is_much_darker_than_the_enabled_one",
        inner(&dead_frame) < 60 && inner(&frame) > 100,
        format!(
            "disabled max {} vs enabled max {} — the sheets' own histograms \
             top out at 52 and 172",
            inner(&dead_frame),
            inner(&frame)
        ),
    );

    // p8: the label's colour, measured where the sprite cannot reach.
    //
    // On `button_disabled` the sheet's brightest texel is 52, so the label is
    // unmistakable — and so is its absence.
    //
    // **`0xA0A0A0` reaches the framebuffer as `srgb_encode(160/255)` = 208,
    // not 160.** Rewo's text pass writes its colour straight to an sRGB
    // attachment, so the number it is handed is treated as *linear*. That is a
    // pre-existing Rewo-wide convention — the chat, the F3 block, the titles
    // and the HUD all share it — and a real deviation from vanilla, which
    // composites GUI text in gamma space. Recorded here rather than fixed:
    // changing it would move every text pixel in every existing gate.
    let label_band = |buf: &[u8]| interior_max(buf, bx0 + 6, by0 + 8, bx1 - 6, by1 - 8);
    let dead_label = srgb_encode(160.0 / 255.0);
    c.record(
        "p8.an_active_label_is_white_and_a_dead_one_is_the_inactive_grey",
        label_band(&frame) == 255 && label_band(&dead_frame) == dead_label,
        format!(
            "active max {} (white), dead max {} == srgb_encode(160/255), over \
             a sheet whose own maximum is 52 — colouring the dead label white \
             instead pushes the second number to 255",
            label_band(&frame),
            label_band(&dead_frame)
        ),
    );

    // p9: the title is drawn at TITLE_SCALE, so its glyph band is twice as
    // tall as the score's. Text is the only thing here that can be white.
    let white_rows = |buf: &[u8], y0: u32, y1: u32| {
        (y0..y1)
            .filter(|&y| {
                (0..W).any(|x| {
                    let p = px(buf, x, y);
                    p[0] > 200 && p[1] > 200 && p[2] > 200
                })
            })
            .count()
    };
    let title_rows = white_rows(
        &frame,
        (ds::title_top() * SCALE) as u32,
        ((ds::title_top() + 8 * ds::TITLE_SCALE) * SCALE) as u32,
    );
    let cause_rows = white_rows(&frame, (85 * SCALE) as u32, ((85 + 8) * SCALE) as u32);
    c.record(
        "p9.the_title_is_drawn_at_double_scale_where_the_cause_is_not",
        title_rows >= 24 && (12..=16).contains(&cause_rows),
        format!(
            "{title_rows} white rows in the title's 32-px band vs \
             {cause_rows} in the cause's 16-px one — `withScale(2.0F)` \
             doubles the glyph, it does not merely move it"
        ),
    );

    // p10: the score's value is yellow where its template is white. Rewo's
    // text pass writes its colour straight to an sRGB attachment, so 0x55
    // stores as `srgb_encode(85/255)`; that is a pre-existing Rewo-wide
    // convention, not this milestone's, and the prediction accounts for it.
    let blue_of_yellow = srgb_encode(85.0 / 255.0);
    let score_band = ((100 * SCALE) as u32, ((100 + 8) * SCALE) as u32);
    let mut saw_yellow = false;
    let mut saw_white = false;
    for y in score_band.0..score_band.1 {
        for x in 0..W {
            let p = px(&frame, x, y);
            if p[0] > 200 && p[1] > 200 {
                if p[2].abs_diff(blue_of_yellow) <= 3 {
                    saw_yellow = true;
                } else if p[2] > 200 {
                    saw_white = true;
                }
            }
        }
    }
    c.record(
        "p10.the_score_value_renders_yellow_beside_a_white_template",
        saw_yellow && saw_white,
        format!(
            "expected blue channel {blue_of_yellow} for 0xFFFF55; yellow={} \
             white={} — colouring the value white loses the first",
            saw_yellow, saw_white
        ),
    );

    // p11: the nine-slice, in pixels — and **three milestones have now written
    // this witness**.
    //
    // M82 shipped a 1:1 blit and asserted that a button of any other size was
    // *skipped*. M85 transcribed the two branches that resize horizontally and
    // moved the skip assertion to a button whose *height* is not the sheet's.
    // M84 transcribed all four, because a 6x32 scroller drawn at 6x35 is a
    // vertical resize and a 130x24 tab sheet at 98 needs an asymmetric border —
    // neither of which M85's `SPRITE_W`-and-`NINE_SLICE_BORDER` form could say.
    // There is one implementation now and no size is skipped, so the property
    // worth grading is what a nine-slice *is*: the sheet's own 3-px borders,
    // unscaled, on **both** axes.
    //
    // The control is the 200x20 button, where the nine-slice degenerates to the
    // 1:1 blit M82 shipped, so a border that matches it is the sheet's own. A
    // skip would be identical to the bare backdrop; a stretch would resample.
    let button_at = |width: i32, height: i32| rewo_gpu::screen::ScreenDraw {
        backdrop: Some((ds::BACKDROP.top, ds::BACKDROP.bottom)),
        buttons: vec![rewo_gpu::screen::ButtonDraw {
            x: 60,
            y: 132,
            width,
            height,
            sprite: rewo_gpu::screen::ButtonSprite::Enabled,
        }],
        ..Default::default()
    };
    let backdrop_only = rewo_gpu::screen::ScreenDraw {
        backdrop: Some((ds::BACKDROP.top, ds::BACKDROP.bottom)),
        ..Default::default()
    };
    let odd_frame = shot(&mut gpu, &mut off, &mut wr, button_at(150, 24), Vec::new())?;
    let control = shot(&mut gpu, &mut off, &mut wr, button_at(200, 20), Vec::new())?;
    let bare = shot(&mut gpu, &mut off, &mut wr, backdrop_only, Vec::new())?;
    let border_px = 3 * SCALE as u32;
    // **Every sample is a 3-GUI-px corner block**, never a whole column or a
    // whole row. The first version compared a full column of each button, and
    // the two are different heights, so the `Vec`s could never be equal — and a
    // full row would have run into the narrow button's *right* border at GUI x
    // 147 while the control was still inside its tiled middle. A corner is the
    // only span the two agree about by construction.
    let block = |buf: &[u8], x: u32, y0: u32| -> Vec<[u8; 3]> {
        (y0..y0 + border_px).map(|y| px(buf, x, y)).collect()
    };
    let odd_right = (60 + 150) as u32 * SCALE as u32;
    let odd_bottom = (132 + 24) as u32 * SCALE as u32;
    let ctl_right = (60 + 200) as u32 * SCALE as u32;
    let ctl_bottom = (132 + 20) as u32 * SCALE as u32;
    // Horizontal: the top-left corner is at the same x in both; the top-right
    // is measured back from each button's own right edge.
    let left_same =
        (0..border_px).all(|d| block(&odd_frame, bx0 + d, by0) == block(&control, bx0 + d, by0));
    let right_same = (1..=border_px)
        .all(|d| block(&odd_frame, odd_right - d, by0) == block(&control, ctl_right - d, by0));
    // Vertical — the branch M85 could not reach. The bottom-left corner of a
    // 24-tall button is the sheet's own rows 17..20, exactly as it is on a
    // 20-tall one, even though four rows of tiled middle sit above it that do
    // not exist on the control.
    let bottom_same = (0..border_px).all(|d| {
        block(&odd_frame, bx0 + d, odd_bottom - border_px)
            == block(&control, bx0 + d, ctl_bottom - border_px)
    });
    c.record(
        "p11.a_button_at_neither_the_sheets_width_nor_its_height_is_nine_sliced",
        odd_frame != bare && left_same && right_same && bottom_same,
        "150x24 out of a 200x20 sheet: the 3-px borders survive unscaled on          both axes. A skip would leave the bare backdrop, a stretch would          resample all three, and M85's horizontal-only transcription would          fail `bottom_same` by drawing nothing at all",
    );

    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("out dir: {e}"))?;
        for (name, buf) in [
            ("death_live.png", &frame),
            ("death_hovered.png", &hovered),
            ("death_guard.png", &dead_frame),
        ] {
            write_png(&dir.join(name), buf)?;
        }
        println!("[deathshot] wrote 3 frames to {}", dir.display());
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
