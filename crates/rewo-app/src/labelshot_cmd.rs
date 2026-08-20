//! `rewo labelshot --check` — the M70 entity-label visibility oracle.
//!
//! Three features share one predicate — the nametag, the health bar, and
//! anything else that floats above an entity — and this grades it. Unlike
//! `healthbarshot`, which has no vanilla behaviour to transcribe and grades
//! against a written spec, **every property here has a decompile citation**:
//! `EntityRenderer.shouldShowName` / `extractNameTags`,
//! `LivingEntityRenderer.shouldShowName`, `MobRenderer` / `AvatarRenderer`'s
//! extra conjunct, `Entity.isDiscrete` / `isVehicle` / `isInvisibleTo`, and
//! `Team.Visibility`.
//!
//! The path under test, end to end:
//!
//! ```text
//! raw set_entity_data / update_attributes / set_passengers / set_player_team
//!   -> rewo_net::route_* / parse_set_passengers / parse_set_player_team
//!   -> EntityTable + Teams
//!   -> live_cmd::label_inputs_from_table + teams::label_team   (the SAME
//!      resolution collect_entities uses)
//!   -> rewo_world::label::should_show_name / should_show_health_bar
//!   -> live_cmd::resolve_labels          (the SAME seam that fills EntityDraw)
//!   -> EntityPass::set_draws             (the real frame path; text-range count)
//! ```
//!
//! **Nothing is measured by diffing two frames** (M50, M37). Every measurement
//! is a decoded boolean or a vertex count.
//!
//! **Every witness names its mutation partner in its detail string**, and each
//! partner was run. The suppression properties are the sharp ones here: an
//! entity that should be hidden emits *zero* label vertices, and its partner —
//! the same entity with the one input flipped — emits some. A witness that
//! only ever measured zero would pass for a renderer that drew nothing at all,
//! which is why every hide is paired with a show.
//!
//! **Fail-closed** on a fixed [`EXPECTED_WITNESSES`] count.

use clap::Args as ClapArgs;

use rewo_data::assets;
use rewo_data::attributes::AttributeRegistry;
use rewo_data::entity_types::{EntityClasses, EntityTypes};
use rewo_data::packets::Packets;
use rewo_data::DataPaths;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;
use rewo_net::ids::Ids;
use rewo_net::teams::{parse_set_player_team, Teams};
use rewo_world::entities::{EntityState, EntityTable};
use rewo_world::label::{
    should_show_health_bar, should_show_name, LabelRenderer, TeamView, DEFAULT_NAME_TAG_DISTANCE,
    DISCRETE_MAX_DISTANCE_SQ,
};

use crate::live_cmd::{label_inputs_from_table, resolve_labels, LabelViewer};

const EXPECTED_WITNESSES: usize = 52;

const W: u32 = 256;
const H: u32 = 256;

#[derive(ClapArgs, Debug)]
pub struct LabelshotArgs {
    #[arg(long, default_value = "26.2")]
    pub version: String,
    /// Assert every property and exit non-zero on any failure.
    #[arg(long)]
    pub check: bool,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[labelshot] {}  {name}: {detail}",
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

pub fn run(args: LabelshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[labelshot] mode: {mode} (serverless; the oracle asserts \
         unconditionally). Transcribed from the 26.2 decompile — \
         EntityRenderer / LivingEntityRenderer / MobRenderer / AvatarRenderer \
         shouldShowName, Entity.isDiscrete/isVehicle/isInvisibleTo, \
         Team.Visibility."
    );

    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_predicate(&mut c, &paths)?;
    check_teams(&mut c, &paths)?;
    check_pick(&mut c, &paths, &jar)?;
    check_wiring(&mut c, &paths, &baked, &jar)?;

    println!(
        "[labelshot] witnesses observed: {} / {}",
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
            "witness count {} != expected {EXPECTED_WITNESSES} — a named property \
             was skipped (fail-closed)",
            c.witnessed
        ));
    }
    println!("[labelshot] PASS — {} witnesses", c.witnessed);
    Ok(())
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

fn attrs_body(entity: i32, snaps: &[(i32, f64)]) -> Vec<u8> {
    let mut out = Vec::new();
    varint(entity, &mut out);
    varint(snaps.len() as i32, &mut out);
    for (attr, base) in snaps {
        varint(*attr, &mut out);
        out.extend_from_slice(&base.to_be_bytes());
        varint(0, &mut out); // no modifiers
    }
    out
}

/// A `SynchedEntityData` delta stream: VarInt entity id (kept < 128 so it is
/// one byte), then `index u8 + serializer VarInt + value`, terminated by 0xFF.
fn meta_body(eid: u8, index: u8, serializer: u8, value: &[u8]) -> Vec<u8> {
    let mut b = vec![eid, index, serializer];
    b.extend_from_slice(value);
    b.push(0xFF);
    b
}

/// `ClientboundSetPassengersPacket`: VarInt vehicle, then a VarInt array.
fn passengers_body(vehicle: i32, riders: &[i32]) -> Vec<u8> {
    let mut out = Vec::new();
    varint(vehicle, &mut out);
    varint(riders.len() as i32, &mut out);
    for r in riders {
        varint(*r, &mut out);
    }
    out
}

/// `ClientboundSetPlayerTeamPacket`, method 0 (create + parameters + roster).
///
/// The three `Component`s are network-NBT string tags, as
/// `ComponentSerialization.TRUSTED_STREAM_CODEC` writes them.
fn team_body(name: &str, visibility: i32, options: u8, players: &[&str]) -> Vec<u8> {
    let mut b = Vec::new();
    varint(name.len() as i32, &mut b);
    b.extend_from_slice(name.as_bytes());
    b.push(0); // method 0 = ADD
    for s in ["Display", "pre", "suf"] {
        b.push(8); // NBT tag id 8 = string
        b.extend_from_slice(&(s.len() as u16).to_be_bytes());
        b.extend_from_slice(s.as_bytes());
    }
    varint(visibility, &mut b);
    varint(0, &mut b); // collision rule ALWAYS
    b.push(0); // no colour
    b.push(options);
    varint(players.len() as i32, &mut b);
    for p in players {
        varint(p.len() as i32, &mut b);
        b.extend_from_slice(p.as_bytes());
    }
    b
}

// ---------------------------------------------------------------------------
// a..d — the predicate, through the real decode and the real resolver.
// ---------------------------------------------------------------------------

/// The one place this file builds inputs. Everything downstream of it is
/// production code.
struct Fixture {
    ids: Ids,
    classes: EntityClasses,
    types: EntityTypes,
    reg: AttributeRegistry,
    zombie: i32,
    boat: i32,
    max_health: i32,
    name_tag_distance: i32,
}

impl Fixture {
    fn load(paths: &DataPaths) -> Result<Fixture, String> {
        let packets = Packets::load(&paths.packets_json())?;
        let ids = Ids::resolve(&packets)?;
        let types = EntityTypes::load(&paths.registries_json())?;
        let classes = EntityClasses::resolve(&types)?;
        let reg = AttributeRegistry::load(&paths.registries_json())?;
        let zombie = types
            .id_of("minecraft:zombie")
            .ok_or("registries.json: no minecraft:zombie")?;
        let boat = types
            .id_of("minecraft:oak_boat")
            .ok_or("registries.json: no minecraft:oak_boat")?;
        let max_health = reg.id_of("max_health").ok_or("no minecraft:max_health")?;
        let name_tag_distance = reg
            .id_of("name_tag_distance")
            .ok_or("no minecraft:name_tag_distance")?;
        Ok(Fixture {
            ids,
            classes,
            types,
            reg,
            zombie,
            boat,
            max_health,
            name_tag_distance,
        })
    }

    fn spawn(&self, type_id: i32) -> EntityTable {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, type_id, 0.0, 0.0, 0.0, 0.0, 0.0));
        t
    }

    fn send_meta(&self, t: &mut EntityTable, body: &[u8]) {
        self.send_meta_lang(t, body, None);
    }

    /// [`Self::send_meta`] with a language table, for the M163 nametag
    /// witnesses. Same production router; the table is the only difference, so
    /// the pair `(Some(lang), None)` is the mutation partner built in.
    fn send_meta_lang(
        &self,
        t: &mut EntityTable,
        body: &[u8],
        lang: Option<&rewo_data::lang::Language>,
    ) {
        rewo_net::route_set_entity_data(
            self.ids.cb_play_set_entity_data,
            body,
            &self.ids,
            t,
            rewo_net::MetaKinds {
                classes: Some(&self.classes),
                lang,
                ..Default::default()
            },
        );
    }

    fn send_attrs(&self, t: &mut EntityTable, body: &[u8]) {
        rewo_net::route_update_attributes(
            self.ids.cb_play_update_attributes,
            body,
            &self.ids,
            t,
            Some(&self.classes),
            Some(&self.types),
            Some(&self.reg),
        );
    }

    /// A zombie that has synced a max health, so a bar is possible, plus a
    /// visible custom name so a tag is possible. Everything else is the
    /// default: not sneaking, not invisible, not ridden, no team.
    fn healthy_named_zombie(&self) -> EntityTable {
        let mut t = self.spawn(self.zombie);
        self.send_attrs(&mut t, &attrs_body(1, &[(self.max_health, 20.0)]));
        self.send_meta(&mut t, &meta_body(1, 9, 3, &7.0f32.to_be_bytes()));
        self.send_meta(&mut t, &meta_body(1, 3, 8, &[0x01])); // custom name visible
        t.set_custom_name(1, Some("Bob".into()));
        t
    }

    /// `(nametag shown, health bar shown)` for entity 1 — through the
    /// production input resolution and the production predicate.
    fn labels(
        &self,
        t: &EntityTable,
        type_name: &str,
        distance_sq: f64,
        viewer: &LabelViewer<'_>,
        team: Option<TeamView<'_>>,
        is_player: bool,
    ) -> (bool, bool) {
        let label = label_inputs_from_table(
            t,
            1,
            Some(type_name),
            Some(&self.reg),
            is_player,
            distance_sq,
            viewer,
            team,
        );
        (should_show_name(&label), should_show_health_bar(&label))
    }

    /// The common case: a mob at point-blank range seen by a plain viewer.
    fn zombie_labels(&self, t: &EntityTable) -> (bool, bool) {
        self.labels(t, "minecraft:zombie", 4.0, &LabelViewer::default(), None, false)
    }

    /// [`Self::healthy_named_zombie`] with `riders` aboard, through the real
    /// `set_passengers` route. Built fresh each time rather than cloned —
    /// `EntityTable` is deliberately not `Clone`, and rebuilding also means
    /// every scenario below starts from the same packets.
    fn ridden_zombie(&self, riders: &[i32]) -> EntityTable {
        let mut t = self.healthy_named_zombie();
        for r in riders {
            t.add(*r, EntityState::new(0, self.zombie, 0.0, 0.0, 0.0, 0.0, 0.0));
        }
        rewo_net::route_set_passengers(
            self.ids.cb_play_set_passengers,
            &passengers_body(1, riders),
            &self.ids,
            &mut t,
        );
        t
    }

    /// A named, name-visible boat, optionally invisible.
    fn named_boat(&self, invisible: bool) -> EntityTable {
        let mut t = self.spawn(self.boat);
        self.send_attrs(&mut t, &attrs_body(1, &[(self.max_health, 20.0)]));
        self.send_meta(&mut t, &meta_body(1, 3, 8, &[0x01]));
        if invisible {
            self.send_meta(&mut t, &meta_body(1, 0, 0, &[0x20]));
        }
        t.set_custom_name(1, Some("Boaty".into()));
        t
    }
}

