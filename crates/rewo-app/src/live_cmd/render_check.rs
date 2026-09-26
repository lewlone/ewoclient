use super::*;

/// How many seconds `--render-check` runs for when `--run-seconds` is absent.
///
/// Long enough for the server to send the inventory and for the weather /
/// particle paths to have been reached many times over, short enough to be a
/// gate rather than a soak.
pub(super) const RENDER_CHECK_SECONDS: f32 = 8.0;

/// `ChatFormatting.GRAY`'s 0xAAAAAA, in the LINEAR space the text pass wants.
///
/// Spelled out rather than taken from `parse_color` + `srgb_to_linear`,
/// because a witness that computes its expectation from the functions under
/// test is self-calibrating (§0.0 gotcha 0a) — it would pass with a wrong
/// palette entry and with a wrong transfer function alike.
///
/// **The first version of r41 compared against the sRGB value and read 0 with
/// the feature working**, which is this codebase's own open item (the title
/// path and the death screen hand the text pass `/255` bytes where it wants
/// linear) reproduced inside a new witness. The chat path converts correctly;
/// the witness did not.
///
/// **The second version read 0 too, and for a different reason worth keeping.**
/// It compared bits. The renderer evaluates the transfer function in `f32` at
/// every step and reaches `0.40197787`; the same formula in `f64`, narrowed
/// once at the end, reaches `0.40197778`. Those are different `f32`s. A
/// bit-exact comparison across two evaluation orders of one formula is a
/// **wrong** assertion rather than a strict one — the same shape as M12's
/// finding that JOML's `Math.fma` is non-fused by default and so `lengthSquared`
/// must be read right-associatively. The tolerance below is ~40x tighter than
/// one 8-bit colour step (1/255) and ~1000x looser than the gap above, so it
/// cannot admit a wrong palette entry or a missing conversion.
pub(super) const GRAY_LINEAR: f32 = 0.401_977_78;

/// Well under one 8-bit colour step, well over the f32/f64 evaluation gap.
pub(super) const COLOR_EPS: f32 = 1e-4;
/// r48 — the entity ids `--render-check`'s crowd injection creates.
///
/// Two zombies and a villager, spawned through the production `add_entity`
/// route. They are named here rather than written at the injection site
/// because **the census joins on them**: the two crowd counts are taken over
/// exactly these three draws and nothing else in the frame, so that a fixture
/// elsewhere in the same run which happens to spawn a zombie cannot satisfy
/// r48's threshold on this check's behalf. Far above any id a short
/// render-check session's server will hand out, and far from any real profile.
pub(super) const CROWD_IDS: [i32; 3] = [0x5A00, 0x5A01, 0x5A02];

/// M128 — the text of the one clickable message `--render-check` injects.
///
/// Deliberately nothing a server would say, so the drawn-line scan that finds
/// it cannot match a real message.
pub(super) const CLICK_WITNESS_TEXT: &str = "rewo click witness";

/// The command that message's `click_event` runs, **without** its leading
/// slash — `Commands.trimOptionalPrefix` strips one, and the witness asserts
/// the trimmed form, so a dispatcher that forgot to trim fails here.
pub(super) const CLICK_WITNESS_COMMAND: &str = "rewoclickwitness";

/// The `system_chat` body carrying that message, as network NBT.
///
/// Hand-assembled because `rewo_proto::nbt` reads and does not write, and
/// because a *compound* is required: a `click_event` has nowhere to live on a
/// bare string component, which is what every other injected witness uses.
///
/// ```text
/// TAG_Compound
///   TAG_String "text"        = "rewo click witness"
///   TAG_Compound "click_event"
///     TAG_String "action"    = "run_command"
///     TAG_String "command"   = "/rewoclickwitness"
///   TAG_End
/// TAG_End
/// ```
///
/// then the `overlay` boolean the packet's second field is.
pub(super) fn click_witness_body() -> Vec<u8> {
    fn string(out: &mut Vec<u8>, name: &str, value: &str) {
        out.push(8); // TAG_String
        out.extend_from_slice(&(name.len() as u16).to_be_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        out.extend_from_slice(value.as_bytes());
    }
    let mut body: Vec<u8> = vec![10]; // TAG_Compound, unnamed at the root
    string(&mut body, "text", CLICK_WITNESS_TEXT);
    body.push(10);
    body.extend_from_slice(&(11u16).to_be_bytes());
    body.extend_from_slice(b"click_event");
    string(&mut body, "action", "run_command");
    // With the slash, so the trim is exercised rather than assumed.
    string(&mut body, "command", &format!("/{CLICK_WITNESS_COMMAND}"));
    body.push(0); // end of click_event
    body.push(0); // end of root
    body.push(0); // overlay = false
    body
}

/// M166 — the UUID r55's PLAY-state `resource_pack_push` injection carries.
///
/// Deliberately not the one `render_check.py` stages for the configuration
/// task: r55 asserts the two replies in order, so a client that answered only
/// the configuration push, or answered the same id twice, fails on the id
/// rather than on a count.
pub(super) const PLAY_PACK_ID: u128 = 0x00c0_ffee_0000_4000_8000_0000_0bad_f00d;

/// The entity id r48's injected zombie is given.
///
/// Far outside anything a server assigns in a short session, so it cannot
/// collide with a real entity and make the witness read someone else's name.
pub(super) const NAMETAG_WITNESS_ID: i32 = 0x0052_5701;

/// Where r49's injected sign is placed, relative to the player's block.
///
/// Two east and one up, so it sits in air rather than replacing whatever the
/// player is standing on — a sign whose block is overwritten by the server's
/// next chunk update would make the witness flap.
pub(super) const SIGN_WITNESS_OFFSET: (i32, i32, i32) = (2, 1, 0);

/// The translation key r49's sign carries.
pub(super) const SIGN_WITNESS_KEY: &str = "block.minecraft.dirt";

/// What `en_us.json` says that key is.
///
/// A literal rather than a `baked.lang` lookup ON PURPOSE: a witness that asks
/// the table under test what to expect grades everything except the table
/// (§0.0 gotcha 0a). This is the jar's own answer, read once and written down.
pub(super) const SIGN_WITNESS_RESOLVED: &str = "Dirt";

/// What `live --render-check` observed (M86).
///
/// Every field is a **count of frames on which something happened**, never a
/// snapshot, because the failure this gate exists for is "the branch never ran
/// at all" and a snapshot taken on the wrong frame cannot tell that from "it
/// ran and had nothing to do".
#[derive(Default)]
pub(super) struct RenderCheck {
    pub(super) frames: u64,
    /// M168 — the most ARMOUR icons the survival layout emitted in any
    /// frame, and how many of them were FULL. `render_check.py` stages an
    /// iron chestplate (`item replace entity @s armor.chest`), so the
    /// server has to mark the player's own ARMOR attribute dirty and send
    /// it to the player itself — `sendToTrackingPlayersAndSelf`, the path
    /// the initial pairing never takes — for this to move off zero. A
    /// count AND a value: armour draws ten icons whenever it draws any,
    /// so the count alone is "some attribute arrived", and the three
    /// full ones are `6 / 2` of an iron chestplate specifically.
    pub(super) armor_icons_max: usize,
    pub(super) armor_full_max: usize,
    /// M168 — the most EFFECT icons emitted in any frame, and whether one
    /// of them sat on the beneficial row at the first slot. The staged
    /// `effect give @s minecraft:speed infinite 0` — WITHOUT the
    /// `hideParticles` argument: `/effect give` builds its instance with the
    /// five-argument constructor, whose `visible` doubles as `showIcon`
    /// (`MobEffectInstance.java:61`), so hiding the particles hides the
    /// icon the layout gates on. The first staging did, and measured zero.
    pub(super) effect_icons_max: usize,
    pub(super) effect_first_beneficial: bool,
    /// M169 — the jump bar's whole chain, live: the most JumpBar blits in
    /// any frame (2 = background + progress while charging), the highest
    /// `getJumpRidingScale()` seen, and the `START_RIDING_JUMP`s sent.
    /// `render_check.py` summons a tamed, saddled horse, mounts the bot on
    /// it, and the run holds Space over 0.32..0.46 of its length (clear of the
    /// 0.5 inventory), which charges the meter to 1.0 and releases it.
    pub(super) jump_bar_blits_max: usize,
    pub(super) leash_verts_max: usize,
    pub(super) book_frames: u64,
    pub(super) book_pages_seen: usize,
    pub(super) options_slider_sprites_max: usize,
    pub(super) jump_scale_max: f32,
    pub(super) riding_jumps_sent: u64,
    /// `self.baked.is_some()` observed at the top of a frame. The witness whose
    /// absence let the bug live from M3 to M86.
    pub(super) baked_frames: u64,
    /// Frames on which the rain-fog band came from the bake rather than from
    /// the `None` sentinel `[1e9, 1e9 + 1]`. A *value* witness, not a call
    /// count: the sentinel is what the dead branch produced, so a finite band
    /// cannot be faked by merely entering the arm.
    pub(super) fog_band_frames: u64,
    pub(super) gui_item_frames: u64,
    pub(super) hand_frames: u64,
    pub(super) weather_frames: u64,
    /// Frames on which the inventory screen was drawn. The gate opens it
    /// halfway through, so this is roughly half the run — it exists to prove
    /// the screen path (and with it `VelvetTextPass::sync_atlas`) was reached
    /// at all, not to pin a count.
    pub(super) screen_frames: u64,
    /// Frames on which a CONTAINER's screen was drawn — a menu other than the
    /// player's own (M88).
    ///
    /// M87 shipped the container render and could not prove the windowed
    /// client reached it: `--render-check` opens the *inventory*, which is
    /// `menu_layout::PLAYER` and takes the pass's own `inventory.png` rect, so
    /// every container-specific path — the panel blits, the container-sized
    /// origin, the layout's own slot rects — stayed unexercised here. That is
    /// the exact shape of the blind spot M86 was: a path nothing drives.
    pub(super) container_frames: u64,
    /// M94 — the most quads the recipe book drew in any frame **with no
    /// which-of-these overlay open**. Split from the field below by M104 so
    /// the two are comparable; r23's threshold is a floor, so its meaning is
    /// unchanged.
    pub(super) book_quads_max: usize,
    /// M132 — frames on which the SCOREBOARD SIDEBAR put text on the list.
    ///
    /// The gate injects an objective, three scores and a display-slot
    /// assignment through the production router, so this grades the whole
    /// chain: the three packets decode into `Scoreboard`, `select_objective`
    /// finds the slot, the rows resolve and format, and the layout places
    /// them. A break anywhere drops it to zero. It is a *text* count rather
    /// than a fill count because the two bands are drawn unconditionally once
    /// a sidebar exists, while a row's glyphs need every stage above.
    pub(super) sidebar_frames: u64,
    /// The most text lines the sidebar drew in any frame.
    ///
    /// Separate from the frame count because a sidebar with a title and no
    /// rows still scores `sidebar_frames` — this is what says the *scores*
    /// reached the renderer. Three injected rows with a name and a number
    /// each, plus the title, is seven.
    pub(super) sidebar_text_max: usize,
    /// M151 — frames on which the TAB LIST was drawn in the windowed client.
    ///
    /// The M86 shape a second time: `rewo_gpu::tab_list` had 41 passing tests
    /// and no caller at all, so every other witness in this file is satisfied
    /// by a client whose Tab key does nothing. The gate holds the key down for
    /// the second half of the run, so this must be non-zero and must be
    /// **less** than `frames` — a client that drew the list unconditionally
    /// would score every frame and is exactly as wrong as one that never drew
    /// it.
    pub(super) tab_list_frames: u64,
    /// The most rows the layout PLACED in any frame.
    ///
    /// Separate from the frame count because the bands are drawn as soon as a
    /// view exists: this is what says the listed-player filter, the sort and
    /// the column solve all produced slots. The run injects a set of players,
    /// one of them unlisted, so the count is a claim about the filter and not
    /// merely about arrival.
    pub(super) tab_list_rows_max: usize,
    /// The most text lines the tab list emitted, and the most ping icons.
    ///
    /// Two separate counts because they come from two different lists —
    /// `set_text` and `set_hud_icons` — and a break in either leaves the other
    /// healthy. The icons are the first `HudBlit` anything produces.
    pub(super) tab_list_text_max: usize,
    pub(super) tab_list_icons_max: usize,
    /// M163 — frames on which a mob's nametag carried a RESOLVED translatable.
    ///
    /// The run injects a zombie whose `Entity.DATA_CUSTOM_NAME` is
    /// `{translate: "entity.minecraft.zombie"}`, and this counts the frames on
    /// which the label the renderer was handed reads `Zombie`.
    ///
    /// Every stage has to work: the raw body reaches
    /// `route_set_entity_data`, `MetaKinds::lang` carries the table the app
    /// installed on `PlaySession`, `chat_style::flatten` resolves the key,
    /// `EntityTable::custom_name` stores it and `resolve_labels` hands it to
    /// `EntityDraw::name`. Before M163 the decode called `to_plain_text`, which
    /// has no table.
    /// M166 — what the two blocking configuration tasks asked for, mirrored
    /// off the `PlaySession`.
    ///
    /// Mirrored rather than counted: a counter is satisfied by any client that
    /// reached the arm, including one replying with a zero UUID, whereas the
    /// server stages a **known** pack id and code-of-conduct text and this
    /// carries what came off the wire.
    pub(super) config_tasks: rewo_net::config_tasks::ConfigTaskLog,
    pub(super) nametag_resolved_frames: u64,
    /// The negative half, and the reason a `> 0` counter alone is not enough.
    ///
    /// r26 scored full marks while chat drew `multiplayer.player.joined`, so a
    /// witness that only counts "a name was drawn" cannot see this bug at all.
    /// This counts frames on which the label was the RAW KEY, and must be zero.
    pub(super) nametag_raw_key_frames: u64,
    /// M163 — frames on which a SIGN line carried a RESOLVED translatable.
    ///
    /// The sibling of `nametag_resolved_frames`, and it exists because the two
    /// halves of that feature fail independently. The nametag resolves at
    /// DECODE (`MetaKinds::lang`); a sign resolves in the FRAME LOOP, because
    /// `BlockEntity::sign_text` is called per sign per frame — so r48 proves
    /// nothing at all about this path.
    ///
    /// A review demonstrated the gap by changing both of the app's
    /// `collect_sign_text` call sites to pass no table, with
    /// `blockentityshot` still 179/179: `sg1`/`sg2` hand the collector a table
    /// of their own, so they grade the function and never ask who calls it.
    /// [`collect_session_sign_text`] now makes that particular edit a compile
    /// error, and this counts the frames on which the line the renderer was
    /// handed actually reads `Dirt`.
    pub(super) sign_resolved_frames: u64,
    /// The negative half — frames on which the sign line was the RAW KEY.
    ///
    /// Truncated, and deliberately matched as a PREFIX: `getRenderMessages`
    /// splits each line against the board and keeps fragment 0, so
    /// `block.minecraft.dirt` reaches the renderer as `block.minecraft.di` on
    /// a 90 px standing sign. That truncation is what the bug looked like in
    /// game, and asserting the whole key here would score zero for the wrong
    /// reason.
    pub(super) sign_raw_key_frames: u64,
    /// M164 — the largest `ColumnMesh::carried_fluid_cells` any column uploaded
    /// in the windowed client. See `r53`.
    pub(super) carried_fluid_cells: u32,
    /// Over the draws THIS RUN'S OWN INJECTION produced (`CROWD_IDS`, joined
    /// to the draw list by the position `collect_entities` gave each of them
    /// this frame): the most draws sharing one `EntityModelKind` that reached a
    /// single `set_entities`, and the most distinct non-capsule kinds among
    /// them.
    ///
    /// **The restriction to our own three entity ids is the load-bearing part.**
    /// The first version counted every `EntityModelKind::Zombie` draw in the
    /// frame, which is a claim about the world rather than about this
    /// injection: any other fixture in the same run that happens to spawn a
    /// zombie satisfies `>= 2` on its own, so a *partially* landed injection
    /// (one of the three bodies decoded) would still have passed. Restricting
    /// by id makes the two counts a statement about exactly the three entities
    /// this check created — and the two together pin the multiset `{A, A, B}`,
    /// since `same >= 2` with at most three draws and `distinct >= 2` cannot
    /// both hold otherwise.
    ///
    /// Two counts because they break independently. The first is the crowd —
    /// §0.0's mob-texture report says the symptom needs more than one mob in
    /// the scene, and nothing anywhere drove that in the *windowed* client. The
    /// second is kind RESOLUTION: every serverless gate (including this
    /// milestone's `mobtexshot`) hands `EntityDraw::kind` in directly, so the
    /// live `etypes.name(type_id)` -> `kind_for_entity_name` chain has never
    /// had a witness. A break in the type registry, the `add_entity` decode or
    /// the name match leaves both at their unlucky values rather than erroring.
    ///
    /// The first is a real maximum over kinds, not a count of one hard-coded
    /// kind: the doc used to say "the most draws of ONE mob kind" while the
    /// code counted only `Zombie`, which is the comment-that-outranks-the-code
    /// shape this file keeps recording.
    pub(super) crowd_same_kind_max: usize,
    pub(super) crowd_kinds_max: usize,
    /// M138a — listener transforms that reached the audio device.
    ///
    /// Compared against `frames`, not merely to zero: the interesting claim is
    /// that the push happens per FRAME. A client that moved it beside
    /// `sounds.drive` would still be non-zero here and would score roughly the
    /// tick count instead, which at any real frame rate is an order of
    /// magnitude smaller.
    pub(super) listener_pushes: u64,
    /// M147 — the sound event id `MusicManager` last started, if any.
    ///
    /// **Not gated on the `audio` feature**, and that is the point: music
    /// selection runs against whatever device is attached, so this grades the
    /// attribute layering, the selection and the timers in the DEFAULT build
    /// where the silent device stands in. The device is the only part it cannot
    /// see, and no check in this project can.
    pub(super) music_started: Option<String>,
    /// The forward vector the device last received, for a shape check.
    pub(super) listener_forward: Option<[f32; 3]>,
    /// M162 — where the engine put the wither-spawn sound a global
    /// `level_event` asked for, and where the listener was when it landed.
    ///
    /// **The position rather than the fact.** `live_identifiers` would prove
    /// the sound started and say nothing about where, and the whole of
    /// `globalLevelEvent` is about where: 1023 played at the block instead of
    /// two blocks from the listener is a sound at the right time from the wrong
    /// direction, which every count-shaped witness passes.
    pub(super) global_event_sound: Option<(f64, f64, f64)>,
    pub(super) global_event_eye: Option<[f64; 3]>,
    /// The block CENTRE the injected 1023 named, recorded by the gate from its
    /// own arithmetic so the bearing's direction can be predicted without
    /// asking the code under test where it thought the event was.
    pub(super) global_event_target: Option<[f64; 3]>,
    /// M162 — the explosion sound the `explode` tail carried, if the tail was
    /// walked at all.
    pub(super) explosion_sound: Option<(f64, f64, f64)>,
    /// M162 — `MotionStats::explosion_tails` / `explosion_sounds` at the end.
    pub(super) explosion_tails: u32,
    pub(super) explosion_sounds_queued: u32,
    /// M162 — `SoundStats::queued_delayed`, which only a distance-delayed
    /// `level_event` can move in an Overworld run (the other producer is the
    /// End flash).
    pub(super) queued_delayed: u32,
    /// M111 — frames on which the chat SCROLLBAR was drawn.
    ///
    /// It exists only while the screen is open AND the backlog exceeds the box
    /// (`virtualHeight != chatHeight`), so the gate injects 25 lines to reach
    /// it — otherwise this would be a witness over a path the run cannot enter,
    /// which is worse than no witness at all.
    pub(super) chat_scrollbar_frames: u64,
    /// M110 — frames on which the CHAT SCREEN was open and drew its input bar.
    ///
    /// Separate from `chat_line_frames`, which counts the read-only HUD box
    /// M108 built: that one is non-zero from the first message whether or not
    /// a screen exists, so it cannot see whether `T` opens anything. The gate
    /// force-opens the screen a fifth of the way in, the same way it injects a
    /// container for r19 — a windowed run has no keyboard.
    pub(super) chat_screen_frames: u64,
    /// M174 — frames on which the sign editor's board was actually pushed
    /// into the screen pass (read where the draw reads it, the r20 rule), and
    /// whether a replacement commit sent `sign_update`. The staged injection
    /// opens the editor at 0.55; the 0.90 book-menu injection REPLACES it,
    /// which is exactly vanilla's `setScreen` → `removed()` → commit path.
    pub(super) sign_edit_frames: u64,
    pub(super) sign_update_sent: bool,
    /// M115 — frames on which the SUGGESTION POPUP put fills in the list.
    ///
    /// A strictly narrower claim than r27's: the screen can be open with no
    /// popup at all, which is its state until something is typed. The gate
    /// reaches it through the production chain rather than a fake — a
    /// `custom_chat_completions` packet, then a real keystroke — so a break
    /// anywhere from the decode to the render drops this to zero.
    pub(super) suggestion_popup_frames: u64,
    /// M117 — frames on which the chat field was drawn as COLOURED RUNS
    /// rather than one flat line.
    ///
    /// Narrower than r27 and than r30: it needs a `/`-command in the field
    /// *and* a parse of it, and it is the only witness that can see the
    /// highlighting reach the windowed client at all.
    pub(super) highlighted_command_frames: u64,
    /// M117 — frames on which the USAGE BOX was drawn.
    ///
    /// Mutually exclusive with r29 by construction, so it cannot be satisfied
    /// by the same moment: the popup and the box never coexist.
    pub(super) usage_box_frames: u64,
    /// M134 — frames whose usage box held a parse-error MESSAGE rather than
    /// usage lines. Strictly narrower than `usage_box_frames`: the two name
    /// disjoint moments, because an exception suppresses the entries.
    pub(super) parse_error_frames: u64,
    /// M118 — how many times an `@`-selector was offered by the client's own
    /// `EntitySelectorParser`.
    ///
    /// Narrower than r30: a literal completion needs only the tree, where this
    /// needs `minecraft:entity` to have stopped being `Unknown`.
    pub(super) local_selector_completions: u64,
    /// M119 — how many times a registry id (a block or an item) was offered
    /// by the client's own parser.
    ///
    /// Narrower than r30 and disjoint from r33: it needs `block_state` or
    /// `item_stack` to have stopped being `Unknown`, and it counts a colon,
    /// which no literal or selector-option name carries.
    pub(super) local_resource_completions: u64,
    /// M120 — how many times a COORDINATE default was offered by the
    /// client's own parser. Disjoint from r33 and r34: `~` appears in no
    /// literal, selector or registry id.
    pub(super) local_coordinate_completions: u64,
    /// M124 — how many local answers offered a name from one of the seven
    /// literal tables. `sidebar.team.` prefixes only `scoreboard_slot`'s, so
    /// this is disjoint from r33/r34/r35 by construction.
    pub(super) local_literal_table_completions: u64,
    /// M116 — how many times a `/`-command's completion was answered by the
    /// CLIENT rather than the server.
    ///
    /// The claim r30 makes is the milestone's whole point, and it is a
    /// negative one: M114 sent a packet for every keystroke on a command line
    /// and this counts the ones that no longer leave. A witness on "the popup
    /// opened" cannot see it, because both paths open a popup.
    pub(super) local_command_completions: u64,
    /// M108 — frames on which the chat box put at least one line into the
    /// windowed frame's label list.
    ///
    /// The count comes from the production derivation (`build_text` returns
    /// it) rather than from the gate re-deriving `chat_lines` — a gate that
    /// recomputes the rule it grades agrees with any implementation, which is
    /// M93q's finding. It needs no caller staging: `--render-check` sends its
    /// own chat line, so an unstaged run still exercises the whole path from
    /// `player_chat` through the signature cache to the wrapped line.
    pub(super) chat_line_frames: u64,
    /// M125 — frames on which a drawn chat line was a RESOLVED translatable.
    ///
    /// The scene is the `/give` this gate ALREADY stages for r14, so this adds
    /// no caller requirement — and it is a much stronger claim than the one it
    /// replaces, because the server's success message is a translatable THREE
    /// LEVELS DEEP whose middle argument is a bare integer:
    ///
    /// ```text
    /// commands.give.success.single   "Gave %s %s to %s"
    ///   with[0] = 1                             (a raw IntTag)
    ///   with[1] = chat.square_brackets "[%s]"
    ///               with[0] = item.minecraft.diamond_sword
    ///   with[2] = <the player's display-name component>
    /// ```
    ///
    /// So `Gave 1 [Diamond Sword]` cannot be produced unless the lookup, the
    /// substitution, the recursion into a component argument, and the
    /// heterogeneous-list unwrap all work. **None of those five words appears
    /// anywhere in the raw component**, so the string cannot leak through from
    /// a flattener that resolved nothing.
    ///
    /// The first version of this witness drove the JOIN message instead, on
    /// the stated premise that a server announces a joining player to that
    /// player. **It does not**, and the gate said so by scoring zero:
    /// `PlayerList.placeNewPlayer` broadcasts at line 202 and does
    /// `this.players.add(player)` at line 210, and `broadcastSystemMessage`
    /// iterates `this.players` — so the joiner is not yet in the list it is
    /// announced to. Suspect the witness first.
    pub(super) translated_chat_frames: u64,
    /// M126d — frames on which the drawn chat carried MORE THAN ONE colour.
    ///
    /// The claim r26 and r37 cannot make. Both are satisfied by a chat box
    /// that flattens every message to one white string: r26 counts rows and
    /// r37 reads their characters, and neither can see whether the spans
    /// survived the wrap and reached the renderer as separately-coloured
    /// lines. A zero here with the scene injected means the pipeline is
    /// carrying spans and then throwing them away at the last step, which is
    /// exactly the failure the milestone exists to prevent.
    ///
    /// Counted over the DRAWN lines in `build_text`'s chat range, not over the
    /// chat store — M125's rule, one surface further on.
    pub(super) styled_chat_frames: u64,
    /// M127 — frames whose chat drew a DECORATED line: `<name> text`, from an
    /// injected `disguised_chat` bound to the server's own `minecraft:chat`.
    pub(super) decorated_chat_frames: u64,
    /// M127 — frames where that decoration's OWN style reached the row.
    pub(super) styled_decoration_frames: u64,
    /// M126d — frames on which a drawn chat line carried a non-plain
    /// `TextStyle`.
    ///
    /// Separate from the row above because the two halves travel by different
    /// routes: the colour rides `OwnedTextLine::color`, which existed before
    /// this milestone, while the five flags ride the `style` field it added.
    /// A wiring that dropped `style` on the floor would leave the colour
    /// witness green.
    pub(super) flagged_chat_frames: u64,
    /// M128 — where `chat_lines` DREW the clickable witness message, in
    /// screen pixels, read off the render rather than recomputed.
    ///
    /// That is the whole point of routing the click through this: the hit test
    /// and the draw are two consumers of one geometry, and the failure this
    /// repo keeps producing (M89, M94/M95, M106b, M112) is the two disagreeing
    /// by a row or by a panel's worth of pixels. Feeding the draw's own
    /// numbers back into the hit test is the only version of the witness that
    /// can see it.
    pub(super) chat_link_at: Option<(f32, f32)>,
    /// M128 — the click was fired once and answered `RunCommand` with the
    /// injected command. A count rather than a bool so the report can say how
    /// many frames it held (it is fired once, so 1 is the healthy value and 0
    /// is the failure).
    pub(super) chat_click_ok: u64,
    /// The once-guard for the click above, so the gate sends one command
    /// rather than one per frame.
    pub(super) chat_click_fired: bool,
    /// Frames on which a drawn chat line still carried a raw translation key.
    ///
    /// Not redundant with the row above: that one would stay green if the
    /// outer template resolved and an inner one did not, and this names all
    /// three keys of the same scene, so a break at ANY level turns it red.
    /// It must stay at zero.
    pub(super) unresolved_key_frames: u64,
    /// M105 — frames on which the book drew its `x/y` page counter.
    ///
    /// A LABEL, not a quad, so `book_quads_max` cannot see it: the counter goes
    /// through the text pass. It needs a book of more than one page, which
    /// needs more unlocked recipes than a fresh player has — hence the caller
    /// requirement, the same shape as r14's hotbar staging.
    pub(super) book_page_label_frames: u64,
    /// M104 — and the most it drew on a frame where one WAS open.
    ///
    /// A pair rather than one max, because the claim is a DIFFERENCE: an
    /// overlay adds a nine-sliced panel and a button each. One max over the
    /// whole run could not tell an overlay that drew from one that did not,
    /// since the book alone already clears any absolute threshold.
    pub(super) book_overlay_quads_max: usize,
    /// The panel height the RENDERER was holding while a container was open.
    ///
    /// Read back from `WorldRenderer::container_panel_height`, not from the
    /// open menu's layout — the first cut asked the layout, which answers 168
    /// for a chest whether or not the panel builder returned one, so it could
    /// not tell a working container from one that had silently fallen back to
    /// the player's 166-tall panel. A value witness is only a value witness if
    /// it reads the value the draw used.
    pub(super) container_panel_h: Option<f32>,
    /// The most overlay sprites the renderer's panel carried on any frame
    /// (M92).
    ///
    /// The chest injected at 0.4 has none — a chest's screen paints one sheet
    /// and stops — so this stays 0 unless the second injection at 0.85 (a
    /// brewing stand, with its data slots set) reaches the overlay builder.
    /// `containershot` grades those overlays offscreen; this is the only check
    /// that says the WINDOWED client draws them, which is the gap M88 closed
    /// for the panel itself and M86 for nine features before that.
    pub(super) container_overlays_max: usize,
    /// Frames on which a container was drawn **before** the gate force-opened
    /// the inventory (M89).
    ///
    /// The witness for `open_screen` opening the client's screen. M87 decoded
    /// the packet and drew whatever menu was open, but nothing turned the
    /// screen on, so a chest recorded its menu and showed nothing unless the
    /// player independently pressed E. These frames can only exist if the
    /// packet did it.
    pub(super) container_self_opened_frames: u64,
    /// Passes actually constructed by the end of the run.
    pub(super) gui_items_ready: bool,
    pub(super) hand_ready: bool,
    pub(super) text_ready: bool,
    pub(super) container_ready: bool,
    pub(super) clouds_ready: bool,
    pub(super) weather_ready: bool,
    pub(super) particles_ready: bool,
    pub(super) border_ready: bool,
    pub(super) crumbling_ready: bool,
    /// The ring witnesses, for each of the two passes M86 names.
    ///
    /// `last` is the handle the previous frame bound; `orphans` counts the
    /// frames on which that handle was **no longer among the pass's live
    /// buffers** — i.e. the previous frame's vertex buffer was destroyed while
    /// that frame could still be reading it, which is the VUID stated as a
    /// property. `max_live` is the deepest the ring ever got.
    ///
    /// A first cut of these counted *distinct consecutive handles* instead, on
    /// the theory that a 1-slot ring would keep handing back the same address.
    /// It does not: measured against the 1-slot mutation, that version reported
    /// 3,097 distinct handles and zero repeats over 3,099 frames while
    /// validation logged 11,881 destroy-while-in-use errors. A driver mints a
    /// fresh `VkBuffer` even for an immediate free-and-recreate, so the changed
    /// handle was never evidence of anything.
    pub(super) gui_item_last: u64,
    pub(super) hand_last: u64,
    pub(super) gui_item_orphans: u64,
    pub(super) hand_orphans: u64,
    pub(super) gui_item_max_live: usize,
    pub(super) hand_max_live: usize,
    /// Last-seen rebuild counters, so a legitimate ring reset is not scored as
    /// a use-after-free.
    pub(super) gui_item_generation: u64,
    pub(super) hand_generation: u64,
    /// How many rebuilds happened, reported so a run where the exemption fired
    /// suspiciously often is visible rather than silently forgiven.
    pub(super) gui_item_rebuilds: u64,
    pub(super) hand_rebuilds: u64,
    /// Whether validation was actually on. Asserting "0 errors" is worthless
    /// without it — a run with the layer off reports 0 for free.
    pub(super) validation: bool,
}

impl RenderCheck {
    /// Sample the ringed passes once per frame, after this frame's `set_*`
    /// calls and before the next one.
    pub(super) fn sample_rings(&mut self, wr: &WorldRenderer) {
        // A pass rebuild (`init_gui_items` / `init_hand`, on an atlas repack)
        // legitimately throws the whole ring away — `Pass::destroy` idles
        // first — so the frame after one is exempt, and the comparison
        // restarts from the new ring.
        let g_gen = wr.gui_item_generation();
        let g_rebuilt = g_gen != self.gui_item_generation;
        self.gui_item_rebuilds += u64::from(g_rebuilt);
        self.gui_item_generation = g_gen;
        let g_live = wr.gui_item_live_buffers();
        self.gui_item_max_live = self.gui_item_max_live.max(g_live.len());
        let g = wr.gui_item_vertex_buffer();
        if g != 0 {
            if !g_rebuilt && self.gui_item_last != 0 && !g_live.contains(&self.gui_item_last) {
                self.gui_item_orphans += 1;
            }
            self.gui_item_last = g;
        }
        let h_gen = wr.hand_generation();
        let h_rebuilt = h_gen != self.hand_generation;
        self.hand_rebuilds += u64::from(h_rebuilt);
        self.hand_generation = h_gen;
        let h_live = wr.hand_live_buffers();
        self.hand_max_live = self.hand_max_live.max(h_live.len());
        let h = wr.hand_vertex_buffer();
        if h != 0 {
            if !h_rebuilt && self.hand_last != 0 && !h_live.contains(&self.hand_last) {
                self.hand_orphans += 1;
            }
            self.hand_last = h;
        }
    }

    /// Print one row per witness; `true` if every one passed.
    /// Witnesses `live --render-check` declares (M160).
    ///
    /// **A shared, merge-silent resource, which is why it is declared at all.**
    /// See the seam block at the bottom of [`Self::report`]. Raise it in the
    /// same commit that adds a row, and take the next free id from
    /// `REWO_PLAN.md` §0.0's shared-resource allocation table rather than from
    /// "the highest one I can see" — that is how fifteen specs all chose r48.
    const EXPECTED_RENDER_CHECK_WITNESSES: usize = 66;

    pub(super) fn report(&self) -> bool {
        let vuids = rewo_gpu::validation_error_count();
        let mut rows: Vec<(&str, bool, String)> = Vec::new();
        let mut row = |name: &'static str, ok: bool, detail: String| rows.push((name, ok, detail));

        row(
            "r1 the run rendered frames",
            self.frames >= 60,
            format!("{} frames", self.frames),
        );
        row(
            "r2 the bake survives `resumed`",
            self.baked_frames == self.frames && self.frames > 0,
            format!("{} of {} frames", self.baked_frames, self.frames),
        );
        // Every frame **but the first**. `RainFog` is a stateful ease advanced
        // by `delta_ticks`, and frame 1's `dt` is 0 because there is no previous
        // frame to subtract from — so on that one frame the multiplier is still
        // exactly zero and `rain_fog_band` correctly returns the disabled
        // sentinel. The bound is derived from that, not fitted to the
        // measurement: it is 1, and a second frame of sentinel would fail.
        row(
            "r3 the rain-fog band is the bake's, not the None sentinel",
            self.fog_band_frames + 1 >= self.frames && self.frames > 0,
            format!("{} of {} frames", self.fog_band_frames, self.frames),
        );
        row(
            "r4 the GUI-item pass was built",
            self.gui_items_ready,
            format!("{}", self.gui_items_ready),
        );
        row(
            "r5 the GUI-item branch ran every frame",
            self.gui_item_frames == self.frames && self.frames > 0,
            format!("{} of {} frames", self.gui_item_frames, self.frames),
        );
        row(
            "r6 the hand pass was built",
            self.hand_ready,
            format!("{}", self.hand_ready),
        );
        row(
            "r7 the hand branch ran every frame",
            self.hand_frames == self.frames && self.frames > 0,
            format!("{} of {} frames", self.hand_frames, self.frames),
        );
        row(
            "r8 the weather branch ran every frame",
            self.weather_frames == self.frames && self.frames > 0,
            format!("{} of {} frames", self.weather_frames, self.frames),
        );
        // r9-r13 are **weaker than they look, and were measured to be**.
        //
        // Under the milestone's headline mutation — dropping the bake again, so
        // every branch below goes dead — all five of these still pass, because
        // these passes are constructed in `resumed` from the bake it still had
        // at that moment. They catch a pass that failed to build (a jar missing
        // a texture); they cannot catch a pass that is built and never fed.
        // The rows that die under that mutation are r2, r3, r5, r7, r8, r14,
        // r15 and r16. Do not read a green r9-r13 as "the clouds rendered".
        row(
            "r9 the cloud pass was built",
            self.clouds_ready,
            format!("{}", self.clouds_ready),
        );
        row(
            "r10 the precipitation pass was built",
            self.weather_ready,
            format!("{}", self.weather_ready),
        );
        row(
            "r11 the particle pass was built",
            self.particles_ready,
            format!("{}", self.particles_ready),
        );
        row(
            "r12 the world-border pass was built",
            self.border_ready,
            format!("{}", self.border_ready),
        );
        row(
            "r13 the block-breaking pass was built",
            self.crumbling_ready,
            format!("{}", self.crumbling_ready),
        );
        // The ring witnesses, stated as the property the VUID is about: a
        // buffer a frame bound must still exist on the next frame. `orphans`
        // counts violations directly; `max_live` proves the ring reached its
        // declared depth rather than merely never being caught out.
        //
        // The depth bar is `MAX_FRAMES_IN_FLIGHT + 1`, derived here from the
        // contract rather than read off `buf_ring_slots()` — comparing the ring
        // against its own declared length would be self-calibrating, passing at
        // 4 and at 1 alike. `buf_ring_slots()` appears only in the message, so
        // a disagreement between the two is visible.
        let required = rewo_gpu::MAX_FRAMES_IN_FLIGHT + 1;
        let slots = rewo_gpu::buf_ring_slots();
        row(
            "r14 the GUI-item ring keeps a bound buffer alive",
            self.gui_item_orphans == 0 && self.gui_item_max_live >= required,
            format!(
                "{} orphaned, {} live at peak, need {required}, declared {slots}, {} rebuilds",
                self.gui_item_orphans, self.gui_item_max_live, self.gui_item_rebuilds
            ),
        );
        row(
            "r15 the hand ring keeps a bound buffer alive",
            self.hand_orphans == 0 && self.hand_max_live >= required,
            format!(
                "{} orphaned, {} live at peak, need {required}, declared {slots}, {} rebuilds",
                self.hand_orphans, self.hand_max_live, self.hand_rebuilds
            ),
        );
        // The screen is the only door to `VelvetTextPass::sync_atlas`, this
        // milestone's ninth destroy-in-place. A quarter of the run is a floor
        // well under the half the gate opens for, so it fails on "never
        // reached" rather than on scheduling jitter.
        row(
            "r16 the inventory screen was drawn",
            self.screen_frames * 4 >= self.frames && self.frames > 0,
            format!("{} of {} frames", self.screen_frames, self.frames),
        );
        // M88 — the gap M87 recorded and could not close from a headless gate.
        row(
            "r19 a container screen was drawn in the windowed client",
            self.container_frames > 0,
            format!("{} of {} frames", self.container_frames, self.frames),
        );
        // ...and that it was a CONTAINER's panel, not the player's. A fallback
        // to `PLAYER` would keep r19 green while drawing 176x166 geometry for a
        // 63-slot menu, which is the failure worth naming rather than the one
        // that is merely absent.
        // M94 — and that the recipe book itself was drawn. A shut book carries
        // no quads at all, so this is 0 unless the book's builder ran.
        //
        // The minimum is the panel, the book's OWN tab count and the filter
        // toggle, with no unlocked recipes to fill the grid. Read from
        // `CRAFTING_TABS` rather than written as a literal: M95 corrected that
        // count from four to five, and a literal would have been quietly
        // generous by one from then on.
        let min_quads = 1 + rewo_world::recipe_book_screen::CRAFTING_TABS.len() + 1;
        row(
            "r23 the recipe book was drawn in the windowed client",
            self.book_quads_max >= min_quads,
            format!(
                "{} quads at peak (panel + {} crafting tabs + filter = {min_quads} with an empty grid)",
                self.book_quads_max,
                rewo_world::recipe_book_screen::CRAFTING_TABS.len()
            ),
        );
        // M104 — and that the which-of-these overlay reached the windowed
        // draw. Nothing here can right-click a recipe cell (that needs the
        // cursor over a specific cell AND a server that has sent a
        // multi-recipe group), so the `Open` is injected through the same
        // `open_overlay` the click path calls — M17's rule, that injection is
        // the deterministic proof where a live encounter depends on timing
        // nothing here controls. What that leaves to the unit tests is the
        // click ROUTING; what it proves is the render path, which is the only
        // thing this gate can see and M86's whole reason for existing.
        //
        // The floor is derived from the claim rather than from the emitter: an
        // overlay is at least one panel quad plus one button each, so three
        // buttons must add at least four. Reading `overlay_chrome`'s own
        // length here would make it self-calibrating.
        row(
            "r24 the which-of-these overlay was drawn in the windowed client",
            self.book_overlay_quads_max >= self.book_quads_max + 4,
            format!(
                "{} quads at peak with an overlay open against {} without — at least a panel quad and one per button",
                self.book_overlay_quads_max, self.book_quads_max
            ),
        );
        // M105 — the counter reaches the windowed frame's label list. It is a
        // caller requirement like r14's hotbar: a fresh player's book is one
        // page and draws no counter at all, so `recipe give @s *` has to be
        // staged. Failing closed on an unstaged run is the gate refusing to
        // certify a path it never saw.
        row(
            "r25 the recipe book drew its page counter",
            self.book_page_label_frames > 0,
            format!(
                "{} of {} frames — needs a multi-page book, i.e. REWO_PRECMD with `recipe give @s *`",
                self.book_page_label_frames, self.frames
            ),
        );
        // M108 — the chat box reached the windowed frame. Unlike r14 and r25
        // this needs no caller staging: the run sends its own chat line, and
        // the server echoing it back is what drives `player_chat` through the
        // signature cache, the trust level, the wrap and the geometry. A zero
        // here means the whole chain is dead in the windowed client, which is
        // the failure M86 existed to catch.
        //
        // **It is structurally blind to the fade**, and the near-total count is
        // the tell rather than a worry: `RENDER_CHECK_SECONDS` is 8, i.e. 160
        // ticks, and `AlphaCalculator.timeBased` holds full alpha until 180, so
        // no message this run receives can fade before it ends. The fade is
        // graded by unit tests on both sides of the seam
        // (`the_fade_and_the_text_opacity_both_reach_the_line` here,
        // `a_message_holds_full_alpha_for_one_hundred_and_eighty_ticks` in
        // `rewo_world::chat`). Lengthening the run to reach the fade would turn
        // a gate into a soak for one property two tests already pin.
        row(
            "r26 the chat box drew a line in the windowed client",
            self.chat_line_frames > 0,
            format!(
                "{} of {} frames carried at least one wrapped chat line                  (near-total is CORRECT: the run is 8 s = 160 ticks and the                  fade starts at 180, so nothing can fade inside it)",
                self.chat_line_frames, self.frames
            ),
        );
        // M125 — and that a line was a RESOLVED translatable, which r26 cannot
        // see: before M125 the chat box drew `multiplayer.player.joined` and
        // scored a full r26.
        row(
            "r37 a chat line resolved a nested translatable component",
            self.translated_chat_frames > 0 && self.unresolved_key_frames == 0,
            format!(
                "{} of {} frames drew \"Gave 1 [Diamond Sword]\"; {} drew a raw key                  (must be 0). Three nesting levels and a bare-integer argument,                  off the `/give` r14 already stages — a count of 0 with the                  sword staged means the resolution is dead.",
                self.translated_chat_frames, self.frames, self.unresolved_key_frames
            ),
        );
        // M126d — that the spans survived to the renderer. r26 and r37 are
        // both satisfied by a chat box that flattens everything to one white
        // string, so neither can see this.
        row(
            "r38 a drawn chat line carried more than one colour",
            self.styled_chat_frames > 0,
            format!(
                "{} of {} frames drew ONE ROW in 2+ distinct colours (from one                  injected section-sign-coded system message: the codes must                  survive `parse_component`, the wrap's part list, and                  `chat_lines`' per-span emit. Across the whole box would be a                  weaker claim a flattening client also satisfies)",
                self.styled_chat_frames, self.frames
            ),
        );
        row(
            "r39 a drawn chat line carried a non-plain style flag",
            self.flagged_chat_frames > 0,
            format!(
                "{} of {} frames drew italic/underline/strikethrough (the flags                  ride `TextStyle`, a different field from the colour, so a                  wiring that dropped them would leave r38 green)",
                self.flagged_chat_frames, self.frames
            ),
        );
        // M127 — the decoration. r26, r37, r38 and r39 are all satisfied by a
        // client that renders a chat line's own content and never decorates
        // it, which is what Rewo did from M1 to M126: a player line read `hi`
        // where vanilla reads `<Steve> hi`.
        //
        // This grades the WHOLE chain and nothing else can: the server's
        // configuration `registry_data` has to have carried
        // `minecraft:chat_type` and been parsed (an unsynced registry makes
        // `chat_type_id` answer `None` and the injection is skipped, scoring
        // zero); the bound's `holder` VarInt has to resolve back to that entry;
        // the decoration has to build `Component.translatable("chat.type.text",
        // [name, content])`; M125's resolution has to find `<%s> %s` and
        // substitute both arguments; and M126's spans have to reach the glyphs.
        row(
            "r40 a drawn chat line was DECORATED by its chat type",
            self.decorated_chat_frames > 0,
            format!(
                "{} of {} frames drew \"<RewoDecoWitness> decorated\" — the                  server's own `minecraft:chat` entry, looked up by name rather                  than assumed, so an unsynced registry scores 0 rather than                  silently using index 0",
                self.decorated_chat_frames, self.frames
            ),
        );
        // M127 — and that the decoration's own `Style` reached the row, which
        // r40 cannot see: `chat.type.text` carries `Style.EMPTY`, so a
        // decoration that dropped `withStyle` entirely would leave r40 green.
        // Five of the seven vanilla types are unstyled and the two `/msg` ones
        // are `withColor(GRAY).withItalic(true)` — which is the whole reason
        // M126's pipeline had to land before this milestone.
        row(
            "r41 the decoration's own style reached the row",
            self.styled_decoration_frames > 0,
            format!(
                "{} of {} frames drew the `/msg` decoration's row entirely in                  gray AND italic. The arguments are bare literals with                  `Style.EMPTY`, so they INHERIT it — which is the composition                  rule (`getStyle().applyTo(parentStyle)`) and not a coincidence",
                self.styled_decoration_frames, self.frames
            ),
        );
        // M128 — a click on chat text, driven through the production path:
        // `chat_mouse_pressed` -> `clickable_style_at` -> `ChatScreen
        // ::mouse_clicked` -> `handle_component_clicked` -> `send_command`.
        // The click POINT is read off the drawn line rather than recomputed,
        // so a hit test that disagreed with the renderer about where a row
        // sits turns this red while every unit test stays green.
        row(
            "r42 a click on a chat link ran its command",
            self.chat_click_ok > 0,
            format!(
                "{} clicks answered RunCommand at the drawn line's own                  position (0 means the injected `click_event` did not survive                  the wire, the wrap, the store or the hit test — or that the                  hit test and the draw disagree about where a row is)",
                self.chat_click_ok
            ),
        );
        // M110 — the chat SCREEN, as distinct from r26's read-only box. A zero
        // means `T` reaches nothing in the windowed client, which is the
        // failure M86 existed to catch and which r26 structurally cannot see.
        row(
            "r27 the chat screen opened and drew its input bar",
            self.chat_screen_frames > 0,
            format!(
                "{} of {} frames had the screen up with its input bar in the                  backdrop list",
                self.chat_screen_frames, self.frames
            ),
        );
        // M111 — and the scrollbar within it, which needs more chat than the
        // box holds and so is a strictly narrower claim than r27's.
        row(
            "r28 the chat scrollbar was drawn",
            self.chat_scrollbar_frames > 0,
            format!(
                "{} of {} frames drew the bar's two rects (needs a backlog past                  the focused box's 20 rows, which the run injects)",
                self.chat_scrollbar_frames, self.frames
            ),
        );
        // M115 — and the popup within it, which needs a completion word AND a
        // keystroke, so it is narrower again than r28's.
        row(
            "r29 the suggestion popup was drawn",
            self.suggestion_popup_frames > 0,
            format!(
                "{} of {} frames drew the popup's row fills (needs a                  custom_chat_completions word and a keystroke, both of which the run                  drives through the production path)",
                self.suggestion_popup_frames, self.frames
            ),
        );
        // M117 — the syntax highlighting reached the frame at all.
        row(
            "r31 the command line was drawn as coloured runs",
            self.highlighted_command_frames > 0,
            format!(
                "{} of {} frames drew the field from a parse rather than as one                  flat line",
                self.highlighted_command_frames, self.frames
            ),
        );
        // M117 — the usage box, which `extractRenderState` draws only when
        // the popup is absent, so this and r29 name disjoint moments.
        row(
            "r32 the usage box was drawn under the field",
            self.usage_box_frames > 0,
            format!(
                "{} of {} frames drew a usage line (needs an ARGUMENT expected                  at the cursor and no popup over it)",
                self.usage_box_frames, self.frames
            ),
        );
        // M134 — and a parse-error MESSAGE in that same box, which is a
        // strictly narrower claim than r32's and names a disjoint moment:
        // `if (commandUsage.isEmpty())` means an exception suppresses the
        // usage entries, so no frame can score both. The staged input is a
        // command that does not exist, whose `getExceptions()` is EMPTY —
        // it reaches the line through `Commands.getParseException`, the
        // branch M117 omitted outright.
        row(
            "r43 a parse error was drawn white under the command field",
            self.parse_error_frames > 0,
            format!(
                "{} of {} frames drew a line opening \"Unknown command\" in the                  usage box, in WHITE. Grey is `USAGE_FORMAT`'s and a brigadier                  message carries no style at all, so `extractUsage`'s `-1`                  stands — the colour is half the claim because the box drew                  every line white from M117 until this milestone",
                self.parse_error_frames, self.frames
            ),
        );
        // M118 — and a SELECTOR among them, which needs the entity argument
        // type rather than just the tree.
        row(
            "r33 an entity selector was offered locally",
            self.local_selector_completions > 0,
            format!(
                "{} completions containing an @-selector (needs                  minecraft:entity to parse, which it did not before M118)",
                self.local_selector_completions
            ),
        );
        // M119 — and a registry id among them, which needs the block/item
        // argument types rather than the tree or the selector.
        row(
            "r34 a registry id was offered locally",
            self.local_resource_completions > 0,
            format!(
                "{} completions containing a namespaced id (needs block_state or                  item_stack to parse, which they did not before M119)",
                self.local_resource_completions
            ),
        );
        // M120 — and a coordinate among them.
        row(
            "r35 a coordinate default was offered locally",
            self.local_coordinate_completions > 0,
            format!(
                "{} completions containing a `~` (needs the coordinate family to                  parse, which it did not before M120)",
                self.local_coordinate_completions
            ),
        );
        // M124 — and a name from one of the seven tables that live outside
        // their own argument class.
        row(
            "r36 a literal table was offered locally",
            self.local_literal_table_completions > 0,
            format!(
                "{} completions offering a `sidebar.team.*` slot (needs                  scoreboard_slot's table, which read as a bare word before M124)",
                self.local_literal_table_completions
            ),
        );
        // M116 — the dispatcher answered a command locally, i.e. WITHOUT a
        // round trip. Both paths open a popup, so r29 cannot see this.
        row(
            "r30 a command completion was answered locally",
            self.local_command_completions > 0,
            format!(
                "{} completions answered by the client's own dispatcher (the run                  types `/` then a letter, which reaches only literals)",
                self.local_command_completions
            ),
        );
        row(
            "r21 open_screen opened the client's screen by itself",
            self.container_self_opened_frames > 0,
            format!(
                "{} frames drawn before the gate force-opened the inventory",
                self.container_self_opened_frames
            ),
        );
        row(
            "r20 the container's panel was its own, not the player's 166",
            self.container_panel_h.is_some_and(|h| (h - 166.0).abs() > 0.5),
            format!("panel height {:?} (player's is 166)", self.container_panel_h),
        );
        row(
            "r22 the windowed client drew a container's overlays",
            self.container_overlays_max > 0,
            format!(
                "{} overlay sprites at peak (a brewing stand draws fuel + arrow + bubbles;                  the chest injected earlier draws none)",
                self.container_overlays_max
            ),
        );
        // M132 — the scoreboard sidebar. Nothing before this could see it:
        // Rewo decoded the four scoreboard packets from M65 and drew nothing,
        // so every other witness here is satisfied by a client with no sidebar
        // at all.
        //
        // The gate injects an objective, three scores and a display-slot
        // assignment as raw bodies through the production router (M17's rule),
        // which makes the whole chain the claim: the packets have to decode,
        // `select_objective` has to find the slot, the rows have to resolve
        // their names and formats, and the layout has to place them. A hidden
        // holder is injected alongside, so a client that skipped
        // `isHidden` scores EIGHT lines rather than seven and fails on the
        // count as well as on the ordering.
        row(
            "r44 the scoreboard sidebar was drawn in the windowed client",
            self.sidebar_frames > 0 && self.sidebar_text_max == 7,
            format!(
                "{} of {} frames drew a sidebar, at most {} text lines (must be                  exactly 7: a title, then three visible rows of name + score.                  A fourth `#hidden` holder is injected too, so 9 means                  `isHidden` was skipped and 1 means only the title resolved)",
                self.sidebar_frames, self.frames, self.sidebar_text_max
            ),
        );
        // M138a — the listener seam. `AudioDevice` had four methods and none of
        // them was a listener, so every sound was positioned in absolute world
        // coordinates against ears at the origin facing -Z.
        //
        // **Compared against the frame count, not against zero.** The push
        // belongs on the render path, because vanilla's
        // `SoundEngine.updateSource(camera)` is called with the camera the frame
        // is about to use, while `SoundEngine.tick` is `Minecraft.tick`'s. Moved
        // beside `sounds.drive` this would still be non-zero and would score
        // about the tick count — an order of magnitude below the frames — so a
        // `> 0` threshold cannot tell the two apart. This is also the only check
        // that reaches the call site at all: it is a composition root in a binary
        // crate, and deleting it survives the whole unit suite.
        row(
            "r45 the listener reached the audio device, once per frame",
            self.listener_pushes == self.frames
                && self
                    .listener_forward
                    .is_some_and(|f| (f[0] * f[0] + f[1] * f[1] + f[2] * f[2] - 1.0).abs() < 1e-3),
            format!(
                "{} pushes over {} frames (must be equal -- per tick would be                  roughly the tick count), last forward {:?} (must be a unit                  vector, which a default-constructed or zeroed transform is not)",
                self.listener_pushes, self.frames, self.listener_forward
            ),
        );
        row(
            "r46 the client selected and started a music track",
            self.music_started.is_some(),
            format!(
                "started {:?} (M147: the Overworld's BackgroundMusic lives on the                  DIMENSION TYPE and not on plains, so with an EMPTY base this                  is None and no music plays anywhere in the Overworld -- which                  is what M146 shipped. The run is 8 s = 160 ticks against a                  100-tick STARTING_DELAY, so a client that never decremented                  scores None too.)",
                self.music_started
            ),
        );
        // M151 — the tab list. The M86 shape a second time: `rewo_gpu::tab_list`
        // shipped with 41 passing tests and no caller, so every other witness
        // above is satisfied by a client whose Tab key does nothing at all.
        //
        // Four claims, and each fails differently:
        //
        // * `> 0` — the key, the resolver, the layout and the emitters were all
        //   reached in the WINDOWED client, which is the only thing no
        //   serverless gate can see.
        // * `< frames` — and it is a HOLD. The key goes down at half the run,
        //   so a client that drew the list unconditionally scores every frame
        //   and is exactly as wrong as one that never drew it.
        // * `rows == 3` — the local player plus the two the run injects as
        //   `listed: true`. Two more are injected as `listed: false`, so a
        //   client that ignored `getListedOnlinePlayers()` scores FIVE. That is
        //   the whole reason `listed` had to stop being decoded-and-dropped.
        // * `icons == rows` — one ping sprite per row, through `set_hud_icons`,
        //   which is a different list from the text and breaks independently.
        row(
            "r47 the tab list was drawn in the windowed client, while its key was held",
            self.tab_list_frames > 0
                && self.tab_list_frames < self.frames
                && self.tab_list_rows_max == 3
                && self.tab_list_icons_max == 3
                && self.tab_list_text_max == 3,
            format!(
                "{} of {} frames drew it (must be neither 0 nor all — the key \
                 goes down at half the run), at most {} rows / {} ping icons / \
                 {} text lines. Three is the local player plus the two injected \
                 `listed: true` players; FIVE means the two `listed: false` ones \
                 were shown, and one means the injection never landed.",
                self.tab_list_frames,
                self.frames,
                self.tab_list_rows_max,
                self.tab_list_icons_max,
                self.tab_list_text_max
            ),
        );
        // ── M162: the two packet tails that carry a sound ──────────────────
        //
        // All three call sites live inside `PlaySession`, which owns a socket
        // and cannot be built in a unit test (M71) — so `soundshot` grades the
        // arithmetic exactly and would stay green with `queue_explosion_sound`
        // deleted and `camera_eye` hardwired to `None`. This is the only check
        // that can see them, which is r45's stated reason for existing.
        let bearing = match (
            self.global_event_sound,
            self.global_event_eye,
            self.global_event_target,
        ) {
            (Some(s), Some(eye), Some(t)) => {
                let to_sound = [s.0 - eye[0], s.1 - eye[1], s.2 - eye[2]];
                let len = (to_sound[0] * to_sound[0]
                    + to_sound[1] * to_sound[1]
                    + to_sound[2] * to_sound[2])
                    .sqrt();
                let to_event = [t[0] - eye[0], t[1] - eye[1], t[2] - eye[2]];
                let dot = to_sound[0] * to_event[0]
                    + to_sound[1] * to_event[1]
                    + to_sound[2] * to_event[2];
                let to_block = ((s.0 - t[0]).powi(2) + (s.1 - t[1]).powi(2) + (s.2 - t[2]).powi(2))
                    .sqrt();
                Some((len, dot, to_block))
            }
            _ => None,
        };
        row(
            "r48 a global level event placed its sound against the live camera",
            // 2.0 blocks from the EYE the gate recorded independently — so a
            // `camera_eye` reading feet rather than the eye misses by 1.62 and
            // fails here. The dot's SIGN is what kills the reversed
            // `Vec3.subtract`, which a distance-only check passes at 180
            // degrees. And the block is 58 blocks away, so "the sound is not at
            // the block" separates this from the block-placed path.
            bearing.is_some_and(|(len, dot, to_block)| {
                (len - 2.0).abs() < 1e-6 && dot > 0.0 && to_block > 50.0
            }),
            format!(
                "eye {:?} -> sound {:?} (target {:?}): {:?} = (blocks from eye, dot with the \
                 bearing, blocks from the block). `None` here means the camera never \
                 resolved, which is what a client that hands `route_level_event_sound` a \
                 hardcoded `None` scores.",
                self.global_event_eye, self.global_event_sound, self.global_event_target, bearing
            ),
        );
        row(
            "r49 an explode packet's TAIL was walked and its sound started",
            self.explosion_tails > 0
                && self.explosion_sounds_queued > 0
                && self.explosion_sound.is_some(),
            format!(
                "{} tails walked, {} sounds queued, engine position {:?}. M68 consumed only \
                 the physics prefix and threw the sound away; a client that still does \
                 scores 0/0/None here with every other witness green.",
                self.explosion_tails, self.explosion_sounds_queued, self.explosion_sound
            ),
        );
        row(
            "r50 a far level event was QUEUED rather than played",
            self.queued_delayed > 0,
            format!(
                "{} delayed. The injected 3012 is 200 blocks out, past the strict \
                 `distanceToSqr > 100.0` gate, so it must reach `playDelayed`. The only \
                 other producer in this run would be an End flash, and this is the \
                 Overworld.",
                self.queued_delayed
            ),
        );
        // M163 — the wire-time flattens. Two halves, and the second is what
        // makes this a witness rather than a count: r26 scored full marks for
        // three milestones while chat drew `multiplayer.player.joined`, so
        // "a name was drawn" is satisfied by the bug.
        row(
            "r51 a mob nametag drew a RESOLVED translatable, and never its raw key",
            self.nametag_resolved_frames > 0 && self.nametag_raw_key_frames == 0,
            format!(
                "{} of {} frames carried `Zombie`, {} carried \
                 `entity.minecraft.zombie`. The injected zombie's \
                 `DATA_CUSTOM_NAME` is `{{translate: \"entity.minecraft.zombie\"}}`; \
                 before M163 `metadata::parse` flattened it with \
                 `Nbt::to_plain_text`, which has no language table, so the \
                 second number was the whole of it. Zero for BOTH means the \
                 injection never landed",
                self.nametag_resolved_frames, self.frames, self.nametag_raw_key_frames
            ),
        );
        row(
            "r52 a sign line drew a RESOLVED translatable, and never its raw key",
            self.sign_resolved_frames > 0 && self.sign_raw_key_frames == 0,
            format!(
                "{} of {} frames carried `Dirt`, {} carried the truncated key. \
                 The injected sign's front text is \
                 `{{translate: \"block.minecraft.dirt\"}}`; the app resolves it \
                 in the FRAME LOOP rather than at decode, which is why r51 \
                 cannot stand in for this. Zero for BOTH means the injection \
                 never landed or `collect_session_sign_text` was not called",
                self.sign_resolved_frames, self.frames, self.sign_raw_key_frames
            ),
        );
        // M164 — waterlogged water in the WINDOWED client.
        //
        // This does not grade a pixel, and says so: `meshshot`'s waterlogged
        // control grades the geometry, and the water rides the translucent pass
        // the client has drawn since M3. What no serverless gate can see is the
        // wiring — `MeshTables.fluid` is a new field, and a `live_cmd` that
        // failed to fill it would mesh no carried water anywhere while every
        // other witness here, every unit test and every `*shot` stayed green.
        // That is the M86 shape exactly, so the claim is "the production chain
        // from the bake through the mesh pool reached a real server's block".
        //
        // The counter behind it is **emission-gated** (`mesh_column`:
        // `f.carried && fv.len() > fluid_verts_before`), which it was not when
        // M164 shipped: it incremented beside the `emit_fluid` CALL, so it
        // proved the table reached the mesher rather than that any water was
        // meshed, and this row's own wording over-claimed by exactly that gap.
        //
        // The run places the block itself rather than making this a fourth
        // caller requirement (M108's precedent): `/setblock` needs op, which
        // `tools/render_check.py` already grants for r25.
        row(
            "r53 a waterlogged block from the server meshed its water",
            self.carried_fluid_cells > 0,
            format!(
                "{} carried-fluid cells in the largest column (0 means the bake, \
                 the MeshTables field, the mesh pool or the setblock did not \
                 reach each other -- and renders EXACTLY as it did before M164)",
                self.carried_fluid_cells
            ),
        );
        // r48 — the crowd, and the kind resolution under it.
        //
        // Both counts are taken over THIS CHECK'S OWN three injected entities
        // (`CROWD_IDS`), never over the frame at large. That is what stops the
        // threshold being satisfied by somebody else's mob: with at most three
        // draws in scope, `same >= 2` means both injected zombies decoded and
        // resolved to one kind, and `distinct >= 2` means the villager decoded
        // and resolved to a different one — together the multiset `{A, A, B}`,
        // which no partially landed injection can produce.
        //
        // It stays `>=` rather than `==` because the numbers are running
        // maxima over frames and a third distinct kind is unreachable from
        // three entities of two types; the restriction, not the comparison, is
        // what makes it a claim about the client rather than about the world.
        row(
            "r54 two mobs of one type and one of another reached ONE draw list, with distinct resolved kinds",
            self.crowd_same_kind_max >= 2 && self.crowd_kinds_max >= 2,
            format!(
                "over the {} injected entities: {} draws of one kind at peak, {} \
                 distinct non-capsule kinds (0 same-kind means the injected \
                 `add_entity` bodies never decoded or never reached the draw \
                 list; 1 distinct kind means `kind_for_entity_name` collapsed \
                 them, which is the wrong-model half of the reported bug)",
                CROWD_IDS.len(),
                self.crowd_same_kind_max,
                self.crowd_kinds_max
            ),
        );
        // ── M166: the two BLOCKING configuration tasks ──
        //
        // These two are unlike every other row here, because before M166 the
        // failure was not a wrong pixel — it was **no run at all**. A server
        // with `resource-pack=` or `enable-code-of-conduct=true` set left the
        // client in `run_configuration` forever, so the whole gate scored
        // 0 witnesses and the window never opened.
        //
        // Both claims are cross-checks against something the SERVER was
        // configured with, passed in by `tools/render_check.py` through the
        // environment. That matters: a counter (`we replied twice`) is
        // satisfied by a client that replied with a zero id and an empty
        // string, which is exactly what the malformed-body path produces.
        // Comparing against the staged values means the wire must have carried
        // them.
        //
        // They fail CLOSED when unstaged, on §5's rule — a witness that skips
        // itself when its precondition is absent is green on every machine
        // where the precondition is what matters.
        let want_pack = crate::knobs::var("REWO_RC_PACK_ID").ok();
        row(
            "r55 both pushed resource packs were decoded and answered TERMINALLY",
            match (&want_pack, self.config_tasks.pack_replies.as_slice()) {
                (Some(want), [(cfg_id, cfg_act), (play_id, play_act)]) => {
                    use rewo_net::config_tasks::PackAction;
                    format!("{cfg_id:032x}") == want.to_ascii_lowercase()
                        && *cfg_act == PackAction::FailedDownload
                        && *play_id == PLAY_PACK_ID
                        && *play_act == PackAction::InvalidUrl
                }
                _ => false,
            },
            format!(
                "staged id {:?}, replies {:?}. TWO are required, in order: the                  configuration task's (the server's own `resource-pack-id`, so                  agreement proves the UUID came off the wire rather than out of                  a default) answered FAILED_DOWNLOAD, then the injected                  PLAY-state push answered INVALID_URL because its url is                  `ftp:`. One reply means the play arm is dead -- nothing else                  in this repo reaches it, since a vanilla server pushes during                  CONFIGURATION. Empty means the run was not staged, or -- the                  bug this milestone fixes -- the client never got past                  configuration at all, in which case every row above is also                  zero.",
                want_pack,
                self.config_tasks
                    .pack_replies
                    .iter()
                    .map(|(id, a)| format!("{id:032x}:{a:?}"))
                    .collect::<Vec<_>>()
            ),
        );
        let want_coc = crate::knobs::var("REWO_RC_COC").ok();
        row(
            "r56 the server's code of conduct was decoded and accepted",
            match (&want_coc, self.config_tasks.codes_of_conduct.as_slice()) {
                (Some(want), [got]) => got == want,
                _ => false,
            },
            format!(
                "staged {:?}, accepted {:?}. This task is queued AHEAD of the                  resource pack (`addOptionalTasks`), so on a server with both                  it is the one that hangs first and r55 can never be reached                  without it.",
                want_coc, self.config_tasks.codes_of_conduct
            ),
        );
        // M168 — the survival HUD's two live inputs that only a server can
        // supply. Both are COUNT-plus-VALUE: ten armour icons is "an ARMOR
        // attribute reached the local store" and three full ones is "it was
        // the staged iron chestplate's 6"; one effect icon at the first
        // beneficial slot is "`update_mob_effect` for YOU was kept, its
        // showIcon bit read, and the table called speed beneficial". The
        // server sends the player's own attributes only through
        // `sendToTrackingPlayersAndSelf` on a dirty set, never in the initial
        // pairing, so a green r57 is also the proof that path exists.
        row(
            "r57 the staged iron chestplate reached the armour row as three full icons of ten",
            self.armor_icons_max == 10 && self.armor_full_max == 3,
            format!(
                "max armour icons {} (10 when any armour is drawn), max full {} \
                 (iron chestplate = 6 armour = 3 full). Zero means the player's own \
                 `update_attributes` never arrived or never reached `local_attributes`",
                self.armor_icons_max, self.armor_full_max
            ),
        );
        row(
            "r58 the staged speed effect drew one icon at the first beneficial slot",
            self.effect_icons_max == 1 && self.effect_first_beneficial,
            format!(
                "max effect icons {}, first beneficial slot seen {}. `effect give ... \n                 infinite 0` with particles ON: the `hideParticles` argument also clears \n                 showIcon (MobEffectInstance.java:61); the icon sits at (guiWidth - 25 + 3, 4)",
                self.effect_icons_max, self.effect_first_beneficial
            ),
        );
        row(
            "r59 the staged saddled horse put the jump bar up and the release sent START_RIDING_JUMP",
            self.jump_bar_blits_max >= 2 && self.jump_scale_max >= 0.9 && self.riding_jumps_sent >= 1,
            format!(
                "max jump-bar blits {} (2 = background + progress), max scale {:.2} \
                 (a 16-tick hold peaks at 1.0 and decays toward 0.8), START_RIDING_JUMPs \
                 sent {}. Needs: the saddle off equipment slot 7, the bot as the horse's \
                 first passenger, the ramp in the mounted tick, and the packet",
                self.jump_bar_blits_max, self.jump_scale_max, self.riding_jumps_sent
            ),
        );
        row(
            "r60 the staged leashed cow drew a rope through collect_leashes",
            self.leash_verts_max >= 3,
            format!(
                "max leash ribbon verts {} (a ribbon is 98 triangles = 294 verts). \
                 Needs: the cow + fence-knot in the table, the set_entity_link \
                 holder, and collect_leashes building the ribbon into the pass",
                self.leash_verts_max
            ),
        );
        row(
            "r61 the injected written book opened the reader with its pages",
            self.book_frames > 0 && self.book_pages_seen == 2,
            format!(
                "book-screen frames {} (needs set_player_inventory recording the \
                 SlotText, open_book resolving hand 0 off the app's hotbar slot, \
                 and the screen opening), pages {} (want 2 — the resolve read the \
                 written_book_content off the wire)",
                self.book_frames, self.book_pages_seen
            ),
        );
        row(
            "r62 the sound options page drew its eleven sliders in the windowed client",
            self.options_slider_sprites_max == 22,
            format!(
                "max slider sprites {} (11 sliders x track+handle = 22; fewer means a                  lowering was dropped, more a stray widget). The page was opened and                  closed through the production paths — Sound, root, pause, game",
                self.options_slider_sprites_max
            ),
        );
        row(
            "r17 validation was enabled",
            self.validation,
            format!("{}", self.validation),
        );
        row(
            "r63 the injected open_sign_editor opened the editor in the windowed client",
            self.sign_edit_frames > 0,
            format!(
                "{} editor frames (needs the staged oak_sign BE, the production decode \
                 opening the screen, and the board reaching the screen pass)",
                self.sign_edit_frames
            ),
        );
        row(
            "r64 the editor's replacement committed sign_update",
            self.sign_update_sent,
            format!(
                "sent {} (vanilla's setScreen calls removed() on the old screen, which \
                 IS the commit — the 0.90 book-menu injection replaces the editor)",
                self.sign_update_sent
            ),
        );
        // The windowed client's OWN renderer has the passes that draw text and
        // container screens. Every other text/container row counts data
        // (lines built, panel heights) that is stored whether or not a pass
        // exists to draw it — which is how the windowed client shipped with
        // neither pass while those rows stayed green.
        row(
            "r65 the windowed renderer built the vanilla-font text pass",
            self.text_ready,
            format!("{}", self.text_ready),
        );
        row(
            "r66 the windowed renderer built the container-screen pass",
            self.container_ready,
            format!("{}", self.container_ready),
        );
        row(
            "r18 the session was validation-clean",
            vuids == 0,
            format!("{vuids} errors"),
        );

        // ── M160: the witness namespace is a SHARED, MERGE-SILENT resource ──
        //
        // Until this block existed, `report` ended `pass == rows.len()` with no
        // declared count and no uniqueness check — so two branches could each
        // add an `r48`, git would merge both cleanly, and the gate would print
        // `48/48` while grading one of them twice and the other never.
        //
        // **That is not hypothetical.** M127-M134's integration lost r42 to
        // exactly this (three branches minting the same id, §0.0), and a
        // 20-agent survey in 2026-08-17 found **fifteen** independent specs all
        // claiming `r48`. A conflict git can see is a good outcome; this one it
        // cannot.
        //
        // So: ids are parsed out of the names, and the run fails closed on a
        // duplicate, a gap, or a count that has moved without the constant
        // moving with it. All three are things a merge produces and no pixel
        // can show.
        let seam = witness_seam_faults(
            &rows.iter().map(|(n, _, _)| *n).collect::<Vec<_>>(),
            Self::EXPECTED_RENDER_CHECK_WITNESSES,
        );
        for fault in &seam {
            println!("[rendercheck] SEAM FAIL {fault}");
        }
        let seam_ok = seam.is_empty();
        let mut pass = 0usize;
        for (name, ok, detail) in &rows {
            println!(
                "[rendercheck] {} {name} ({detail})",
                if *ok { "PASS" } else { "FAIL" }
            );
            if *ok {
                pass += 1;
            }
        }
        println!("[rendercheck] {pass}/{} witnesses", rows.len());
        pass == rows.len() && seam_ok
    }
}

/// Faults in `live --render-check`'s witness namespace (M160).
///
/// **The namespace is a shared, MERGE-SILENT resource, and that is the whole
/// reason this exists.** Until M160, `report` ended `pass == rows.len()` with no
/// declared count and no uniqueness check — so two branches could each add an
/// `r48`, git would merge both cleanly, and the gate would print `48/48` while
/// grading one of them twice and the other never.
///
/// That is not hypothetical. The M127-M134 integration lost r42 to exactly this
/// (`REWO_PLAN.md` §0.0), and a 20-agent survey on 2026-08-17 found **fifteen**
/// independent specs all claiming `r48` — because every one of them picked "the
/// highest id I can see plus one" from the same pre-merge tree.
///
/// Three faults, each of which a merge produces and no pixel can show:
///
/// * a **duplicate** id — two milestones minted the same number;
/// * a **count** that has moved without [`RenderCheck::EXPECTED_RENDER_CHECK_WITNESSES`]
///   moving with it, which is what catches a row silently lost to a conflict
///   resolution;
/// * a **gap** in the sequence — a deleted row whose id was never reclaimed,
///   which is how the *next* milestone picks a number that looks free and is not.
///
/// Free-standing rather than a method on `RenderCheck` because `report` needs a
/// live session and a server, so anything inside it is untestable by
/// construction — M97's finding, applied for the fifth time in this repo.
pub(crate) fn witness_seam_faults(names: &[&str], expected: usize) -> Vec<String> {
    let ids: Vec<usize> = names
        .iter()
        .filter_map(|name| {
            name.strip_prefix('r')?
                .split_whitespace()
                .next()?
                .parse::<usize>()
                .ok()
        })
        .collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    let mut faults = Vec::new();

    let mut dupes: Vec<usize> = Vec::new();
    for w in sorted.windows(2) {
        if w[0] == w[1] && !dupes.contains(&w[0]) {
            dupes.push(w[0]);
        }
    }
    if !dupes.is_empty() {
        faults.push(format!(
            "duplicate witness id(s) {dupes:?} — two milestones minted the same \
             rNN and the merge was silent"
        ));
    }
    if ids.len() != expected {
        faults.push(format!(
            "witness count {} != declared {expected} — bump \
             EXPECTED_RENDER_CHECK_WITNESSES in the same commit that adds a row, \
             and take the next free id from REWO_PLAN §0.0's shared-resource table",
            ids.len()
        ));
    }
    sorted.dedup();
    if let Some(gap) = (1..=sorted.len()).find(|n| sorted.get(n - 1) != Some(n)) {
        faults.push(format!(
            "witness ids are not contiguous from r1 — first gap at r{gap}"
        ));
    }
    faults
}

/// Whether the wavy cape is switched on for this run (M61).
pub(crate) fn wavy_cape_requested(flag: bool) -> bool {
    flag || matches!(
        std::env::var("REWO_WAVY_CAPE").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// Reject a `[0, 1]` option before any I/O so a bad `--gamma` /
/// `--darkness-effect-scale` fails fast with a named error (M13).
pub(super) fn validate_unit(name: &str, v: f32) -> Result<(), String> {
    if !v.is_finite() || !(0.0..=1.0).contains(&v) {
        return Err(format!(
            "--{name} must be a finite value in [0, 1], got {v}"
        ));
    }
    Ok(())
}

impl LiveApp {
    /// M86's gate drives the inventory open for the second half of its run.
    /// 
    /// The screen is one of the paths the bake fix re-enables, and it is the
    /// one carrying the ninth instance of this milestone's bug —
    /// `VelvetTextPass::sync_atlas` destroying its glyph image and rewriting
    /// its descriptor set in place, whose own comment said "the caller is
    /// expected to have idled" of a caller that does not. Nothing reaches it
    /// except an open inventory, so a check that never opens one would grade
    /// that fix not at all. The mouse is parked over the panel so a tooltip
    /// lays out glyphs and the cache actually goes dirty.
    pub(super) fn render_check_inject(&mut self) {
    if let Some(c) = self.check.as_ref() {
        // Half of *this* run, not half of the default: `--render-check
        // --run-seconds 4` would otherwise never reach the trigger and
        // `r16` would fail with the screen having never opened. Caught by
        // exactly that.
        let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
        let half = c.frames > 0 && self.started.elapsed().as_secs_f32() >= limit * 0.5;
        // M89 note: the guard is `!screen_forced_open`, not
        // `!inventory_open()`. Since M89 the injected container opens the
        // screen at 0.4, so an `inventory_open()` guard skips this whole
        // branch — INCLUDING the cursor park below, which is the only
        // thing that lays out a tooltip and therefore the only door to
        // `VelvetTextPass::sync_atlas`. `r16` would have stayed green
        // while no longer proving anything it was written for, which is
        // the same vacuity `p3` exists to catch in `containershot`.
        if half && !self.screen_forced_open {
            self.screen_forced_open = true;
            if !self.screen.inventory_open() {
                self.set_screen_open(true);
            }
            // **After** the open, not before: `grab_for_screen` parks the
            // cursor in the middle of the window, which would overwrite
            // this. Menu slot 36 is hotbar slot 0, which the spawn handler
            // fills with a stack of dirt on the creative test server.
            //
            // The slot choice is not cosmetic. A tooltip is the only thing
            // in this client that lays out Velvet glyphs, an *empty* slot
            // produces none, and without one the glyph cache never goes
            // dirty and `VelvetTextPass::sync_atlas` never runs at all. Two
            // earlier cuts of this — a plausible-looking fraction of the
            // window, then the right slot set before the open — both left
            // the M3 mutation (deleting that path's `wait_idle`) alive
            // through the entire gate.
            if let Some(s) = self.state.as_ref() {
                let e = s.window.inner_size();
                let r = screen_slot_rects(e.width as f32, e.height as f32)
                    [rewo_world::inventory::HOTBAR_MENU_START];
                self.screen.mouse = ((r.0 + r.2 * 0.5) as f64, (r.1 + r.2 * 0.5) as f64);
            }
        }
        // M88 — six-tenths through, open a CONTAINER over the inventory.
        //
        // Injected as a raw `open_screen` body through the production
        // router rather than staged by interacting with a real chest,
        // which is M17's precedent and its reasoning: raw-packet injection
        // into the production dispatcher is the deterministic proof, where
        // a live encounter depends on the server's own timing and on the
        // client aiming at the right block. What is being graded here is
        // the *render*, and this drives the whole chain that feeds it —
        // decode, layout resolution, `Menus::apply_open_screen`, and the
        // frame loop's choice of which menu to draw.
        //
        // A generic_9x3 chest: menu id 2, 63 slots, and a 168-tall panel,
        // so its geometry is distinguishable from the player's 46-slot
        // 166-tall one by more than rounding.
        // Four-tenths through — BEFORE the gate force-opens the inventory
        // at half (M89). The ordering is the witness: if the screen is up
        // between 0.4 and 0.5 it can only be because `open_screen` opened
        // it, which is the behaviour M87 was missing and M89 added. With
        // the injection after the forced open, the container would render
        // either way and `r21` could not tell.
        // M94 — open the recipe book, which no server does unprompted: a
        // fresh player's `RecipeBookSettings` are all shut, so without this
        // the windowed client never reaches the book's draw and `r23` would
        // be measuring nothing. Injected EARLY so it is live while the
        // inventory (a CRAFTING book menu) is on screen for `r16`.
        //
        // Eight booleans, four `(open, filtering)` pairs in
        // `RecipeBookSettings`' positional order — crafting first.
        if !self.book_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.92 {
                if let Some(session) = self.session.as_mut() {
                    let id = session.ids.cb_play_recipe_book_settings;
                    let body = [1u8, 0, 0, 0, 0, 0, 0, 0];
                    session.apply_recipe_book(id, &body);
                    if session.recipe_book_settings.crafting.open {
                        self.book_injected = true;
                    }
                }
            }
        }
        // M110 — force-open the chat screen a fifth of the way in. A
        // windowed run has no keyboard, so without this `T` reaches
        // nothing the gate can see and r27 measures a path no test drives
        // — the M86 shape. It goes in EARLY and stays open: the screen is
        // closed by nothing here, so every later frame carries its bar and
        // the count is unambiguous.
        // M132 — an objective, four scores and a display-slot assignment,
        // as raw bodies through the production router (M17's rule). No
        // server sets a sidebar up unprompted, so without this r42 would
        // be a witness over a path the gate cannot reach.
        //
        // Injected EARLY and never cleared, so the sidebar is live for
        // most of the run and its frame count is unambiguous. The fourth
        // holder is named `#hidden`, which `PlayerScoreEntry.isHidden`
        // filters out: a client that skipped that filter draws nine text
        // lines rather than seven and fails r42 on the count.
        if !self.sidebar_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.1 {
                if let Some(session) = self.session.as_mut() {
                    let obj = "rewo_sidebar";
                    let mc_string = |b: &mut Vec<u8>, s: &str| {
                        rewo_proto::varint::write_varint(b, s.len() as i32);
                        b.extend_from_slice(s.as_bytes());
                    };
                    let nbt_string = |b: &mut Vec<u8>, s: &str| {
                        b.push(8); // TAG_String
                        b.extend_from_slice(&(s.len() as u16).to_be_bytes());
                        b.extend_from_slice(s.as_bytes());
                    };
                    // `set_objective`, METHOD_ADD.
                    let mut body: Vec<u8> = Vec::new();
                    mc_string(&mut body, obj);
                    body.push(0); // ADD
                    nbt_string(&mut body, "Rewo Sidebar");
                    rewo_proto::varint::write_varint(&mut body, 0); // INTEGER
                    body.push(0); // no number format
                    session.inject_packet(session.ids.cb_play_set_objective, &body);

                    // Four holders. The scores are deliberately out of
                    // order on the wire, so a client that drew them in
                    // arrival order rather than by value would place them
                    // differently — which the sort test pins and this
                    // keeps honest end to end.
                    for (owner, value) in
                        [("RewoAlpha", 3), ("#hidden", 999), ("RewoGamma", 9), ("RewoBeta", 6)]
                    {
                        let mut b: Vec<u8> = Vec::new();
                        mc_string(&mut b, owner);
                        mc_string(&mut b, obj);
                        rewo_proto::varint::write_varint(&mut b, value);
                        b.push(0); // no display override
                        b.push(0); // no number format
                        session.inject_packet(session.ids.cb_play_set_score, &b);
                    }

                    // `set_display_objective` — slot 1 is SIDEBAR. Sent
                    // last, because it is what makes the panel appear.
                    let mut b: Vec<u8> = Vec::new();
                    rewo_proto::varint::write_varint(
                        &mut b,
                        rewo_net::scoreboard::DisplaySlot::Sidebar.id(),
                    );
                    mc_string(&mut b, obj);
                    session.inject_packet(session.ids.cb_play_set_display_objective, &b);
                }
                self.sidebar_injected = true;
            }
        }
        // M151 — four players onto the tab list, and hold Tab.
        //
        // Both halves are needed and neither exists in a windowed run: a
        // fresh test server has exactly one player (this client), and
        // there is no keyboard, so without this r47 would be a witness
        // over a path the gate cannot reach — worse than no witness.
        //
        // **Two of the four are `listed: false`**, which is what makes the
        // row count a claim about `getListedOnlinePlayers()` rather than
        // about arrival: the list must come to THREE (the local player
        // plus the two listed ones), where a client that ignored the flag
        // scores five.
        //
        // The key goes down at half, not at the start, so
        // `tab_list_frames` must be strictly less than `frames` — a client
        // that drew the list unconditionally is exactly as wrong as one
        // that never drew it, and a `> 0` threshold cannot tell them
        // apart.
        {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            let elapsed = self.started.elapsed().as_secs_f32();
            if !self.tab_list_injected && elapsed >= limit * 0.45 {
                if let Some(session) = self.session.as_mut() {
                    // ADD_PLAYER (0) | UPDATE_GAME_MODE (2) |
                    // UPDATE_LISTED (3) | UPDATE_LATENCY (4).
                    let mask = (1u8 << 0) | (1u8 << 2) | (1u8 << 3) | (1u8 << 4);
                    let mut body: Vec<u8> = vec![mask];
                    rewo_proto::varint::write_varint(&mut body, 4);
                    // `(name, listed, gamemode, latency)`. The spectator is
                    // named so that it would sort FIRST alphabetically and
                    // must not: `PLAYER_COMPARATOR` puts spectators after
                    // everyone else, one key ahead of the name.
                    for (i, (name, listed, mode, ms)) in [
                        ("RewoZulu", true, 0i32, 10i32),
                        ("RewoAlpha", true, 3, 800),
                        ("RewoHidden", false, 0, 50),
                        ("RewoGhost", false, 3, 50),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        // A uuid far from any real profile's, so a
                        // collision with the local player is impossible.
                        let uuid = 0x5157_0000_0000_0000_0000_0000_0000_0000u128 + i as u128;
                        body.extend_from_slice(&uuid.to_be_bytes());
                        rewo_proto::varint::write_varint(&mut body, name.len() as i32);
                        body.extend_from_slice(name.as_bytes());
                        rewo_proto::varint::write_varint(&mut body, 0); // no properties
                        rewo_proto::varint::write_varint(&mut body, mode);
                        body.push(u8::from(listed));
                        rewo_proto::varint::write_varint(&mut body, ms);
                    }
                    session.inject_packet(session.ids.cb_play_player_info_update, &body);
                }
                // M163 — a zombie whose custom name is a TRANSLATABLE, so
                // r48 can see whether the windowed client's decode reached
                // a language table. Injected as raw bodies through the
                // production router (M17's rule): a live encounter would
                // depend on a server spawning a mob and naming it, which is
                // neither deterministic nor stageable without a third
                // caller requirement (deliberately not added -- §0.0).
                if let Some(session) = self.session.as_mut() {
                    if let Some(zombie) = self.etypes.id_of("minecraft:zombie") {
                        // `add_entity`: id, uuid, type, xyz, LpVec3 zero
                        // sentinel, then pitch / yaw / head-yaw bytes.
                        let mut add: Vec<u8> = Vec::new();
                        rewo_proto::varint::write_varint(&mut add, NAMETAG_WITNESS_ID);
                        add.extend_from_slice(
                            &0x5257_0000_0000_0000_0000_0000_0000_0001u128.to_be_bytes(),
                        );
                        rewo_proto::varint::write_varint(&mut add, zombie);
                        // Beside the PLAYER, not at a fixed point: the
                        // label is gated on `name_tag_distance`, so a
                        // zombie at the origin scores zero for a reason
                        // that has nothing to do with the flatten. The
                        // first cut did exactly that and r48 read 0/0.
                        let p = &session.player;
                        for v in [p.x + 2.0, p.y, p.z] {
                            add.extend_from_slice(&v.to_be_bytes());
                        }
                        add.push(0); // LpVec3's zero sentinel
                        add.extend_from_slice(&[0, 0, 0]); // pitch, yaw, head yaw
                        session.inject_packet(session.ids.cb_play_add_entity, &add);
                        // `set_entity_data`: index 2, OPTIONAL_COMPONENT
                        // (serializer 6), present, `{translate: "..."}`.
                        let key = "entity.minecraft.zombie";
                        let mut meta: Vec<u8> = Vec::new();
                        rewo_proto::varint::write_varint(&mut meta, NAMETAG_WITNESS_ID);
                        meta.extend_from_slice(&[2, 6, 1]);
                        meta.extend_from_slice(&[10, 8]);
                        meta.extend_from_slice(&9u16.to_be_bytes());
                        meta.extend_from_slice(b"translate");
                        meta.extend_from_slice(&(key.len() as u16).to_be_bytes());
                        meta.extend_from_slice(key.as_bytes());
                        meta.push(0); // TAG_End
                        // …and `DATA_CUSTOM_NAME_VISIBLE` (index 3,
                        // BOOLEAN) in the same body, because
                        // `Entity.shouldShowName()` IS
                        // `isCustomNameVisible()` for everything but a
                        // player — a named mob nobody has told to show its
                        // tag draws none, and `resolve_labels` returns
                        // `None` rather than the string.
                        meta.extend_from_slice(&[3, 8, 1]);
                        meta.push(0xFF); // metadata terminator
                        session.inject_packet(session.ids.cb_play_set_entity_data, &meta);
                    }
                }
                // M163 — a SIGN whose front text is a translatable, for
                // r49. Written straight into the world rather than
                // injected as two packets, and the reason is a decode rule
                // rather than convenience: `World::set_block_entity_data`
                // keeps vanilla's "a position with no block entity is
                // ignored" (`lib.rs:218-221`), so a `block_entity_data`
                // packet cannot CREATE one — a block entity arrives only
                // in a chunk payload, and staging a real sign would mean a
                // third caller requirement plus an SNBT form nothing here
                // can check.
                //
                // What r49 grades is downstream of that anyway: the claim
                // is that the windowed frame loop hands the language table
                // to the sign collector, not that a block entity can be
                // conjured mid-session.
                if let Some(session) = self.session.as_mut() {
                    if let Some(state) = self.blocks.default_state("minecraft:oak_sign") {
                        let (dx, dy, dz) = SIGN_WITNESS_OFFSET;
                        let (x, y, z) = (
                            session.player.x.floor() as i32 + dx,
                            session.player.y.floor() as i32 + dy,
                            session.player.z.floor() as i32 + dz,
                        );
                        session.world.set_block(x, y, z, state);
                        let face = rewo_proto::nbt::Nbt::Compound(vec![(
                            "messages".to_string(),
                            rewo_proto::nbt::Nbt::List(vec![
                                rewo_proto::nbt::Nbt::Compound(vec![(
                                    "translate".to_string(),
                                    rewo_proto::nbt::Nbt::String(
                                        SIGN_WITNESS_KEY.to_string(),
                                    ),
                                )]),
                                rewo_proto::nbt::Nbt::String(String::new()),
                                rewo_proto::nbt::Nbt::String(String::new()),
                                rewo_proto::nbt::Nbt::String(String::new()),
                            ]),
                        )]);
                        session.world.block_entities.insert(
                            rewo_world::block_entities::BlockEntityPos { x, y, z },
                            rewo_world::block_entities::BlockEntity {
                                // The type id is not read by the sign path
                                // — `collect_sign_text` keys off the BLOCK
                                // STATE through `SignStates::get` — so this
                                // is a marker rather than a claim about the
                                // block-entity registry.
                                type_id: -1,
                                data: rewo_proto::nbt::Nbt::Compound(vec![(
                                    "front_text".to_string(),
                                    face,
                                )]),
                            },
                        );
                        log::info!("render-check: sign witness at ({x},{y},{z})");
                    }
                }
                self.tab_list_injected = true;
            }
            // r48 — TWO mobs of one type plus one of another, injected
            // through the production `add_entity` route (M17/M88's rule:
            // injection is the deterministic proof where a live encounter
            // depends on the server's spawn timing).
            //
            // This is the only check anywhere that drives
            // `etypes.name(type_id)` -> `kind_for_entity_name`: every
            // serverless gate, this milestone's `mobtexshot` included,
            // hands `kind` straight to `EntityDraw`. It is also the only
            // one that puts more than one mob in the WINDOWED client's
            // draw list, which is the axis §0.0's "a mob renders with
            // another mob's texture when more than one is in the scene"
            // report is defined by.
            //
            // The ids are fixed and named in `CROWD_IDS` because the census
            // below joins on them: without that, the count is a claim about
            // whatever the world contains rather than about this injection.
            if !self.crowd_injected && elapsed >= limit * 0.3 {
                if let Some(session) = self.session.as_mut() {
                    let types = session.entity_types.clone();
                    let p = &session.player;
                    let eye = [p.x, p.y, p.z];
                    if let Some(types) = types {
                        let spawn = |eid: i32, tid: i32, dx: f64| -> Vec<u8> {
                            let mut b: Vec<u8> = Vec::new();
                            rewo_proto::varint::write_varint(&mut b, eid);
                            // A uuid far from any real profile's.
                            let uuid =
                                0xC0D0_0000_0000_0000_0000_0000_0000_0000u128 + eid as u128;
                            b.extend_from_slice(&uuid.to_be_bytes());
                            rewo_proto::varint::write_varint(&mut b, tid);
                            b.extend_from_slice(&(eye[0] + dx).to_be_bytes());
                            b.extend_from_slice(&eye[1].to_be_bytes());
                            b.extend_from_slice(&(eye[2] - 4.0).to_be_bytes());
                            // `LpVec3`'s one-byte zero sentinel: no motion.
                            b.push(0);
                            b.extend_from_slice(&[0, 0, 0]); // pitch, yaw, head yaw
                            b
                        };
                        let pid = session.ids.cb_play_add_entity;
                        for (i, (name, dx)) in [
                            ("minecraft:zombie", -1.5f64),
                            ("minecraft:zombie", 0.0),
                            ("minecraft:villager", 1.5),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            if let Some(tid) = types.id_of(name) {
                                let body = spawn(CROWD_IDS[i], tid, dx);
                                session.inject_packet(pid, &body);
                            }
                        }
                    }
                }
                self.crowd_injected = true;
            }
            // M173 — walk the options pages through the PRODUCTION open
            // and close paths (r62): the Sound page at 0.06 (sliders
            // sampled while it is up), Done-equivalent closes at 0.12
            // (to the root), 0.14 (to the pause screen) and 0.16 (back
            // to the game), so nothing later meets a leftover screen or
            // a stale `self.view`.
            if self.options_staged == 0 && elapsed >= limit * 0.06 {
                self.options_staged = 1;
                self.open_options_screen(rewo_world::options_screen::OptionsPage::Sound);
            }
            if self.options_staged == 1 && elapsed >= limit * 0.12 {
                self.options_staged = 2;
                self.close_options(); // Sound -> Root, saving options.txt
            }
            if self.options_staged == 2 && elapsed >= limit * 0.14 {
                self.options_staged = 3;
                self.close_options(); // Root -> the pause screen
            }
            if self.options_staged == 3 && elapsed >= limit * 0.16 {
                self.options_staged = 4;
                self.close_view_screen(); // pause -> the game
            }
            // M169 — mount the bot on a saddled horse by INJECTION, the way
            // r48's crowd and r19's chest are staged (M17: injection is the
            // deterministic proof where a live encounter depends on the
            // server). The `/ride` command does not survive the render check
            // — the server never sent `set_passengers` naming the bot, so
            // `mounts` stayed empty and `jumpable_vehicle()` was always None.
            // Three packets through the production router: add the horse,
            // give it a saddle (slot 7), seat the bot on it. Before the
            // 0.32 hold so the meter has a vehicle to charge against.
            if !self.jump_injected && elapsed >= limit * 0.28 {
                if let Some(session) = self.session.as_mut() {
                    let types = session.entity_types.clone();
                    if let (Some(types), Some(me)) = (types, session.player_id) {
                        if let Some(horse) = types.id_of("minecraft:horse") {
                            const HORSE_EID: i32 = 0x00A9_0000;
                            let p = &session.player;
                            let mut add: Vec<u8> = Vec::new();
                            rewo_proto::varint::write_varint(&mut add, HORSE_EID);
                            let uuid = 0xC0D0_0000_0000_0000_0000_0000_00A9_0000u128;
                            add.extend_from_slice(&uuid.to_be_bytes());
                            rewo_proto::varint::write_varint(&mut add, horse);
                            add.extend_from_slice(&p.x.to_be_bytes());
                            add.extend_from_slice(&p.y.to_be_bytes());
                            add.extend_from_slice(&p.z.to_be_bytes());
                            add.push(0); // LpVec3 zero sentinel — no motion
                            add.extend_from_slice(&[0, 0, 0]); // pitch, yaw, head yaw
                            session.inject_packet(session.ids.cb_play_add_entity, &add);
                            // set_equipment: the saddle in slot 7. A minimal
                            // present stack (count 1, an arbitrary valid id,
                            // an empty patch); `saddled` only tests non-empty.
                            let mut eq: Vec<u8> = Vec::new();
                            rewo_proto::varint::write_varint(&mut eq, HORSE_EID);
                            eq.push(7); // SADDLE ordinal, top bit clear = last
                            rewo_proto::varint::write_varint(&mut eq, 1); // count
                            rewo_proto::varint::write_varint(&mut eq, 1); // item id (any)
                            rewo_proto::varint::write_varint(&mut eq, 0); // added
                            rewo_proto::varint::write_varint(&mut eq, 0); // removed
                            session.inject_packet(session.ids.cb_play_set_equipment, &eq);
                            // set_passengers: seat the bot on the horse.
                            let mut pass: Vec<u8> = Vec::new();
                            rewo_proto::varint::write_varint(&mut pass, HORSE_EID);
                            rewo_proto::varint::write_varint(&mut pass, 1);
                            rewo_proto::varint::write_varint(&mut pass, me);
                            session.inject_packet(session.ids.cb_play_set_passengers, &pass);
                        }
                    }
                }
                self.jump_injected = true;
            }
            // M170 — a leashed cow tied to a fence knot, injected so
            // `collect_leashes` builds a ribbon and the pass draws it.
            // The leash decode (`set_entity_link`) is graded by
            // `rideshot`; this proves the render path only a live frame
            // reaches. Same injection technique as the horse above.
            if !self.leash_injected && elapsed >= limit * 0.28 {
                if let Some(session) = self.session.as_mut() {
                    let types = session.entity_types.clone();
                    if let Some(types) = types {
                        if let (Some(cow), Some(knot)) = (
                            types.id_of("minecraft:cow"),
                            types.id_of("minecraft:leash_knot"),
                        ) {
                            const COW_EID: i32 = 0x00C0_0000;
                            const KNOT_EID: i32 = 0x00C0_0001;
                            let p = &session.player;
                            let add = |eid: i32, tid: i32, x: f64, y: f64, z: f64| {
                                let mut b: Vec<u8> = Vec::new();
                                rewo_proto::varint::write_varint(&mut b, eid);
                                b.extend_from_slice(&((eid as u128) << 8).to_be_bytes());
                                rewo_proto::varint::write_varint(&mut b, tid);
                                b.extend_from_slice(&x.to_be_bytes());
                                b.extend_from_slice(&y.to_be_bytes());
                                b.extend_from_slice(&z.to_be_bytes());
                                b.push(0);
                                b.extend_from_slice(&[0, 0, 0]);
                                b
                            };
                            let cow_body = add(COW_EID, cow, p.x + 2.0, p.y, p.z);
                            let knot_body = add(KNOT_EID, knot, p.x + 3.0, p.y + 1.0, p.z);
                            session.inject_packet(session.ids.cb_play_add_entity, &cow_body);
                            session.inject_packet(session.ids.cb_play_add_entity, &knot_body);
                            // set_entity_link: source cow, dest knot —
                            // two fixed BE i32s.
                            let mut link: Vec<u8> = Vec::new();
                            link.extend_from_slice(&COW_EID.to_be_bytes());
                            link.extend_from_slice(&KNOT_EID.to_be_bytes());
                            session.inject_packet(session.ids.cb_play_set_entity_link, &link);
                        }
                    }
                }
                self.leash_injected = true;
            }
            // M172 — a written book into hotbar slot 0 + an `open_book`,
            // both through the production router. The window is
            // 0.465..0.5: after the jump hold (ends 0.46), before the
            // 0.5 inventory force-open — which then REPLACES the reader
            // (`Screens` is one slot) and exercises the stale-book sync,
            // so every later screen-dependent witness (r22-r25 at
            // 0.90-0.95) keeps its stage. The first cut injected at 0.93
            // and r24's overlay lost its inventory.
            if !self.book_view_injected && elapsed >= limit * 0.465 {
                if let Some(session) = self.session.as_mut() {
                    let book_item = self.items.id("minecraft:written_book");
                    let comp = session.swing_data.as_ref().map(|d| d.components);
                    if let (Some(item), Some(comp)) = (book_item, comp) {
                        let pstr = |b: &mut Vec<u8>, t: &str| {
                            rewo_proto::varint::write_varint(b, t.len() as i32);
                            b.extend_from_slice(t.as_bytes());
                        };
                        let snbt = |b: &mut Vec<u8>, t: &str| {
                            b.push(8); // TAG_String
                            b.extend_from_slice(&(t.len() as u16).to_be_bytes());
                            b.extend_from_slice(t.as_bytes());
                        };
                        // written_book_content: Filterable<title>, author,
                        // generation, pages, resolved.
                        let mut value: Vec<u8> = Vec::new();
                        pstr(&mut value, "Gate Book");
                        value.push(0);
                        pstr(&mut value, "r61");
                        rewo_proto::varint::write_varint(&mut value, 0);
                        rewo_proto::varint::write_varint(&mut value, 2);
                        snbt(&mut value, "the first page");
                        value.push(0);
                        snbt(&mut value, "the second page");
                        value.push(0);
                        value.push(0);
                        // set_player_inventory: VarInt INVENTORY slot 0
                        // (hotbar 0 = menu 36 — the app's hotbar_slot is 0,
                        // which is the slot hand 0 resolves), then the stack.
                        let mut body: Vec<u8> = Vec::new();
                        rewo_proto::varint::write_varint(&mut body, 0);
                        rewo_proto::varint::write_varint(&mut body, 1); // count
                        rewo_proto::varint::write_varint(&mut body, item);
                        rewo_proto::varint::write_varint(&mut body, 1); // added
                        rewo_proto::varint::write_varint(&mut body, 0); // removed
                        rewo_proto::varint::write_varint(
                            &mut body,
                            comp.written_book_content,
                        );
                        body.extend_from_slice(&value);
                        session.inject_packet(session.ids.cb_play_set_player_inventory, &body);
                        // open_book: one enum ordinal — hand 0.
                        session
                            .inject_packet(session.ids.cb_play_open_book, &[0]);
                    }
                }
                self.book_view_injected = true;
            }
            // The hold. Assigned every frame rather than latched, so it is
            // the same "read the key state fresh" the real gate does.
            self.keys.tab_list = elapsed >= limit * 0.5;
            // M169 — hold Space for a tenth of the run while mounted on
            // the staged horse: the meter climbs for ~16 ticks and the
            // release sends `START_RIDING_JUMP`. A level, like Tab.
            self.keys.jump = elapsed >= limit * 0.32 && elapsed < limit * 0.46;
        }
        // M162 — the two packet tails that carry a sound. Both call sites
        // live inside `PlaySession`, which owns a socket and cannot be
        // built in a unit test (M71), so this is the only check that can
        // reach them: `soundshot` grades the arithmetic and would stay
        // green with `queue_explosion_sound` deleted and `camera_eye`
        // returning `None` forever.
        //
        // Raw bodies through the production dispatcher (M17's rule), and
        // late enough in the run that `spawned` is true — before the
        // server's first `player_position` the camera is legitimately
        // absent and 1023 is correctly silent.
        // M166 — the PLAY-state `resource_pack_push`, injected because
        // NOTHING ELSE REACHES IT. A vanilla server pushes its pack during
        // configuration, so the play arm is only taken by `/resourcepack`
        // or a plugin, and without this the whole arm could be deleted with
        // every gate green — the M45/M158 shape, caught by re-reading this
        // milestone's own diff rather than by any check.
        //
        // Raw body through the production dispatcher (M17's rule), with a
        // DIFFERENT UUID from the configuration one so r55's list has to
        // grow rather than merely be non-empty.
        if !self.play_pack_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.55 {
                if let Some(session) = self.session.as_mut() {
                    let mut b = PLAY_PACK_ID.to_be_bytes().to_vec();
                    // An `ftp:` URL, so this one must come back INVALID_URL
                    // and the two replies differ in their ACTION as well as
                    // their id. A second FAILED_DOWNLOAD would be satisfied
                    // by a client that ignored the url entirely.
                    for t in ["ftp://example.invalid/mid-session.zip", ""] {
                        rewo_proto::varint::write_varint(&mut b, t.len() as i32);
                        b.extend_from_slice(t.as_bytes());
                    }
                    b.push(0); // required = false
                    b.push(0); // no prompt
                    let pid = session.ids.cb_play_resource_pack_push;
                    session.inject_packet(pid, &b);
                    // Inside the `if let`, not after it: a run that reached
                    // the deadline before the session existed would
                    // otherwise mark itself injected and never inject.
                    self.play_pack_injected = true;
                }
            }
        }
        if !self.sound_tails_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.3 {
                let explode_holder = self
                    .sounds
                    .registry
                    .id_of("minecraft:entity.generic.explode")
                    .map(|id| id + 1);
                let mut recorded: Option<([f64; 3], [f64; 3])> = None;
                if let (Some(session), Some(holder)) = (self.session.as_mut(), explode_holder) {
                    let eye = [session.player.x, session.player.eye_y(), session.player.z];
                    let block = |dx: i64, dy: i64, dz: i64| {
                        let x = eye[0].floor() as i64 + dx;
                        let y = eye[1].floor() as i64 + dy;
                        let z = eye[2].floor() as i64 + dz;
                        let packed = ((x & 0x3FF_FFFF) << 38)
                            | ((z & 0x3FF_FFFF) << 12)
                            | (y & 0xFFF);
                        (x, y, z, packed)
                    };
                    let level_event = |kind: i32, packed: i64, global: bool| {
                        let mut b = kind.to_be_bytes().to_vec();
                        b.extend_from_slice(&packed.to_be_bytes());
                        b.extend_from_slice(&0i32.to_be_bytes());
                        b.push(u8::from(global));
                        b
                    };

                    // 1023, a wither spawning 40 blocks north-east and 12
                    // up: far enough that "the sound is 2 blocks from the
                    // listener" cannot be confused with "the sound is at
                    // the block".
                    let (bx, by, bz, packed) = block(40, 12, 40);
                    recorded = Some((
                        eye,
                        [bx as f64 + 0.5, by as f64 + 0.5, bz as f64 + 0.5],
                    ));
                    session.inject_packet(
                        session.ids.cb_play_level_event.unwrap_or(-1),
                        &level_event(1023, packed, true),
                    );

                    // 3012, a trial spawner 200 blocks away: past the
                    // strict `distanceToSqr > 100.0` gate, so it must be
                    // QUEUED rather than played.
                    let (_, _, _, far) = block(200, 0, 0);
                    session.inject_packet(
                        session.ids.cb_play_level_event.unwrap_or(-1),
                        &level_event(3012, far, false),
                    );

                    // An `explode` whose TAIL carries the sound. The
                    // particle is named rather than numbered, through the
                    // session's own report-backed registry — a hardcoded id
                    // would fail closed for the wrong reason if the
                    // registry were ever renumbered.
                    if let Some(emitter) =
                        session.particle_types().id_of("minecraft:explosion_emitter")
                    {
                        let poof = session.particle_types().id_of("minecraft:poof");
                        let smoke = session.particle_types().id_of("minecraft:smoke");
                        if let (Some(poof), Some(smoke)) = (poof, smoke) {
                            let mut b = Vec::new();
                            for v in [eye[0], eye[1] - 1.0, eye[2]] {
                                b.extend_from_slice(&v.to_be_bytes());
                            }
                            b.extend_from_slice(&4.0f32.to_be_bytes());
                            b.extend_from_slice(&37i32.to_be_bytes()); // fixed i32
                            b.push(0); // no playerKnockback
                            rewo_proto::varint::write_varint(&mut b, emitter);
                            rewo_proto::varint::write_varint(&mut b, holder);
                            rewo_proto::varint::write_varint(&mut b, 2);
                            for id in [poof, smoke] {
                                rewo_proto::varint::write_varint(&mut b, id);
                                b.extend_from_slice(&0.5f32.to_be_bytes());
                                b.extend_from_slice(&1.0f32.to_be_bytes());
                                rewo_proto::varint::write_varint(&mut b, 1);
                            }
                            session.inject_packet(session.ids.cb_play_explode, &b);
                        }
                    }
                }
                if let (Some(c), Some((eye, target))) = (self.check.as_mut(), recorded) {
                    c.global_event_eye = Some(eye);
                    c.global_event_target = Some(target);
                }
                self.sound_tails_injected = true;
            }
        }
        if !self.chat_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.2 {
                // M111 — 25 lines, because the scrollbar's guard is
                // `virtualHeight != chatHeight`: it does not exist until
                // there is more chat than the focused box's twenty rows,
                // and a run's own join messages come to about six. Without
                // these r28 would be a witness over a path the gate cannot
                // reach, which is worse than no witness.
                //
                // Injected as raw `system_chat` bodies through the
                // production router (M17's rule: injection is the
                // deterministic proof where a live trigger depends on
                // timing nothing here controls).
                if let Some(session) = self.session.as_mut() {
                    let id = session.ids.cb_play_system_chat;
                    for i in 0..25u8 {
                        let text = format!("scrollbar filler {i}");
                        let mut body: Vec<u8> = vec![8];
                        body.extend_from_slice(&(text.len() as u16).to_be_bytes());
                        body.extend_from_slice(text.as_bytes());
                        body.push(0); // overlay = false
                        if let Some(pid) = id {
                            session.inject_packet(pid, &body);
                        }
                    }
                }
                // M126d — one message carrying legacy codes, so the
                // drawn chat has more than one colour and at least one
                // non-plain flag. Injected as a raw `system_chat` body
                // through the production router (M17's rule) rather than
                // sent as chat, because a server owns what it echoes back
                // and `§` in a player message is not something the gate
                // can rely on surviving the round trip.
                //
                // Every stage has to work for this to score: the NBT
                // string reaches `parse_component`, `push_legacy` resolves
                // the codes into five spans, `FlatComponents` carries them
                // through the wrap as separate parts, and `chat_lines`
                // emits one text line each. A flatten anywhere in that
                // chain drops both counters to zero.
                if let Some(session) = self.session.as_mut() {
                    if let Some(pid) = session.ids.cb_play_system_chat {
                        let text = concat!(
                            "\u{00a7}crewored \u{00a7}9blue ",
                            "\u{00a7}oital \u{00a7}nunder \u{00a7}mstrike"
                        );
                        let mut body: Vec<u8> = vec![8];
                        body.extend_from_slice(&(text.len() as u16).to_be_bytes());
                        body.extend_from_slice(text.as_bytes());
                        body.push(0); // overlay = false
                        session.inject_packet(pid, &body);
                    }
                }
                // M127 — two `disguised_chat` bodies, bound to the
                // SERVER's own chat types by name. `handleDisguisedChat`
                // is the shortest path to a decorated line: a component,
                // a bound, and no signature machinery. Injected as raw
                // bodies through the production router (M17's rule).
                //
                // The ids are looked up rather than assumed, because the
                // registry is datapack-driven and the index is whatever
                // the server's packs say. That also makes the lookup part
                // of the claim: a client that never parsed
                // `registry_data` for this registry answers `None` here
                // and injects nothing, so r40 and r41 read zero instead of
                // quietly decorating with entry 0.
                if let Some(session) = self.session.as_mut() {
                    let pid = session.ids.cb_play_disguised_chat;
                    let plain = session.chat_type_id("minecraft:chat");
                    let whisper =
                        session.chat_type_id("minecraft:msg_command_incoming");
                    let body = |content: &str, name: &str, id: i32| {
                        let mut b: Vec<u8> = vec![8];
                        b.extend_from_slice(&(content.len() as u16).to_be_bytes());
                        b.extend_from_slice(content.as_bytes());
                        // `ByteBufCodecs.holder` — id + 1, 0 meaning inline.
                        rewo_proto::varint::write_varint(&mut b, id + 1);
                        b.push(8);
                        b.extend_from_slice(&(name.len() as u16).to_be_bytes());
                        b.extend_from_slice(name.as_bytes());
                        b.push(0); // no targetName
                        b
                    };
                    if let Some(id) = plain {
                        let b = body("decorated", "RewoDecoWitness", id);
                        session.inject_packet(pid, &b);
                    }
                    if let Some(id) = whisper {
                        let b = body("whispered", "RewoStyleWitness", id);
                        session.inject_packet(pid, &b);
                }
                }
                // M128 — one message carrying a `click_event`, so the
                // gate can click it. A COMPOUND rather than a bare string,
                // because a `click_event` has nowhere to live on a string
                // component; assembled as network NBT bytes by hand, which
                // is what `system_chat`'s content is.
                if let Some(session) = self.session.as_mut() {
                    if let Some(pid) = session.ids.cb_play_system_chat {
                        let body = click_witness_body();
                        session.inject_packet(pid, &body);
                    }
                }
                // M115 — a completion word, then a keystroke, so r29
                // measures the whole production chain rather than a
                // hand-built `Suggestions`: the packet reaches
                // `SuggestionProviderState`, `tab_words()` unions it with
                // the online players, `on_edited` matches the typed prefix
                // against it, and `auto_show` opens the list. A break
                // anywhere in that drops the count to zero.
                //
                // The word is deliberately nothing a server would send, so
                // it cannot be confused with a real player's name, and it
                // begins with the character typed below.
                if let Some(session) = self.session.as_mut() {
                    if let Some(pid) = Some(session.ids.cb_play_custom_chat_completions) {
                        let words = ["rewopopupwitness", "rewopopupsecond"];
                        let mut body: Vec<u8> = Vec::new();
                        body.push(2); // Action.SET
                        body.push(words.len() as u8);
                        for w in words {
                            body.push(w.len() as u8);
                            body.extend_from_slice(w.as_bytes());
                        }
                        session.inject_packet(pid, &body);
                    }
                }
                self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Message);
                // `onEdited` is what turns suggestions on and asks for
                // them; nothing else in a windowed run types.
                self.chat_char('r');
                self.chat_injected = true;
            }
        }
        // M128 — click the link, once, at the position the renderer
        // reported for it. Between the chat screen opening (0.2) and the
        // command screen replacing it (0.55), so the popup under the input
        // bar cannot be what answers.
        if self.chat_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            let due = self.started.elapsed().as_secs_f32() >= limit * 0.4;
            let at = self.check.as_ref().and_then(|c| {
                (!c.chat_click_fired).then_some(c.chat_link_at).flatten()
            });
            if let (true, Some((lx, ly))) = (due, at) {
                if let Some(c) = self.check.as_mut() {
                    c.chat_click_fired = true;
                }
                // Four pixels down and one right of the line's origin —
                // inside the first glyph's `[x, x + advance) x [y, y + 9)`
                // box at any GUI scale, because both are screen pixels and
                // the box is at least that big.
                self.screen.mouse = ((lx + 1.0) as f64, (ly + 4.0) as f64);
                // …and close the suggestion popup first, because
                // `ChatScreen::mouse_clicked` gives it precedence over the
                // link (`if self.suggestions.mouse_clicked(..) { return
                // Handled }`) and `SuggestionsList` claims ANY click inside
                // its rect. That is vanilla's ordering, not a bug.
                //
                // The comment above used to argue the popup "cannot be what
                // answers" because the click sits between the screen opening
                // and the command screen replacing it. That held on M128's
                // own branch and stopped holding when M127c's decoration
                // witnesses joined the same injection block: the clickable
                // row is the NEWEST message, so it draws nearest the input
                // bar — which is exactly where the popup is. The click
                // answered `Handled`, the popup's answer, not the link's.
                //
                // Closing it does not weaken the claim. r42 asks whether a
                // click on a chat link runs its command; the popup is a
                // different widget, and r29 already witnesses that it draws.
                if let Some(cs) = self.chat_screen.as_mut() {
                    cs.suggestions.hide();
                }
                let outcome = self.chat_mouse_pressed(0);
                if outcome
                    == rewo_world::chat_screen::ChatClick::RunCommand(
                        CLICK_WITNESS_COMMAND.into(),
                    )
                {
                    if let Some(c) = self.check.as_mut() {
                        c.chat_click_ok += 1;
                    }
                } else {
                    log::warn!("rendercheck: chat click answered {outcome:?}");
                }
            }
        }
        // M116 — later, and as its own screen, so r29's frames stay
        // unambiguously the message popup's. Reopening with `Command`
        // seeds the field with `/`; one letter then reaches the top-level
        // literals, which the dispatcher answers without a packet.
        if self.chat_injected && !self.command_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.55 {
                self.close_chat_screen();
                self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Command);
                self.chat_char('g');
                // …and then on to `/give `, which is where an ARGUMENT is
                // expected: the integer child suggests nothing, so no
                // popup opens and the USAGE box takes its place. r30 has
                // already counted the literal completion `/g` produced;
                // this reaches r31 and r32.
                // …then on to `/give @s `, which is two arguments deep.
                // M118 changed what `/give ` alone shows: its first
                // argument is `minecraft:entity`, so a SELECTOR POPUP
                // opens there and the usage box is suppressed by the
                // mutual exclusion — which is what vanilla does too. The
                // box needs an argument that suggests nothing, and the
                // item after the targets is one.
                // …then on to `/give @s dirt `, which is three arguments
                // deep. **r32's precondition recedes by one word for every
                // argument type transcribed**: M118 made `/give ` open a
                // selector popup and M119 made `/give @s ` open an item
                // one, each time suppressing the usage box by the mutual
                // exclusion. The count after the item is a plain integer
                // and suggests nothing, which is what the box needs.
                for ch in "ive @s dirt ".chars() {
                    self.chat_char(ch);
                }
                self.command_injected = true;
            }
        }
        // M120 — and a coordinate. Injected LAST, at 0.8, so r32 has
        // already banked its frames against `/give @s dirt `: a
        // coordinate popup suppresses the box, which is the receding
        // precondition M118 and M119 both hit.
        if self.command_injected && !self.coords_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.8 {
                self.close_chat_screen();
                // **Drop the draft first.** `close_chat_screen` saves one,
                // and `ChatMethod::Command` restores a COMMAND draft — so
                // reopening handed the field back `/give @s dirt ` and the
                // typing below appended to it. That is M110's
                // `isDraftRestorable` working exactly as documented, and
                // the gate read as "the coordinate family offers nothing"
                // when what it had actually typed was
                // `/give @s dirt setblock `.
                self.chat_draft = None;
                self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Command);
                for ch in "setblock ".chars() {
                    self.chat_char(ch);
                }
                self.coords_injected = true;
            }
        }
        // M124 — a literal table, injected LAST at 0.9 for the reason
        // every one of these has been: whichever popup is open last
        // suppresses the usage box, and r32 banks its frames earlier.
        if self.coords_injected && !self.literal_table_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.9 {
                self.close_chat_screen();
                // The draft drop M120 needed, for M110's reason.
                self.chat_draft = None;
                self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Command);
                for ch in "scoreboard objectives setdisplay ".chars() {
                    self.chat_char(ch);
                }
                self.literal_table_injected = true;
            }
        }
        // M134 — a command that does not exist, which is the shortest
        // route to `updateUsageInfo`'s THIRD branch and therefore to an
        // exception message. Injected after every other typing stage
        // because it is the one that must not have a popup over it: no
        // literal matches, so no completion is offered, so
        // `extractRenderState` falls through to the usage box. Every
        // earlier stage exists to open a popup, and a popup would hide
        // this.
        //
        // Nothing is recorded in `getExceptions()` for this input — the
        // root's children are all literals, none matched, and
        // `getRelevantNodes` therefore returned the empty argument set —
        // so this reaches the line through `Commands.getParseException`
        // rather than through the exception loop. That is the branch M117
        // omitted entirely.
        if self.literal_table_injected && !self.bad_command_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.95 {
                self.close_chat_screen();
                // The draft drop M120 needed, for M110's reason.
                self.chat_draft = None;
                self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Command);
                for ch in "rewonosuchcommand".chars() {
                    self.chat_char(ch);
                }
                self.bad_command_injected = true;
            }
        }
        if !self.container_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.4 {
                if let Some(session) = self.session.as_mut() {
                    // VarInt container id, VarInt menu type (RAW, not a
                    // holder), then an NBT string title.
                    let mut body: Vec<u8> = vec![7, 2, 8];
                    let title = b"Chest";
                    body.extend_from_slice(&(title.len() as u16).to_be_bytes());
                    body.extend_from_slice(title);
                    let id = session.ids.cb_play_open_screen;
                    let opened =
                        rewo_net::route_menu(id, &body, &session.ids, &mut session.menus)
                            && session.menus.open().is_some();
                    if opened {
                        self.container_injected = true;
                    }
                }
            }
        }
        // M174 — open the staged oak_sign's editor through the PRODUCTION
        // decode (`inject_packet` -> the play dispatch), at 0.80: after
        // the container's r19/r20 have latched, late enough that the
        // inventory (force-opened at 0.5) keeps the >25%-of-frames
        // window r16 requires — the first cut injected at 0.55 and
        // starved it to 21% — and before the 0.90 book-menu injection
        // whose replacement exercises r64's commit.
        if !self.sign_editor_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.80 {
                if let Some(session) = self.session.as_mut() {
                    let (dx, dy, dz) = SIGN_WITNESS_OFFSET;
                    let (x, y, z) = (
                        session.player.x.floor() as i32 + dx,
                        session.player.y.floor() as i32 + dy,
                        session.player.z.floor() as i32 + dz,
                    );
                    // Packed BlockPos: x<<38 | z<<12 | y — the packing
                    // `sign_update_body` writes and `position()` reads.
                    let packed: u64 = (((x as i64 & 0x3ff_ffff) as u64) << 38)
                        | (((z as i64 & 0x3ff_ffff) as u64) << 12)
                        | ((y as i64 & 0xfff) as u64);
                    let mut body = Vec::new();
                    body.extend_from_slice(&packed.to_be_bytes());
                    body.push(1); // isFrontText
                    session.inject_packet(session.ids.cb_play_open_sign_editor, &body);
                    self.sign_editor_injected = true;
                }
            }
        }
        // M94 — last of all, a CRAFTING TABLE, because the book only draws
        // while a book menu is on screen and neither of the two injections
        // below is one: `book_type_of` answers for the player's own
        // inventory, `crafting`, and the three furnaces, and nothing else.
        // The chest opened at 0.4 holds the screen for the rest of the run,
        // so without this the windowed client never reaches the book's
        // builder however long it runs — the same shape of gap as M92's
        // overlay injection, one screen over.
        //
        // Last (0.90) so `r20` and `r22` have already latched: a crafting
        // table's panel is 176x166, the player's own size, and it draws no
        // overlays.
        if !self.book_menu_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.90 {
                if let Some(session) = self.session.as_mut() {
                    // Menu type 12. NOT 13 - that is `enchantment`,
                    // which this first got wrong, and which opened a
                    // perfectly valid screen with no book.
                    let mut body: Vec<u8> = vec![11, 12, 8];
                    let title = b"Crafting";
                    body.extend_from_slice(&(title.len() as u16).to_be_bytes());
                    body.extend_from_slice(title);
                    let open_id = session.ids.cb_play_open_screen;
                    if rewo_net::route_menu(open_id, &body, &session.ids, &mut session.menus) {
                        self.book_menu_injected = true;
                    }
                }
            }
        }
        // M104 — and then the which-of-these overlay, through the SAME
        // `open_overlay` a right-click calls.
        //
        // Injected rather than clicked because a click needs the cursor
        // over a particular cell AND a server that has sent a multi-recipe
        // group, neither of which this gate controls — M17's rule. It must
        // come after the crafting table (0.90), because `set_screen_open`
        // clears the overlay: a snapshot of a page must not survive the
        // screen it was taken on.
        if !self.book_overlay_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.95 && self.book_menu_injected {
                // Only `filtering` and `furnace_family` are read out of
                // the view, and both are false for a crafting table's
                // unfiltered book — the rest is named so the fixture reads
                // as a whole `BookView` rather than a partial one.
                let view = rewo_world::recipe_book_screen::BookView {
                    tabs: rewo_world::recipe_book_screen::CRAFTING_TABS.len(),
                    selected_tab: 0,
                    page: 0,
                    total_pages: 1,
                    shown: 1,
                    filtering: false,
                    furnace_family: false,
                };
                let collection = (0..3)
                    .map(|i| rewo_world::recipe_overlay::Button {
                        recipe: i,
                        craftable: i == 0,
                        slots: Vec::new(),
                    })
                    .collect();
                self.screen.book_overlay = Some(open_overlay(collection, 0, view));
                self.book_overlay_injected = true;
            }
        }
        // M92 — near the end, replace it with a BREWING STAND and give it
        // data, so the frame loop has to reach the overlay builder.
        //
        // A chest has no overlays at all, so the injection above cannot
        // exercise this path however long it runs. `containershot` grades
        // the overlays offscreen; without this, nothing says the windowed
        // client ever draws one — the gap M88 closed for the panel and M86
        // for nine features before that.
        //
        // Late (0.85) and after `r20` has latched its height, because a
        // brewing stand's panel is 166 tall — the same as the player's —
        // and so cannot serve `r20`'s discrimination.
        if !self.brewing_injected {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.85 {
                if let Some(session) = self.session.as_mut() {
                    let mut body: Vec<u8> = vec![9, 11, 8];
                    let title = b"Brewing Stand";
                    body.extend_from_slice(&(title.len() as u16).to_be_bytes());
                    body.extend_from_slice(title);
                    let open_id = session.ids.cb_play_open_screen;
                    if rewo_net::route_menu(open_id, &body, &session.ids, &mut session.menus) {
                        // ...and its data: 200 ticks left of a brew, 20
                        // charges of fuel. Both packets through the
                        // production router (M17's precedent), so the
                        // decode, the id gate and the data-slot write are
                        // all the shipped ones.
                        let data_id = session.ids.cb_play_container_set_data;
                        for (slot, value) in [(0i16, 200i16), (1, 20)] {
                            // VarInt container id, then two BE i16s —
                            // fixed-width shorts among the var-ints (M87).
                            let mut d = vec![9u8];
                            d.extend_from_slice(&slot.to_be_bytes());
                            d.extend_from_slice(&value.to_be_bytes());
                            rewo_net::route_menu(data_id, &d, &session.ids, &mut session.menus);
                        }
                        self.brewing_injected = true;
                    }
                }
            }
        }
        // Three-quarters through, turn on advanced tooltips (F3+H), which
        // adds the item's id as a second line.
        //
        // This is what makes the Velvet fix *gradeable*. The rebuild the
        // screen-open triggers happens on the first frame the pass exists,
        // before it has ever drawn — so destroying its image then is legal
        // and the M3 mutation survived it. New glyphs arriving while the
        // pass is already drawing every frame is the case the `wait_idle`
        // is actually for, and a second tooltip line is the cheapest way to
        // produce it.
        if half && self.screen.inventory_open() && !self.advanced_tooltips {
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            if self.started.elapsed().as_secs_f32() >= limit * 0.75 {
                self.advanced_tooltips = true;
            }
        }
    }
    }
}