fn check_predicate(c: &mut Checker, paths: &DataPaths) -> Result<(), String> {
    let f = Fixture::load(paths)?;
    println!(
        "[labelshot] ids: set_entity_data={} update_attributes={} \
         set_passengers={} set_player_team={}; zombie={} boat={}",
        f.ids.cb_play_set_entity_data,
        f.ids.cb_play_update_attributes,
        f.ids.cb_play_set_passengers,
        f.ids.cb_play_set_player_team,
        f.zombie,
        f.boat
    );

    // -- a: the renderer ladder and the name source --------------------------

    let base = f.healthy_named_zombie();
    let (n, hb) = f.zombie_labels(&base);
    c.record(
        "a1.the_baseline_shows_both_labels",
        n && hb,
        format!(
            "name={n} bar={hb} (want both — every other witness in this file \
             turns exactly one input off from here, so a baseline that showed \
             nothing would make all of them vacuous)"
        ),
    );

    // `MobRenderer`'s extra conjunct: no name source, no nametag — but the bar
    // does not take that conjunct, because a mob with no name still has health.
    let mut unnamed = f.spawn(f.zombie);
    f.send_attrs(&mut unnamed, &attrs_body(1, &[(f.max_health, 20.0)]));
    f.send_meta(&mut unnamed, &meta_body(1, 9, 3, &7.0f32.to_be_bytes()));
    let (n_un, hb_un) = f.zombie_labels(&unnamed);
    c.record(
        "a2.an_un_named_mob_is_silent_but_keeps_its_bar",
        !n_un && hb_un,
        format!(
            "name={n_un} bar={hb_un} (want false/true — MobRenderer.shouldShowName \
             ANDs `entity.shouldShowName() || (hasCustomName && crosshairPick)` \
             onto LivingEntityRenderer's, and the bar deliberately does not take \
             that conjunct. MUTATION PARTNER a1, which differs only in having \
             sent index-3 CUSTOM_NAME_VISIBLE. Reading LivingEntityRenderer \
             alone would name every mob in the world)"
        ),
    );

    // A named mob whose visibility flag is off needs the crosshair. Rewo has no
    // entity pick, so the live client feeds `false`; both branches are driven
    // here so the transcription is graded even though one is unreachable.
    let mut named_hidden = f.spawn(f.zombie);
    f.send_attrs(&mut named_hidden, &attrs_body(1, &[(f.max_health, 20.0)]));
    named_hidden.set_custom_name(1, Some("Bob".into()));
    let mut label = label_inputs_from_table(
        &named_hidden,
        1,
        Some("minecraft:zombie"),
        Some(&f.reg),
        false,
        4.0,
        &LabelViewer::default(),
        None,
    );
    let without_pick = should_show_name(&label);
    label.is_crosshair_pick = true;
    let with_pick = should_show_name(&label);
    c.record(
        "a3.a_named_but_not_visible_mob_needs_the_crosshair",
        !without_pick && with_pick,
        format!(
            "crosshairPick false -> {without_pick}, true -> {with_pick} (want \
             false/true — the second half of the name-source clause. The two \
             runs are each other's MUTATION PARTNER; Rewo feeds false live \
             because it has no entity raycast, so this is the branch that \
             proves the clause is transcribed rather than dropped)"
        ),
    );

    // `Player.shouldShowName()` returns a literal `true`, so a player needs no
    // custom name at all.
    let mut player_t = f.spawn(f.zombie);
    f.send_attrs(&mut player_t, &attrs_body(1, &[(f.max_health, 20.0)]));
    let as_player = f.labels(
        &player_t,
        "minecraft:zombie",
        4.0,
        &LabelViewer::default(),
        None,
        true,
    );
    c.record(
        "a4.a_player_is_named_without_a_custom_name",
        as_player.0,
        format!(
            "name={} (want true — Player.shouldShowName() is a literal `true`. \
             MUTATION PARTNER a2, the identical table resolved as a Mob rather \
             than an Avatar, which is silent)",
            as_player.0
        ),
    );

    // A non-living entity takes the base `EntityRenderer` rule, and never a bar.
    let boat_t = f.named_boat(false);
    let boat_label = label_inputs_from_table(
        &boat_t,
        1,
        Some("minecraft:oak_boat"),
        Some(&f.reg),
        false,
        4.0,
        &LabelViewer::default(),
        None,
    );
    c.record(
        "a5.a_non_living_entity_is_named_by_the_base_rule_and_has_no_bar",
        boat_label.renderer == LabelRenderer::Other
            && should_show_name(&boat_label)
            && !should_show_health_bar(&boat_label),
        format!(
            "renderer={:?} name={} bar={} (want Other/true/false — \
             DefaultAttributes.SUPPLIERS is keyed by EntityType<? extends \
             LivingEntity>, so having no supplier is having no LivingEntity \
             renderer, even though an update_attributes was accepted for it. \
             MUTATION PARTNER a1, the same packets on a zombie)",
            boat_label.renderer,
            should_show_name(&boat_label),
            should_show_health_bar(&boat_label)
        ),
    );

    // Non-living entities also skip every suppression rule for their name.
    let boat_hidden = f.named_boat(true);
    let boat_hidden_label = label_inputs_from_table(
        &boat_hidden,
        1,
        Some("minecraft:oak_boat"),
        Some(&f.reg),
        false,
        4.0,
        &LabelViewer {
            hud_hidden: true,
            ..Default::default()
        },
        None,
    );
    c.record(
        "a6.the_base_rule_ignores_invisibility_and_f1",
        should_show_name(&boat_hidden_label),
        format!(
            "name={} (want true — EntityRenderer.shouldShowName reads only the \
             name source; the invisibility and hud.isHidden() clauses live in \
             LivingEntityRenderer's override, which a boat never reaches. \
             MUTATION PARTNER c1/d4, the same two inputs on a zombie, which do \
             suppress)",
            should_show_name(&boat_hidden_label)
        ),
    );

    // -- b: the sneak cut-off ------------------------------------------------

    let mut sneaking = f.healthy_named_zombie();
    // Shared flags bit 1 = FLAG_SHIFT_KEY_DOWN, which is `isDiscrete()`.
    f.send_meta(&mut sneaking, &meta_body(1, 0, 0, &[0x02]));
    let just_inside = f.labels(
        &sneaking,
        "minecraft:zombie",
        DISCRETE_MAX_DISTANCE_SQ - 0.01,
        &LabelViewer::default(),
        None,
        false,
    );
    let at_bound = f.labels(
        &sneaking,
        "minecraft:zombie",
        DISCRETE_MAX_DISTANCE_SQ,
        &LabelViewer::default(),
        None,
        false,
    );
    c.record(
        "b1.the_sneak_cut_off_is_distance_sq_ge_1024_exactly",
        just_inside.0 && !at_bound.0,
        format!(
            "distSq {}: name={}; distSq {DISCRETE_MAX_DISTANCE_SQ}: name={} \
             (want true/false — the source reads `if (distanceToCameraSq >= \
             1024.0) return false`, so the bound itself is excluded. The 32.0F \
             beside it is a dead local; the surviving constant is the folded \
             1024.0. These two runs are each other's MUTATION PARTNER — a `>` \
             would show at the bound)",
            DISCRETE_MAX_DISTANCE_SQ - 0.01,
            just_inside.0,
            at_bound.0
        ),
    );

    let standing = f.healthy_named_zombie();
    let standing_far = f.labels(
        &standing,
        "minecraft:zombie",
        DISCRETE_MAX_DISTANCE_SQ,
        &LabelViewer::default(),
        None,
        false,
    );
    c.record(
        "b2.not_sneaking_at_the_same_distance_still_shows",
        standing_far.0 && standing_far.1,
        format!(
            "name={} bar={} at distSq {DISCRETE_MAX_DISTANCE_SQ} (want both — \
             MUTATION PARTNER b1's second run, the identical distance with \
             shared-flags bit 1 set. Without this, b1 could be passing because \
             of the distance alone)",
            standing_far.0, standing_far.1
        ),
    );

    // The distinguishing property: NAME_TAG_DISTANCE moves the outer gate and
    // leaves the sneak bound alone, because 1024.0 is not Mth.square(ntd).
    let mut far_sneak = f.healthy_named_zombie();
    f.send_meta(&mut far_sneak, &meta_body(1, 0, 0, &[0x02]));
    f.send_attrs(
        &mut far_sneak,
        &attrs_body(1, &[(f.name_tag_distance, 128.0)]),
    );
    let mut far_stand = f.healthy_named_zombie();
    f.send_attrs(
        &mut far_stand,
        &attrs_body(1, &[(f.name_tag_distance, 128.0)]),
    );
    let d40 = 40.0f64 * 40.0;
    let sneak_at_40 = f.labels(&far_sneak, "minecraft:zombie", d40, &LabelViewer::default(), None, false);
    let stand_at_40 = f.labels(&far_stand, "minecraft:zombie", d40, &LabelViewer::default(), None, false);
    c.record(
        "b3.the_sneak_bound_does_not_scale_with_name_tag_distance",
        !sneak_at_40.0 && stand_at_40.0,
        format!(
            "at 40 blocks with NAME_TAG_DISTANCE=128: sneaking name={}, \
             standing name={} (want false/true — the sneak bound is a \
             hard-coded 1024.0, NOT Mth.square(nameTagDistance), so raising the \
             attribute reaches further while a sneaking entity stays capped at \
             32. The two runs are each other's MUTATION PARTNER; a bound \
             written as square(ntd) would show both)",
            sneak_at_40.0, stand_at_40.0
        ),
    );

    // The outer gate itself, and that it reads the attribute rather than 64.
    let plain = f.healthy_named_zombie();
    // Straddle the default from the constant rather than a literal, so the
    // witness moves with `Entity.DEFAULT_NAME_TAG_DISTANCE` if it ever does.
    let d = DEFAULT_NAME_TAG_DISTANCE;
    let inside_64 = f.labels(&plain, "minecraft:zombie", (d - 0.01) * (d - 0.01), &LabelViewer::default(), None, false);
    let outside_64 = f.labels(&plain, "minecraft:zombie", (d + 0.01) * (d + 0.01), &LabelViewer::default(), None, false);
    // **Exactly on the bound.** Straddling it either side is not enough: the
    // first cut of this witness used only 63.99/64.01 and a `<=` mutation kept
    // the whole gate green, because nothing ever sat at the bound itself. The
    // source is `<`, so `distanceToCameraSq == Mth.square(ntd)` is out.
    let at_64 = f.labels(&plain, "minecraft:zombie", d * d, &LabelViewer::default(), None, false);
    let mut long_range = f.healthy_named_zombie();
    f.send_attrs(&mut long_range, &attrs_body(1, &[(f.name_tag_distance, 128.0)]));
    let at_100 = f.labels(&long_range, "minecraft:zombie", 100.0 * 100.0, &LabelViewer::default(), None, false);
    let plain_at_100 = f.labels(&plain, "minecraft:zombie", 100.0 * 100.0, &LabelViewer::default(), None, false);
    c.record(
        "b4.the_outer_gate_is_the_name_tag_distance_attribute",
        inside_64.0 && !outside_64.0 && !at_64.0 && at_100.0 && !plain_at_100.0,
        format!(
            "default: {d}-0.01 -> {}, exactly {d} -> {}, {d}+0.01 -> {}; \
             NAME_TAG_DISTANCE=128: 100 -> {}, default at 100 -> {} (want \
             true/false/false/true/false — extractNameTags tests \
             `distanceToCameraSq < Mth.square(ntd)` and LivingEntityRenderer \
             supplies the attribute. TWO MUTATION PARTNERS, because one is not \
             enough: `<=` is caught only by the exact-bound sample, and a \
             hard-coded 64.0 only by the last pair. The first cut of this \
             witness had no exact-bound sample and a `<=` mutation left the \
             whole gate green)",
            inside_64.0, at_64.0, outside_64.0, at_100.0, plain_at_100.0
        ),
    );

    // -- c: invisibility and the spectator ------------------------------------

    let mut invisible = f.healthy_named_zombie();
    f.send_meta(&mut invisible, &meta_body(1, 0, 0, &[0x20])); // bit 5
    let inv = f.zombie_labels(&invisible);
    c.record(
        "c1.an_invisible_entity_shows_neither_label",
        !inv.0 && !inv.1,
        format!(
            "name={} bar={} (want both false — Entity.isInvisible() is shared \
             flag 5. MUTATION PARTNER a1, the same entity without the flag)",
            inv.0, inv.1
        ),
    );

    let spectating = f.labels(
        &invisible,
        "minecraft:zombie",
        4.0,
        &LabelViewer {
            spectator: true,
            ..Default::default()
        },
        None,
        false,
    );
    c.record(
        "c2.a_spectating_viewer_sees_invisible_entities",
        spectating.0 && spectating.1,
        format!(
            "name={} bar={} (want both true — `isInvisibleTo` opens with `if \
             (player.isSpectator()) return false`, i.e. NOT invisible, so a \
             spectator sees through invisibility. This is the rule that reads \
             backwards, and REWO_HEALTH_BAR_SPEC.md rule 5 records it: there is \
             no 'spectators are hidden' clause anywhere. MUTATION PARTNER c1, \
             the identical table with a non-spectating viewer)",
            spectating.0, spectating.1
        ),
    );

    // -- d: vehicle, camera entity, F1 ---------------------------------------

    let ridden = f.ridden_zombie(&[2]);
    let while_ridden = f.zombie_labels(&ridden);
    // The same table, then told everyone got off — the only packet that brings
    // the label back.
    let mut dismounted = f.ridden_zombie(&[2]);
    rewo_net::route_set_passengers(
        f.ids.cb_play_set_passengers,
        &passengers_body(1, &[]),
        &f.ids,
        &mut dismounted,
    );
    let after_dismount = f.zombie_labels(&dismounted);
    c.record(
        "d1.a_ridden_entity_hides_both_labels",
        !while_ridden.0 && !while_ridden.1 && after_dismount.0 && after_dismount.1,
        format!(
            "ridden: name={} bar={}; after an empty roster: name={} bar={} \
             (want false/false then true/true — Entity.isVehicle() is \
             `!passengers.isEmpty()`. The empty roster is the MUTATION PARTNER \
             and is also the only thing that brings the label back, which is \
             why decoding it as a truncation would be silent)",
            while_ridden.0, while_ridden.1, after_dismount.0, after_dismount.1
        ),
    );

    // `isVehicle` is not `isPassenger`: the rider keeps its own label.
    let rider_label = label_inputs_from_table(
        &ridden,
        2,
        Some("minecraft:zombie"),
        Some(&f.reg),
        false,
        4.0,
        &LabelViewer::default(),
        None,
    );
    c.record(
        "d2.the_rider_is_not_itself_a_vehicle",
        !rider_label.is_vehicle,
        format!(
            "rider is_vehicle={} (want false — `isVehicle()` asks whether \
             something rides THIS entity, not whether it rides something. \
             MUTATION PARTNER d1, the vehicle in the same table, which is true. \
             Inverting the two would hide every rider and show every mount)",
            rider_label.is_vehicle
        ),
    );

    let as_camera = f.labels(
        &base,
        "minecraft:zombie",
        4.0,
        &LabelViewer {
            camera_entity: Some(1),
            local_player: Some(1),
            ..Default::default()
        },
        None,
        false,
    );
    c.record(
        "d3.the_camera_entity_shows_no_label",
        !as_camera.0 && !as_camera.1,
        format!(
            "name={} bar={} (want both false — `entity != minecraft \
             .getCameraEntity()`. MUTATION PARTNER a1, the same entity with the \
             camera elsewhere)",
            as_camera.0, as_camera.1
        ),
    );

    let hidden_hud = f.labels(
        &base,
        "minecraft:zombie",
        4.0,
        &LabelViewer {
            hud_hidden: true,
            ..Default::default()
        },
        None,
        false,
    );
    c.record(
        "d4.f1_hides_both_labels",
        !hidden_hud.0 && !hidden_hud.1,
        format!(
            "name={} bar={} (want both false — `!Minecraft.getInstance().gui \
             .hud.isHidden()` is the first conjunct of the no-team tail. \
             MUTATION PARTNER a1, the identical call with hud_hidden false)",
            hidden_hud.0, hidden_hud.1
        ),
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// e — teams, through real set_player_team packets.
// ---------------------------------------------------------------------------

/// Visibility ids, in `Team.Visibility` declaration order.
const V_ALWAYS: i32 = 0;
const V_NEVER: i32 = 1;
const V_HIDE_OTHER: i32 = 2;
const V_HIDE_OWN: i32 = 3;
/// `PlayerTeam.packOptions` bit 1 — `seeFriendlyInvisibles`.
const OPT_SEE_INVIS: u8 = 2;

fn check_teams(c: &mut Checker, paths: &DataPaths) -> Result<(), String> {
    let f = Fixture::load(paths)?;
    let base = f.healthy_named_zombie();

    // The entity's scoreboard name. `Entity.getScoreboardName()` is
    // `this.stringUUID` for everything but a player, and `EntityState::new`
    // above was built with uuid 0.
    let mob_member = rewo_net::play::uuid_to_dashed(0);

    // Build a `Teams` the way the client does: parse, then apply.
    let teams_with = |visibility: i32, options: u8, members: &[&str]| -> Teams {
        let mut teams = Teams::new();
        let body = team_body("red", visibility, options, members);
        let p = parse_set_player_team(&body).expect("team body");
        assert!(teams.apply(&p), "the create packet must be accepted");
        teams
    };

    // e1 — ALWAYS.
    let t_always = teams_with(V_ALWAYS, 0, &[&mob_member]);
    let always = f.labels(&base, "minecraft:zombie", 4.0, &LabelViewer::default(), rewo_net::teams::label_team(&t_always, &mob_member), false);
    c.record(
        "e1.visibility_always_shows_a_visible_entity",
        always.0 && always.1,
        format!(
            "name={} bar={} (want both — `case ALWAYS -> isVisibleToPlayer`. \
             MUTATION PARTNER e2, the same packet with visibility 1)",
            always.0, always.1
        ),
    );

    // e2 — NEVER.
    let t_never = teams_with(V_NEVER, 0, &[&mob_member]);
    let never = f.labels(&base, "minecraft:zombie", 4.0, &LabelViewer::default(), rewo_net::teams::label_team(&t_never, &mob_member), false);
    c.record(
        "e2.visibility_never_hides_both_labels",
        !never.0 && !never.1,
        format!(
            "name={} bar={} (want both false — `case NEVER -> false`, the only \
             arm that ignores isVisibleToPlayer entirely. MUTATION PARTNER e1)",
            never.0, never.1
        ),
    );

    // e3 — HIDE_FOR_OTHER_TEAMS, all three viewer states.
    let t_other = teams_with(V_HIDE_OTHER, 0, &[&mob_member]);
    let no_team = f.labels(&base, "minecraft:zombie", 4.0, &LabelViewer::default(), rewo_net::teams::label_team(&t_other, &mob_member), false);
    let other_team = f.labels(
        &base,
        "minecraft:zombie",
        4.0,
        &LabelViewer {
            team: Some("blue"),
            ..Default::default()
        },
        rewo_net::teams::label_team(&t_other, &mob_member),
        false,
    );
    let same_team = f.labels(
        &base,
        "minecraft:zombie",
        4.0,
        &LabelViewer {
            team: Some("red"),
            ..Default::default()
        },
        rewo_net::teams::label_team(&t_other, &mob_member),
        false,
    );
    c.record(
        "e3.hide_for_other_teams_needs_the_viewer_on_the_same_team",
        no_team.0 && !other_team.0 && same_team.0,
        format!(
            "teamless viewer -> {}, other team -> {}, same team -> {} (want \
             true/false/true — `myTeam == null ? isVisibleToPlayer : \
             team.isAlliedTo(myTeam) && ...`, and Team.isAlliedTo is `this == \
             other`, i.e. identity, which is name equality here. The three runs \
             are each other's MUTATION PARTNERS; note the teamless arm is the \
             pass-through, not the hidden one)",
            no_team.0, other_team.0, same_team.0
        ),
    );

    // e4 — HIDE_FOR_OWN_TEAM: the exact inverse on the two teamed arms.
    let t_own = teams_with(V_HIDE_OWN, 0, &[&mob_member]);
    let own_no_team = f.labels(&base, "minecraft:zombie", 4.0, &LabelViewer::default(), rewo_net::teams::label_team(&t_own, &mob_member), false);
    let own_other = f.labels(
        &base,
        "minecraft:zombie",
        4.0,
        &LabelViewer {
            team: Some("blue"),
            ..Default::default()
        },
        rewo_net::teams::label_team(&t_own, &mob_member),
        false,
    );
    let own_same = f.labels(
        &base,
        "minecraft:zombie",
        4.0,
        &LabelViewer {
            team: Some("red"),
            ..Default::default()
        },
        rewo_net::teams::label_team(&t_own, &mob_member),
        false,
    );
    c.record(
        "e4.hide_for_own_team_is_the_inverse_on_the_teamed_arms_only",
        own_no_team.0 && own_other.0 && !own_same.0,
        format!(
            "teamless -> {}, other team -> {}, same team -> {} (want \
             true/true/false — `!team.isAlliedTo(myTeam) && isVisibleToPlayer`. \
             MUTATION PARTNER e3, which differs only in the visibility id: the \
             teamless arm agrees between the two and the teamed arms invert, so \
             a gate that only tested the teamless viewer could not tell the two \
             visibilities apart at all)",
            own_no_team.0, own_other.0, own_same.0
        ),
    );

    // e5 — canSeeFriendlyInvisibles, and its asymmetry.
    let mut invisible = f.healthy_named_zombie();
    f.send_meta(&mut invisible, &meta_body(1, 0, 0, &[0x20]));
    let red = LabelViewer {
        team: Some("red"),
        ..Default::default()
    };
    let t_other_csfi = teams_with(V_HIDE_OTHER, OPT_SEE_INVIS, &[&mob_member]);
    let t_other_plain = teams_with(V_HIDE_OTHER, 0, &[&mob_member]);
    let with_csfi = f.labels(&invisible, "minecraft:zombie", 4.0, &red, rewo_net::teams::label_team(&t_other_csfi, &mob_member), false);
    let without_csfi = f.labels(&invisible, "minecraft:zombie", 4.0, &red, rewo_net::teams::label_team(&t_other_plain, &mob_member), false);
    c.record(
        "e5.can_see_friendly_invisibles_carries_the_hide_for_other_teams_arm",
        with_csfi.0 && !without_csfi.0,
        format!(
            "invisible team-mate: with the option -> {}, without -> {} (want \
             true/false — `team.isAlliedTo(myTeam) && \
             (team.canSeeFriendlyInvisibles() || isVisibleToPlayer)`. The two \
             runs are each other's MUTATION PARTNER, and they differ by exactly \
             one bit of the packed options byte)",
            with_csfi.0, without_csfi.0
        ),
    );

    let t_own_csfi = teams_with(V_HIDE_OWN, OPT_SEE_INVIS, &[&mob_member]);
    let own_csfi = f.labels(&invisible, "minecraft:zombie", 4.0, &red, rewo_net::teams::label_team(&t_own_csfi, &mob_member), false);
    // **The discriminating viewer is on the OTHER team.** With the viewer on
    // `red` the arm reads `team.name != mine && ...`, which is already false,
    // so bolting a `canSeeFriendlyInvisibles ||` escape onto it short-circuits
    // away and the mutation is invisible. From `blue` the identity test passes
    // and the escape — if it existed — would show an invisible entity that
    // vanilla hides. The first cut of this witness tested only the `red`
    // viewer, and the mutation left the whole gate green.
    let blue = LabelViewer {
        team: Some("blue"),
        ..Default::default()
    };
    let own_csfi_other = f.labels(&invisible, "minecraft:zombie", 4.0, &blue, rewo_net::teams::label_team(&t_own_csfi, &mob_member), false);
    let t_always_csfi = teams_with(V_ALWAYS, OPT_SEE_INVIS, &[&mob_member]);
    let always_csfi = f.labels(&invisible, "minecraft:zombie", 4.0, &red, rewo_net::teams::label_team(&t_always_csfi, &mob_member), false);
    c.record(
        "e6.the_two_other_arms_read_the_option_only_through_is_invisible_to",
        !own_csfi.0 && !own_csfi_other.0 && always_csfi.0,
        format!(
            "HIDE_FOR_OWN_TEAM+option, viewer on red -> {}, viewer on blue -> \
             {}; ALWAYS+option -> {} (want false/false/true, and the asymmetry \
             is the point. HIDE_FOR_OWN_TEAM has no `canSeeFriendlyInvisibles \
             ||` escape of its own, so an invisible entity stays hidden from \
             BOTH teams — while ALWAYS shows the invisible team-mate purely \
             because isInvisibleTo already returned false for it. Two different \
             paths read the same bit. MUTATION PARTNER e5, the third arm, where \
             the escape does exist; the blue-viewer sample is what makes adding \
             the escape here observable at all)",
            own_csfi.0, own_csfi_other.0, always_csfi.0
        ),
    );

    // e7 — the early return: a team short-circuits the whole no-team tail.
    let t_short = teams_with(V_ALWAYS, 0, &[&mob_member]);
    let ridden = f.ridden_zombie(&[2]);
    let hidden_viewer = LabelViewer {
        hud_hidden: true,
        ..Default::default()
    };
    let teamed = f.labels(&ridden, "minecraft:zombie", 4.0, &hidden_viewer, rewo_net::teams::label_team(&t_short, &mob_member), false);
    let unteamed = f.labels(&ridden, "minecraft:zombie", 4.0, &hidden_viewer, None, false);
    c.record(
        "e7.a_team_short_circuits_f1_and_is_vehicle",
        teamed.0 && !unteamed.0,
        format!(
            "ridden + F1: teamed -> {}, un-teamed -> {} (want true/false — the \
             team switch RETURNS, so `hud.isHidden()`, `getCameraEntity()` and \
             `isVehicle()` are only ever reached by an entity with no team. A \
             teamed player keeps their nametag with F1 pressed and a teamed \
             horse keeps its while ridden. The two runs are each other's \
             MUTATION PARTNER, and they differ only in whether a team was \
             passed)",
            teamed.0, unteamed.0
        ),
    );

    // e8 — the scoreboard key: a mob is filed under its dashed UUID.
    let by_uuid = rewo_net::teams::label_team(&t_always, &mob_member).is_some();
    let by_wrong_key = rewo_net::teams::label_team(&t_always, "Bob").is_some();
    c.record(
        "e8.a_mobs_membership_is_keyed_by_its_dashed_uuid",
        by_uuid && !by_wrong_key,
        format!(
            "by dashed uuid -> {by_uuid}, by custom name -> {by_wrong_key} \
             (want true/false — Entity.getScoreboardName() is `this.stringUUID` \
             for everything but a player, which overrides it to the profile \
             name. MUTATION PARTNER: the second lookup. Using one key for the \
             other is silent — the team would simply never match)"
        ),
    );

    // e9 — the visibility byte is what selects the arm, through the real parse.
    let parsed_never = parse_set_player_team(&team_body("red", V_NEVER, 0, &[]))
        .map_err(|e| format!("team parse: {e}"))?;
    let parsed_own = parse_set_player_team(&team_body("red", V_HIDE_OWN, 0, &[]))
        .map_err(|e| format!("team parse: {e}"))?;
    let vis_never = parsed_never.parameters.as_ref().map(|p| p.name_tag_visibility);
    let vis_own = parsed_own.parameters.as_ref().map(|p| p.name_tag_visibility);
    c.record(
        "e9.the_parameters_block_carries_the_visibility_id",
        vis_never == Some(rewo_net::teams::Visibility::Never)
            && vis_own == Some(rewo_net::teams::Visibility::HideForOwnTeam),
        format!(
            "{vis_never:?} / {vis_own:?} (want Never / HideForOwnTeam — the \
             method byte is the packet's discriminator, and methods 0 and 2 are \
             the ones that carry parameters. MUTATION PARTNER: a method-1 body, \
             which carries none at all, asserted next)"
        ),
    );

    let mut remove_body = Vec::new();
    varint(3, &mut remove_body);
    remove_body.extend_from_slice(b"red");
    remove_body.push(1); // METHOD_REMOVE
    let parsed_remove =
        parse_set_player_team(&remove_body).map_err(|e| format!("team parse: {e}"))?;
    c.record(
        "e10.a_remove_packet_carries_no_parameters_and_no_roster",
        parsed_remove.parameters.is_none() && parsed_remove.players.is_empty(),
        format!(
            "parameters={:?} players={:?} (want None and empty — \
             shouldHaveParameters is {{0,2}} and shouldHavePlayerList is \
             {{0,3,4}}, so method 1 is a name and a byte. MUTATION PARTNER e9, \
             a method-0 body of the same team, which carries both. Reading the \
             sections unconditionally would desynchronise the reader)",
            parsed_remove.parameters, parsed_remove.players
        ),
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// g — the crosshair entity pick (M73), through the production seam.
// ---------------------------------------------------------------------------

/// The pick fixture: the real version tables plus a scratch entity table.
///
/// Everything downstream of [`Self::pick`] is production code —
/// `live_cmd::crosshair_pick_from_table` is the same function
/// `resolve_crosshair_pick` calls on every frame.
struct PickFixture {
    f: Fixture,
    shapes: rewo_data::entity_pick::EntityPickTable,
    redirectable: rewo_data::entity_pick::EntityTypeTag,
    /// The camera entity's id. Never in the table — the server sends no
    /// `add_entity` for your own player, which is the whole reason
    /// `local_attributes` exists.
    camera: i32,
    local: rewo_world::attributes::EntityAttributes,
}

/// The eye, and a unit view vector pointing east (+X). Every distance below is
/// therefore just a difference in `x`, which is what makes an exact-bound
/// sample writable by hand.
const EYE: [f64; 3] = [0.0, 0.0, 0.0];
const EAST: [f64; 3] = [1.0, 0.0, 0.0];

impl PickFixture {
    fn load(paths: &DataPaths, jar: &std::path::Path) -> Result<PickFixture, String> {
        let f = Fixture::load(paths)?;
        let shapes = rewo_data::entity_pick::EntityPickTable::resolve(&f.types)?;
        let redirectable =
            rewo_data::entity_pick::EntityTypeTag::load_redirectable_projectile(jar, &f.types)?;
        Ok(PickFixture {
            f,
            shapes,
            redirectable,
            camera: 99,
            local: rewo_world::attributes::EntityAttributes::default(),
        })
    }

    fn tables(&self) -> crate::live_cmd::PickTables<'_> {
        crate::live_cmd::PickTables {
            types: &self.f.types,
            classes: &self.f.classes,
            shapes: &self.shapes,
            redirectable: &self.redirectable,
            attributes: &self.f.reg,
        }
    }

    /// Spawn one entity of `type_name` standing with its **feet** at
    /// `(x, foot_y, 0)`, so a box of height `h` spans `foot_y .. foot_y + h`.
    fn table_with(&self, type_name: &str, id: i32, x: f64, foot_y: f64) -> EntityTable {
        let mut t = EntityTable::default();
        let type_id = self
            .f
            .types
            .id_of(type_name)
            .unwrap_or_else(|| panic!("{type_name} is not registered"));
        t.add(id, EntityState::new(0, type_id, x, foot_y, 0.0, 0.0, 0.0));
        t
    }

    /// The production pick. `block` is the distance to the block hit, or
    /// `None` for a block miss.
    fn pick(&self, t: &EntityTable, block: Option<f64>) -> Option<i32> {
        crate::live_cmd::crosshair_pick_from_table(
            t,
            self.camera,
            [0.0, -1.62, 0.0], // feet, so the eye lands at the origin
            &self.local,
            self.tables(),
            EYE,
            EAST,
            1.0,
            &|_, _, reach| block.filter(|d| *d <= reach),
        )
        .map(|h| h.id)
    }
}

fn check_pick(c: &mut Checker, paths: &DataPaths, jar: &std::path::Path) -> Result<(), String> {
    use rewo_data::entity_pick::PickRule;
    use rewo_world::entity_pick::{entity_hit_result, Aabb, Candidate};

    let mut p = PickFixture::load(paths, jar)?;
    println!(
        "[labelshot] pick tables: {} shapes, redirectable_projectile = {} types",
        p.shapes.len(),
        p.redirectable.len()
    );

    // -- g1: the basic sweep, both directions ---------------------------------

    let ahead = p.table_with("minecraft:zombie", 1, 2.0, -1.0);
    let behind = p.table_with("minecraft:zombie", 1, -2.0, -1.0);
    let (hit_ahead, hit_behind) = (p.pick(&ahead, None), p.pick(&behind, None));
    c.record(
        "g1.a_mob_ahead_is_picked_and_one_behind_is_not",
        hit_ahead == Some(1) && hit_behind.is_none(),
        format!(
            "ahead -> {hit_ahead:?}, behind -> {hit_behind:?} (want Some(1)/None \
             — the baseline every witness below turns one thing off from. \
             MUTATION PARTNER: the two runs are each other's, differing only in \
             the sign of x)"
        ),
    );

    // -- g2: the entity range, sampled EXACTLY on the bound -------------------
    //
    // A zombie's box is `sized(0.6, 1.95)`, so its near face is `x - w/2`. The
    // half-width is **not** 0.3: vanilla halves the width as a `float` and
    // widens it only inside the `AABB` constructor, so it is 0.30000001192…
    // Placing the mob at `3.0 + that` is what puts the clip point on the bound
    // exactly; a hand-written 3.3 lands a hundred-millionth *inside* it, and
    // the `<` -> `<=` mutation below then passes.
    //
    // M70's own b4 straddled a bound at 63.99/64.01 without ever sitting on
    // it, and that mutation left the whole gate green. This sits on it, and
    // the placement is asserted rather than assumed.
    let half_width = (0.6f32 / 2.0) as f64;
    let on_bound = p.table_with("minecraft:zombie", 1, 3.0 + half_width, -1.0);
    let near_face = rewo_world::entity_pick::bounding_box(
        [3.0 + half_width, -1.0, 0.0],
        &rewo_world::entity_pick::DimensionInputs {
            width: 0.6,
            height: 1.95,
            living: true,
            avatar: false,
            pose: 0,
            baby: false,
            scale: 1.0,
        },
    )
    .min[0];
    let inside = p.table_with("minecraft:zombie", 1, 3.0 + half_width - 1e-9, -1.0);
    let (at, just_in) = (p.pick(&on_bound, None), p.pick(&inside, None));
    c.record(
        "g2.the_entity_range_bound_is_strict_and_sampled_on_it",
        near_face == 3.0 && at.is_none() && just_in == Some(1),
        format!(
            "near face at {near_face} (want exactly 3.0): on the bound -> {at:?}, \
             a nanometre nearer -> {just_in:?} (want None/Some(1) — \
             filterHitResult's closerThan is `<`. MUTATION PARTNER: `<` -> `<=` \
             in crosshair_pick's final filter, which makes the first run \
             Some(1). A sample at 2.99/3.01 would not catch it, and neither \
             would one placed with a hand-computed 0.3 half-width)"
        ),
    );

    // -- g3: the range is the ATTRIBUTE, not a hard-coded 3.0 -----------------
    //
    // Driven through the production `apply_local_attributes` with a raw body,
    // so what is graded is the packet the server actually sends. Creative mode
    // is itself a `+2.0` modifier on this attribute, not a special case.
    let far = p.table_with("minecraft:zombie", 1, 4.0, -1.0);
    let before = p.pick(&far, None);
    let entity_range = p
        .f
        .reg
        .id_of("entity_interaction_range")
        .ok_or("no entity_interaction_range attribute")?;
    let stored = rewo_net::attributes::apply_local_attributes(
        &attrs_body(p.camera, &[(entity_range, 5.0)]),
        Some(p.camera),
        &mut p.local,
    );
    let after = p.pick(&far, None);
    c.record(
        "g3.the_reach_comes_from_the_attribute_not_a_constant",
        !before.is_some() && stored && after == Some(1),
        format!(
            "at 4 blocks: default -> {before:?}, after update_attributes raised \
             entity_interaction_range to 5.0 -> {after:?} (stored={stored}; want \
             None then Some(1) — MUTATION PARTNER: hard-code \
             DEFAULT_ENTITY_INTERACTION_RANGE in InteractionRanges::resolve, \
             after which the scaled server behaves identically to the default \
             one and this witness reads None/None)"
        ),
    );
    // A body naming anyone else must not touch the local player's ranges.
    let other = rewo_net::attributes::apply_local_attributes(
        &attrs_body(p.camera + 1, &[(entity_range, 64.0)]),
        Some(p.camera),
        &mut p.local,
    );
    let unchanged = p.pick(&far, None);
    c.record(
        "g4.another_entitys_attributes_are_not_the_local_players",
        !other && unchanged == Some(1),
        format!(
            "stored={other}, pick still {unchanged:?} (want false/Some(1) — the \
             5.0 from g3 survives and the 64.0 addressed to entity {} does not \
             land. MUTATION PARTNER: drop the `packet.entity_id != player` \
             check, after which a distant mob's ranges become the camera's)",
            p.camera + 1
        ),
    );
    // Put the reach back so the witnesses below run at vanilla defaults.
    rewo_net::attributes::apply_local_attributes(
        &attrs_body(p.camera, &[(entity_range, 3.0)]),
        Some(p.camera),
        &mut p.local,
    );

    // -- g5: a block in front wins, and a dead heat goes to the block ---------

    let mob = p.table_with("minecraft:zombie", 1, 2.0, -1.0);
    // The tie sample has to be the mob's own clip point to the bit — see g2.
    let clip = 2.0 - half_width;
    let (nearer, further, level) = (
        p.pick(&mob, Some(1.5)),
        p.pick(&mob, Some(2.5)),
        p.pick(&mob, Some(clip)),
    );
    c.record(
        "g5.a_block_in_front_of_the_mob_wins_and_a_tie_goes_to_the_block",
        nearer.is_none() && further == Some(1) && level.is_none(),
        format!(
            "block at 1.5 -> {nearer:?}, at 2.5 -> {further:?}, at exactly the \
             mob's clip point {clip} -> {level:?} (want None/Some(1)/None. \
             MUTATION PARTNER: replace block_distance_sq with infinity, \
             removing the truncation and the comparison together, after which \
             the first and third runs both read Some(1). It has to be both, \
             and that is a finding rather than a convenience: vanilla enforces \
             this precedence TWICE — the sweep is truncated at the block hit \
             *and* the surviving hit is compared against it — so neither half \
             alone is observable here. The tie in particular is decided by the \
             truncated sweep bound, whose `dd < nearest` is strict; the final \
             `entityHit.distSq < blockDistSq` is then unreachable except \
             through the same-root-vehicle arm that runs after an inside-pick)"
        ),
    );

    let zombie_id = p.f.types.id_of("minecraft:zombie").ok_or("no zombie")?;

    // -- g6: the broad-phase search box is a real filter ---------------------
    //
    // `getEntityHitResult`'s candidates come from `level.getEntities(except,
    // box, matching)`, where `box` is the camera's own bounding box swept along
    // the ray and inflated by 1.0. It can only *remove* candidates, so it is
    // easy to leave out and never notice — this drives it directly, at the
    // `entity_hit_result` level, with a box that excludes an otherwise
    // perfectly hittable candidate.
    //
    // The block-hit **truncation** has no witness of its own, for the reason
    // g5 records: it and the final block comparison are mutually redundant, so
    // removing either alone changes no answer. g5's mutation removes both.
    let reachable = Aabb::new([1.7, -1.0, -0.3], [2.3, 0.95, 0.3]);
    let cand6 = [Candidate {
        id: 1,
        bb: reachable,
        pickable: true,
        pick_radius: 0.0,
        can_be_picked_from_inside: true,
        shares_root_vehicle: false,
    }];
    let wide = Aabb::new([-2.0, -2.0, -2.0], [6.0, 2.0, 2.0]);
    let narrow = Aabb::new([-2.0, -2.0, -2.0], [1.0, 2.0, 2.0]);
    let (in_box, out_box) = (
        entity_hit_result(EYE, [4.5, 0.0, 0.0], &wide, &cand6, 100.0),
        entity_hit_result(EYE, [4.5, 0.0, 0.0], &narrow, &cand6, 100.0),
    );
    c.record(
        "g6.the_broad_phase_search_box_filters_candidates",
        in_box.is_some() && out_box.is_none(),
        format!(
            "search box reaching x=6 -> {:?}; the same ray and candidate with a \
             box stopping at x=1 -> {:?} (want Some/None — `level.getEntities` \
             takes the swept box as its query volume. MUTATION PARTNER: drop \
             the `bb.intersects(search)` filter, after which the second run is \
             Some(1) too)",
            in_box.map(|h| h.id),
            out_box.map(|h| h.id)
        ),
    );

    // -- g7: the inflation is getPickRadius(), which is 0 for a mob -----------
    //
    // A ray 0.5 to the side of a 0.6-wide box misses it outright. The 0.3 in
    // `DEFAULT_ENTITY_HIT_RESULT_MARGIN` — right next to getEntityHitResult in
    // the same file — belongs to the projectile overload, not to this one.
    let mut grazed = EntityTable::default();
    grazed.add(1, EntityState::new(0, zombie_id, 2.0, -1.0, 0.5, 0.0, 0.0));
    let graze = p.pick(&grazed, None);
    // The same geometry with a redirectable projectile, whose pick radius is
    // 1.0, is caught.
    let fireball = p.table_with("minecraft:fireball", 1, 2.0, -0.5);
    let mut fireball_side = EntityTable::default();
    let fb = p.f.types.id_of("minecraft:fireball").ok_or("no fireball")?;
    fireball_side.add(1, EntityState::new(0, fb, 2.0, -0.5, 0.9, 0.0, 0.0));
    let (fb_ahead, fb_side) = (p.pick(&fireball, None), p.pick(&fireball_side, None));
    c.record(
        "g7.the_inflation_is_the_pick_radius_zero_for_a_mob_one_for_a_projectile",
        graze.is_none() && fb_ahead == Some(1) && fb_side == Some(1),
        format!(
            "zombie 0.5 to the side -> {graze:?}; fireball ahead -> {fb_ahead:?}, \
             0.9 to the side -> {fb_side:?} (want None/Some(1)/Some(1) — \
             Entity.getPickRadius() is 0.0F and Projectile's is 1.0F. MUTATION \
             PARTNER: inflate every candidate by DEFAULT_ENTITY_HIT_RESULT_MARGIN \
             (0.3), the constant sitting beside it in ProjectileUtil, after which \
             the grazed zombie becomes Some(1))"
        ),
    );

    // -- g8: nearest wins, and the two runs SWAP which id is nearer ----------
    //
    // Both tables hold ids {1, 2}, so a `HashMap` yields them in the same
    // order for both. That is the point: with `nearest = dd` removed the loop
    // degenerates to last-wins, which returns *the same id* in both runs —
    // and because the near mob is id 2 in one and id 1 in the other, at least
    // one assertion must break. A pair where the near mob had the same id in
    // both would have survived that mutation.
    let mut near_is_2 = p.table_with("minecraft:zombie", 1, 2.5, -1.0);
    near_is_2.add(2, EntityState::new(0, zombie_id, 1.5, -1.0, 0.0, 0.0, 0.0));
    let mut near_is_1 = p.table_with("minecraft:zombie", 1, 1.5, -1.0);
    near_is_1.add(2, EntityState::new(0, zombie_id, 2.5, -1.0, 0.0, 0.0, 0.0));
    let (a, b) = (p.pick(&near_is_2, None), p.pick(&near_is_1, None));
    c.record(
        "g8.the_nearest_of_two_candidates_wins_whichever_id_it_has",
        a == Some(2) && b == Some(1),
        format!(
            "near mob is id 2 -> {a:?}; the same geometry with the ids swapped \
             -> {b:?} (want Some(2)/Some(1) — `dd < nearest` with `nearest` \
             updated on every accepted hit. MUTATION PARTNER: drop the \
             `nearest = dd` update, which degenerates to last-wins and returns \
             one id for both runs)"
        ),
    );

    // -- g9: the sweep bound is strict too, sampled exactly on it ------------
    //
    // `maxValue` seeds `nearest`, so the range bound and the tie-break are the
    // same `<`. Driven at the `entity_hit_result` level because the outer pick
    // filters at the entity range first and would mask it.
    let far_box = Aabb::new([4.5, -1.0, -0.3], [5.1, 0.8, 0.3]);
    let cand = [Candidate {
        id: 1,
        bb: far_box,
        pickable: true,
        pick_radius: 0.0,
        can_be_picked_from_inside: true,
        shares_root_vehicle: false,
    }];
    let search = Aabb::new([-10.0, -10.0, -10.0], [10.0, 10.0, 10.0]);
    let on = entity_hit_result(EYE, [10.0, 0.0, 0.0], &search, &cand, 4.5 * 4.5);
    let over = entity_hit_result(EYE, [10.0, 0.0, 0.0], &search, &cand, 4.6 * 4.6);
    c.record(
        "g9.the_sweep_bound_is_strict_and_sampled_on_it",
        on.is_none() && over.is_some(),
        format!(
            "clip point at exactly 4.5 with maxValue 4.5² -> {:?}; with 4.6² -> \
             {:?} (want None/Some — `dd < nearest` seeded at maxValue, strict. \
             MUTATION PARTNER: `<` -> `<=`, which makes the first run Some. The \
             same comparison is the nearest-wins tie-break, which is why g8 and \
             this witness are the same line of source)",
            on.map(|h| h.id),
            over.map(|h| h.id)
        ),
    );

    // -- g10: the eye inside a box, and canBePickedFromInside ----------------

    let big = Aabb::new([-2.0, -2.0, -2.0], [2.0, 2.0, 2.0]);
    let mut inside_c = Candidate {
        id: 1,
        bb: big,
        pickable: true,
        pick_radius: 0.0,
        can_be_picked_from_inside: true,
        shares_root_vehicle: false,
    };
    let from_inside = entity_hit_result(EYE, [10.0, 0.0, 0.0], &search, &[inside_c], 100.0);
    inside_c.can_be_picked_from_inside = false;
    let refused = entity_hit_result(EYE, [10.0, 0.0, 0.0], &search, &[inside_c], 100.0);
    c.record(
        "g10.an_entity_containing_the_eye_is_picked_from_inside_at_distance_zero",
        from_inside.is_some_and(|h| h.location == EYE && h.distance_sq == 0.0)
            && refused.is_none(),
        format!(
            "{:?} then {:?} (want a hit at the eye itself with distance 0, then \
             None — AABB.clip only tests near faces, so a segment starting \
             inside clips nothing and `clipPoint.orElse(from)` is the live \
             branch. MUTATION PARTNER: canBePickedFromInside flipped, which is \
             the second run)",
            from_inside.map(|h| (h.id, h.distance_sq)),
            refused.map(|h| h.id)
        ),
    );

    // -- g11: isPickable() — the default is FALSE ----------------------------
    //
    // A dropped item entity sits exactly where the zombie of g1 was picked.
    // `Entity.isPickable()` returns false and `minecraft:item` never overrode
    // it, so it is invisible to the crosshair.
    let item = p.table_with("minecraft:item", 1, 2.0, -0.1);
    let dragon = p.table_with("minecraft:ender_dragon", 1, 2.0, -1.0);
    let (item_hit, dragon_hit) = (p.pick(&item, None), p.pick(&dragon, None));
    c.record(
        "g11.an_unpickable_entity_is_never_returned",
        item_hit.is_none() && dragon_hit.is_none() && hit_ahead == Some(1),
        format!(
            "item -> {item_hit:?}, ender_dragon -> {dragon_hit:?}, zombie in the \
             same place -> {hit_ahead:?} (want None/None/Some(1). The dragon is \
             the sharp one: it is a LivingEntity and would inherit `true`, but \
             overrides isPickable() back to false and delegates to its \
             unregistered EnderDragonPart hitboxes. MUTATION PARTNER: treat \
             every PickRule as pickable, after which both become Some(1))"
        ),
    );

    // The rules that produced those answers, asserted directly so a table
    // regenerated from a later version cannot quietly re-classify them.
    let rules = [
        ("minecraft:item", PickRule::Never),
        ("minecraft:ender_dragon", PickRule::Never),
        ("minecraft:zombie", PickRule::Alive),
        ("minecraft:player", PickRule::AliveUnlessSpectator),
        ("minecraft:fireball", PickRule::RedirectableProjectile),
    ];
    let wrong: Vec<String> = rules
        .iter()
        .filter_map(|(name, want)| {
            let got = p.f.types.id_of(name).and_then(|id| p.shapes.get(id));
            (got.map(|s| s.rule) != Some(*want)).then(|| format!("{name}: {got:?}"))
        })
        .collect();
    c.record(
        "g12.the_generated_pick_rules_are_the_decompiled_ones",
        wrong.is_empty(),
        format!(
            "{} mismatch(es): {wrong:?} (want none — machine-extracted by \
             tools/gen_entity_pick.py from every isPickable() declaration under \
             net/, walked against the class graph. MUTATION PARTNER: g11, which \
             is the same five facts observed through the sweep rather than read \
             out of the table)",
            wrong.len()
        ),
    );

    // -- g13: the box is the type's sized(w, h), not a humanoid default ------
    //
    // A zombie is 1.95 tall and a player 1.8. A ray at y = 1.9 above the feet
    // hits one and misses the other — which the pre-M73 hand-written table,
    // whose default was (0.6, 1.8) for everything it did not name, could not
    // have told apart.
    let zombie_tall = p.table_with("minecraft:zombie", 1, 2.0, -1.9);
    let player_short = p.table_with("minecraft:player", 1, 2.0, -1.9);
    let (z_tall, pl_short) = (p.pick(&zombie_tall, None), p.pick(&player_short, None));
    c.record(
        "g13.the_swept_box_is_the_types_own_dimensions",
        z_tall == Some(1) && pl_short.is_none(),
        format!(
            "a ray 1.9 above the feet: zombie (1.95 tall) -> {z_tall:?}, player \
             (1.8 tall) -> {pl_short:?} (want Some(1)/None. MUTATION PARTNER: \
             give every type the builder default 0.6×1.8 — the shape of the \
             hand-written table this replaced — after which the zombie also \
             reads None)"
        ),
    );

    // -- g14: a candidate sharing the camera's root vehicle is skipped -------
    //
    // Through the real `set_passengers` route: the camera and the mob both
    // ride vehicle 50, so `getRootVehicle()` agrees and the mob is not a
    // target. That is how you cannot click the horse you are sitting on.
    let mut shared = p.table_with("minecraft:zombie", 1, 2.0, -1.0);
    // Behind the eye, so the vehicle itself is never the answer — placed at
    // the origin it would *contain* the camera and be picked from inside,
    // which is a real rule (g10) and would mask this one.
    shared.add(50, EntityState::new(0, zombie_id, -5.0, -1.0, 0.0, 0.0, 0.0));
    rewo_net::route_set_passengers(
        p.f.ids.cb_play_set_passengers,
        &passengers_body(50, &[1, p.camera]),
        &p.f.ids,
        &mut shared,
    );
    let shared_hit = p.pick(&shared, None);
    // The same roster with only the mob aboard: the camera's root is itself,
    // so the two differ and the mob is picked again.
    let mut alone = p.table_with("minecraft:zombie", 1, 2.0, -1.0);
    alone.add(50, EntityState::new(0, zombie_id, -5.0, -1.0, 0.0, 0.0, 0.0));
    rewo_net::route_set_passengers(
        p.f.ids.cb_play_set_passengers,
        &passengers_body(50, &[1]),
        &p.f.ids,
        &mut alone,
    );
    let alone_hit = p.pick(&alone, None);
    c.record(
        "g14.a_candidate_sharing_the_cameras_root_vehicle_is_skipped",
        shared_hit.is_none() && alone_hit == Some(1),
        format!(
            "both aboard vehicle 50 -> {shared_hit:?}, mob aboard alone -> \
             {alone_hit:?} (want None/Some(1) — the two runs are each other's \
             MUTATION PARTNER, differing only in whether the camera is in the \
             roster. Dropping the shares_root_vehicle arm makes the first \
             Some(1))"
        ),
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// f — the wiring: a suppressed label emits zero vertices, and both agree.
// ---------------------------------------------------------------------------

/// Resolve entity 1's labels and push them through the real frame path, then
/// report `(text-range vertex count, nametag shown, health bar shown)`.
///
/// The `EntityDraw` is built by [`resolve_labels`] — the seam
/// `collect_entities` fills its `name` and `health` from — rather than by
/// setting the two fields here. M45 and M41 both shipped gates that had quietly
/// stopped testing their subject by reimplementing a slice of the app's setup.
fn label_verts(
    wr: &mut WorldRenderer,
    f: &Fixture,
    t: &EntityTable,
    viewer: &LabelViewer<'_>,
    team: Option<TeamView<'_>>,
    distance_sq: f64,
) -> (u32, bool, bool) {
    let label = label_inputs_from_table(
        t,
        1,
        Some("minecraft:zombie"),
        Some(&f.reg),
        false,
        distance_sq,
        viewer,
        team,
    );
    let (name, health) = resolve_labels(
        t,
        1,
        Some("minecraft:zombie"),
        Some(&f.reg),
        &label,
        Some("Bob"),
    );
    let mut d = crate::healthbarshot_cmd::neutral_draw();
    d.name = name;
    d.health = health;
    wr.set_entities(std::slice::from_ref(&d), [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 0.0);
    (
        wr.entity_pass().expect("entity pass").text_vert_count(),
        name.is_some(),
        health.is_some(),
    )
}

fn check_wiring(
    c: &mut Checker,
    paths: &DataPaths,
    baked: &assets::BakedAssets,
    jar: &std::path::Path,
) -> Result<(), String> {
    let f = Fixture::load(paths)?;
    let mut gpu = Gpu::new(None, true)?;
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let white: Vec<u8> = vec![255u8; 16 * 16 * 4];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
    let font = crate::live_cmd::font_data(baked);
    // Fail closed rather than substituting a cell: without the baked font
    // `has_font` is false and the emitter draws nothing, so every count below
    // would be a silent zero and every suppression witness would pass for the
    // wrong reason.
    font.as_ref()
        .ok_or("no baked font — a label needs the font atlas's white texel")?;
    wr.init_entities(&mut gpu, font, crate::live_cmd::entity_textures(baked))?;
    let (right, up) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);

    // Build an `EntityDraw` the way `collect_entities` does — through
    // `resolve_labels`, the seam it shares with this gate, rather than by
    // setting `name` and `health` here. M45 and M41 both shipped gates that had
    // quietly stopped testing their subject by reimplementing a slice of the
    // app's setup.
    let base = f.healthy_named_zombie();
    let (shown, shown_name, shown_bar) = label_verts(&mut wr, &f, &base, &LabelViewer::default(), None, 4.0);
    c.record(
        "f1.a_visible_entity_emits_label_vertices",
        shown > 0 && shown_name && shown_bar,
        format!(
            "{shown} text verts (name={shown_name} bar={shown_bar}) (want a \
             non-zero count and both labels — this is the baseline every \
             suppression witness below is measured against. A gate that only \
             ever measured zero would pass for a renderer that drew nothing)"
        ),
    );

    // Each suppression, straight to a vertex count.
    let mut invisible = f.healthy_named_zombie();
    f.send_meta(&mut invisible, &meta_body(1, 0, 0, &[0x20]));
    let (inv_verts, ..) = label_verts(&mut wr, &f, &invisible, &LabelViewer::default(), None, 4.0);
    c.record(
        "f2.an_invisible_entity_emits_zero_label_vertices",
        inv_verts == 0,
        format!(
            "{inv_verts} text verts (want 0 — MUTATION PARTNER f1, the same \
             entity without shared flag 5, which emits {shown}). This is the \
             end-to-end form of c1: the predicate's answer really does reach \
             the vertex buffer)"
        ),
    );

    let mut sneaking = f.healthy_named_zombie();
    f.send_meta(&mut sneaking, &meta_body(1, 0, 0, &[0x02]));
    let (sneak_far, ..) = label_verts(
        &mut wr,
        &f,
        &sneaking,
        &LabelViewer::default(),
        None,
        DISCRETE_MAX_DISTANCE_SQ,
    );
    let (sneak_near, ..) = label_verts(&mut wr, &f, &sneaking, &LabelViewer::default(), None, 4.0);
    c.record(
        "f3.a_sneaking_entity_past_the_cut_off_emits_zero_label_vertices",
        sneak_far == 0 && sneak_near > 0,
        format!(
            "at distSq {DISCRETE_MAX_DISTANCE_SQ}: {sneak_far} verts; at distSq \
             4: {sneak_near} verts (want 0 and non-zero — the same sneaking \
             entity at two distances, so the pair isolates the cut-off from \
             everything else. MUTATION PARTNER b2, the same distance without \
             the sneak flag)"
        ),
    );

    let ridden = f.ridden_zombie(&[2]);
    let (ridden_verts, ..) = label_verts(&mut wr, &f, &ridden, &LabelViewer::default(), None, 4.0);
    c.record(
        "f4.a_ridden_entity_emits_zero_label_vertices",
        ridden_verts == 0,
        format!(
            "{ridden_verts} text verts (want 0 — MUTATION PARTNER f1, the same \
             entity with nobody aboard, which emits {shown})"
        ),
    );

    let (hud_verts, ..) = label_verts(
        &mut wr,
        &f,
        &base,
        &LabelViewer {
            hud_hidden: true,
            ..Default::default()
        },
        None,
        4.0,
    );
    c.record(
        "f5.f1_emits_zero_label_vertices",
        hud_verts == 0,
        format!(
            "{hud_verts} text verts (want 0 — MUTATION PARTNER f1, the \
             identical entity with the HUD showing, which emits {shown})"
        ),
    );

    // The agreement property the milestone exists for.
    let mut agree_failures = Vec::new();
    let mut agree_shown = 0;
    let mut invis = f.healthy_named_zombie();
    f.send_meta(&mut invis, &meta_body(1, 0, 0, &[0x20]));
    let ridden2 = f.ridden_zombie(&[2]);
    let mob_member = rewo_net::play::uuid_to_dashed(0);
    let mut never_team = Teams::new();
    never_team.apply(
        &parse_set_player_team(&team_body("red", V_NEVER, 0, &[&mob_member])).expect("team"),
    );
    let never_view = rewo_net::teams::label_team(&never_team, &mob_member);
    let cases: [(&str, &EntityTable, LabelViewer, Option<TeamView>); 6] = [
        ("visible", &base, LabelViewer::default(), None),
        ("invisible", &invis, LabelViewer::default(), None),
        ("ridden", &ridden2, LabelViewer::default(), None),
        (
            "f1",
            &base,
            LabelViewer {
                hud_hidden: true,
                ..Default::default()
            },
            None,
        ),
        (
            "camera",
            &base,
            LabelViewer {
                camera_entity: Some(1),
                local_player: Some(1),
                ..Default::default()
            },
            None,
        ),
        ("team_never", &base, LabelViewer::default(), never_view),
    ];
    for (label, table, viewer, team) in cases {
        let (verts, name, bar) = label_verts(&mut wr, &f, table, &viewer, team, 4.0);
        if name {
            agree_shown += 1;
        }
        // A suppressed label suppresses BOTH — never one and not the other.
        if name != bar || (name && verts == 0) || (!name && verts != 0) {
            agree_failures.push(format!("{label}: name={name} bar={bar} verts={verts}"));
        }
    }
    c.record(
        "f6.the_nametag_and_the_health_bar_agree_under_every_rule",
        agree_failures.is_empty() && agree_shown > 0,
        format!(
            "{} disagreement(s) over 6 scenarios, {agree_shown} of which showed \
             a label: {:?} (want none, and a non-zero shown count so the \
             property is not vacuous — M59's e3/e4 passed on two empty vectors \
             for exactly that reason. Before M70 the bar had a three-gate \
             subset of shouldShowName and the tag had none, so 'invisible' \
             disagreed: a name and no bar. MUTATION PARTNER: giving either \
             feature its own gate list again)",
            agree_failures.len(),
            agree_failures
        ),
    );

    // -- f7: the clause M70 had to stub, end to end (M73) ---------------------
    //
    // A name-tagged mob whose `CustomNameVisible` is unset shows its tag only
    // while it is under the crosshair. This is the property M73 exists for, and
    // it is measured as a *vertex count* — the crosshair pick's answer really
    // does reach the buffer — rather than as a boolean the gate computed.
    //
    // Nothing about the pick is stubbed here: `crosshair_pick_from_table` is
    // the production seam, and the only difference between the two runs is
    // where the mob is standing.
    let p = PickFixture::load(paths, jar)?;
    // A named mob whose index-3 CUSTOM_NAME_VISIBLE was never sent, standing
    // at `(2, -1, z)`. Only `z` differs between the two runs.
    //
    // No `max_health` is synced, deliberately: rule 4 then refuses the health
    // bar, so the text range holds the **nametag alone** and the count is a
    // measurement of this clause rather than of the bar riding alongside it.
    let named_hidden_at = |z: f64| -> EntityTable {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, f.zombie, 2.0, -1.0, z, 0.0, 0.0));
        t.set_custom_name(1, Some("Bob".into()));
        t
    };
    let mut run = |z: f64| {
        let t = named_hidden_at(z);
        let pick = p.pick(&t, None);
        let viewer = LabelViewer {
            crosshair_pick: pick,
            ..Default::default()
        };
        let (verts, name, _) = label_verts(&mut wr, &f, &t, &viewer, None, 4.0);
        (verts, name, pick)
    };
    let (on_verts, on_name, on_pick) = run(0.0);
    let (off_verts, off_name, off_pick) = run(4.0);
    c.record(
        "f7.a_name_tagged_mob_shows_its_tag_only_under_the_crosshair",
        on_pick == Some(1) && on_name && on_verts > 0 && off_pick.is_none() && !off_name && off_verts == 0,
        format!(
            "on the ray: pick={on_pick:?} name={on_name} verts={on_verts}; four \
             blocks to the side: pick={off_pick:?} name={off_name} \
             verts={off_verts} (want a pick and vertices, then neither. This is \
             the clause M70 transcribed and fed a hard `false` — the two runs \
             are each other's MUTATION PARTNER, differing only in the mob's z. \
             Feeding `false` again, as M70 did, makes the first run zero)"
        ),
    );

    check_name_flatten(c, &f, baked);

    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}

/// M163 — `Entity.DATA_CUSTOM_NAME` is a **component**, and the wire decode
/// used to flatten it with `Nbt::to_plain_text`.
///
/// That flatten has no language table and no legacy-code parser, so on any
/// server that names a mob with a `translate` component or a `§` code the
/// nametag drew the raw key or the two code characters. Vanilla draws neither:
/// `Language.getVisualOrder` (`Language.java:59-65`) runs
/// `StringDecomposer.iterateFormatted` over every literal, which parses `§`
/// pairs into style, and `TranslatableContents.decompose` resolves the key.
///
/// Every witness here drives the **production router** with a raw
/// `set_entity_data` body and reads `EntityTable::custom_name`, i.e. exactly
/// the string `live_cmd::resolve_labels` hands to `EntityDraw::name`.
fn check_name_flatten(c: &mut Checker, f: &Fixture, baked: &assets::BakedAssets) {
    /// One `Entity.DATA_CUSTOM_NAME` entry: index 2, OPTIONAL_COMPONENT
    /// (serializer 6), present, carrying `tag` as network NBT.
    fn named(tag: &[u8]) -> Vec<u8> {
        let mut v = vec![0x01u8];
        v.extend_from_slice(tag);
        meta_body(1, 2, 6, &v)
    }
    fn nbt_str(s: &str) -> Vec<u8> {
        let mut v = vec![8u8];
        v.extend_from_slice(&(s.len() as u16).to_be_bytes());
        v.extend_from_slice(s.as_bytes());
        v
    }
    /// `{translate: "<key>"}` as a network `TAG_Compound`.
    fn nbt_translate(key: &str) -> Vec<u8> {
        let mut v = vec![10u8, 8u8];
        v.extend_from_slice(&(9u16).to_be_bytes());
        v.extend_from_slice(b"translate");
        v.extend_from_slice(&(key.len() as u16).to_be_bytes());
        v.extend_from_slice(key.as_bytes());
        v.push(0); // TAG_End
        v
    }

    // The expectation is the JAR's own answer, not a literal written here:
    // `baked.lang` is `en_us.json` after M54's deprecation pass, and
    // `entity.minecraft.zombie` is what `Entity.getTypeName()` would name a
    // zombie with. If the jar ever renames it, this moves with it.
    let expected = baked.lang.get("entity.minecraft.zombie").unwrap_or("").to_string();

    let resolved = {
        let mut t = f.spawn(f.zombie);
        f.send_meta_lang(&mut t, &named(&nbt_translate("entity.minecraft.zombie")), Some(&baked.lang));
        t.custom_name(1).map(str::to_string)
    };
    let unresolved = {
        let mut t = f.spawn(f.zombie);
        f.send_meta_lang(&mut t, &named(&nbt_translate("entity.minecraft.zombie")), None);
        t.custom_name(1).map(str::to_string)
    };
    c.record(
        "fl1.a_translatable_nametag_resolves_at_the_wire",
        !expected.is_empty()
            && resolved.as_deref() == Some(expected.as_str())
            && resolved != unresolved,
        format!(
            "with the jar's table {resolved:?}, without one {unresolved:?} \
             (en_us says {expected:?}). The second is what EVERY client built \
             before M163 drew, because `metadata::parse` called \
             `Nbt::to_plain_text`, which has no table and never reads `with`"
        ),
    );

    // `getOrDefault(key)`'s key-as-default has to survive: a resolver that
    // blanked an unknown key would satisfy `fl1` and lose every plugin-defined
    // name. This is that half, and it is not the same as `fl1`'s `None` arm —
    // here the TABLE EXISTS and simply does not hold the key.
    let missing = {
        let mut t = f.spawn(f.zombie);
        f.send_meta_lang(&mut t, &named(&nbt_translate("plugin.example.boss")), Some(&baked.lang));
        t.custom_name(1).map(str::to_string)
    };
    c.record(
        "fl2.an_unknown_key_falls_back_to_the_key_itself",
        missing.as_deref() == Some("plugin.example.boss"),
        format!(
            "{missing:?} — `Language.getOrDefault` answers the key, so a server \
             using its own keys renders them rather than blanking. Without this \
             `fl1` is satisfied by a resolver that returns an empty string for \
             everything it does not know"
        ),
    );

    // The witness that separates `to_plain_text` from a real flatten. A `§`
    // pair is STYLE, so neither character is drawn — and this is the one that
    // fires if someone "resolves" by bolting a `translate` branch onto
    // `Nbt::to_plain_text` rather than using the component parser.
    let coded = {
        let mut t = f.spawn(f.zombie);
        f.send_meta_lang(&mut t, &named(&nbt_str("\u{00a7}cRed")), Some(&baked.lang));
        t.custom_name(1).map(str::to_string)
    };
    c.record(
        "fl3.a_legacy_code_in_a_nametag_becomes_style_not_characters",
        coded.as_deref() == Some("Red"),
        format!(
            "{coded:?} — before M163 this was the four characters \
             \"\u{00a7}cRed\", section sign and all, over the mob's head"
        ),
    );

    // `DATA_CUSTOM_NAME` is `Optional<Component>` (`Entity.java:269-271`), so
    // the wire's leading `false` is `Optional.empty()` — a CLEAR. The decode
    // had no `else` arm, so `/data remove entity @e CustomName` never removed
    // anything: `EntityTable::set_custom_name` already implemented the removal
    // and was unreachable.
    let cleared = {
        let mut t = f.spawn(f.zombie);
        f.send_meta_lang(&mut t, &named(&nbt_str("Bob")), Some(&baked.lang));
        let before = t.custom_name(1).map(str::to_string);
        f.send_meta_lang(&mut t, &meta_body(1, 2, 6, &[0x00]), Some(&baked.lang));
        (before, t.custom_name(1).map(str::to_string))
    };
    c.record(
        "fl4.an_absent_optional_clears_the_nametag",
        cleared.0.as_deref() == Some("Bob") && cleared.1.is_none(),
        format!(
            "set {:?} then cleared to {:?} — the accessor is \
             `EntityDataAccessor<Optional<Component>>`, and the decode used to \
             read the presence bit with `unwrap_or(false)` and no `else`, so an \
             explicit clear was indistinguishable from index 2 being absent and \
             the old name stood forever",
            cleared.0, cleared.1
        ),
    );

    // …and a present-but-empty name is a PRESENT name. `hasCustomName()` is
    // `isPresent()`, not "is non-blank" — the old decode dropped an empty
    // string, which merged the two states the packet keeps apart.
    let empty = {
        let mut t = f.spawn(f.zombie);
        f.send_meta_lang(&mut t, &named(&nbt_str("Bob")), Some(&baked.lang));
        f.send_meta_lang(&mut t, &named(&nbt_str("")), Some(&baked.lang));
        t.custom_name(1).map(str::to_string)
    };
    c.record(
        "fl5.a_present_but_empty_name_is_present_rather_than_dropped",
        empty.as_deref() == Some(""),
        format!(
            "{empty:?} — `Entity.hasCustomName()` is \
             `entityData.get(DATA_CUSTOM_NAME).isPresent()`, so an empty \
             component is a name. Dropping it left the PREVIOUS name standing, \
             which is a different picture from the one the server sent"
        ),
    );
}