#[cfg(test)]
mod witness_seam_tests {
    use super::witness_seam_faults;

    fn names(ids: &[usize]) -> Vec<String> {
        ids.iter().map(|i| format!("r{i} something")).collect()
    }
    fn faults(ids: &[usize], expected: usize) -> Vec<String> {
        let owned = names(ids);
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        witness_seam_faults(&refs, expected)
    }

    /// The healthy case, so every assertion below is a difference rather than a
    /// constant.
    #[test]
    fn a_contiguous_unique_set_of_the_declared_size_is_clean() {
        assert!(faults(&[1, 2, 3, 4], 4).is_empty());
    }

    /// **The M127-M134 failure, and the one fifteen specs were about to
    /// reproduce.** Two rows carrying `r48` merge cleanly and the count still
    /// matches, so the duplicate is the ONLY signal.
    #[test]
    fn two_milestones_minting_the_same_id_is_a_fault() {
        let f = faults(&[1, 2, 3, 3], 4);
        assert!(
            f.iter().any(|s| s.contains("duplicate")),
            "a duplicate id is reported: {f:?}"
        );
    }

    /// …and it is reported EVEN THOUGH the other two checks are silent, which
    /// is what makes the duplicate check load-bearing rather than redundant.
    ///
    /// **This assertion was wrong before the code was.** Its first version
    /// claimed two faults — "the duplicate AND the gap at r4" — and there is no
    /// gap: the count check sees four ids against a declared four, and the
    /// contiguity check runs on the DEDUPLICATED set, which is `[1, 2, 3]` and
    /// perfectly contiguous. That is precisely the point. A merge that mints
    /// `r48` twice leaves a namespace whose size and shape both look right, so
    /// the duplicate is the only signal there is.
    #[test]
    fn a_duplicate_is_the_only_signal_when_the_count_and_shape_still_look_right() {
        let f = faults(&[1, 2, 3, 3], 4);
        assert_eq!(f.len(), 1, "exactly one fault: {f:?}");
        assert!(f[0].contains("duplicate"), "{f:?}");
    }

    /// A row lost to a conflict resolution: unique, contiguous, wrong count.
    #[test]
    fn a_row_silently_dropped_moves_the_count() {
        let f = faults(&[1, 2, 3], 4);
        assert!(f.iter().any(|s| s.contains("witness count 3 != declared 4")), "{f:?}");
    }

    /// A deleted row whose id was never reclaimed — the fault that stops the
    /// NEXT milestone choosing a number that looks free.
    #[test]
    fn an_unreclaimed_id_leaves_a_gap() {
        let f = faults(&[1, 2, 4], 3);
        assert!(f.iter().any(|s| s.contains("first gap at r3")), "{f:?}");
    }

    /// A non-`rNN` row is not a witness id and must not be counted as one —
    /// otherwise the count check fires on rows the namespace does not own.
    #[test]
    fn a_row_without_an_id_is_not_counted() {
        let owned = ["r1 a".to_string(), "not-a-witness".to_string()];
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        assert!(witness_seam_faults(&refs, 1).is_empty());
    }
}
