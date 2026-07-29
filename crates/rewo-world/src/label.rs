//! Entity-label visibility — whether a floating label is drawn over an entity
//! at all (M70).
//!
//! Three features share this predicate and each used to implement a slice of
//! it: the nametag (M2-era, which gated on nothing but "is there a name"), the
//! health bar (M59, which gated on distance + invisible + living), and
//! anything else that floats. This module is the single answer.
//!
//! Transcribed from the 26.2 decompile:
//! `net/minecraft/client/renderer/entity/EntityRenderer.java`
//! (`shouldShowName`, `extractNameTags`),
//! `.../LivingEntityRenderer.java` (the override that carries the real rules),
//! `.../MobRenderer.java` and `.../player/AvatarRenderer.java` (which add a
//! conjunct back on), `.../ArmorStandRenderer.java` (which discards all of it),
//! `net/minecraft/world/entity/Entity.java` (`isDiscrete`, `isVehicle`,
//! `isInvisibleTo`, `getTeam`, `shouldShowName`) and
//! `net/minecraft/world/scores/Team.java`.
//!
//! # The ladder
//!
//! `shouldShowName` is overridden four times and they do not compose the way
//! the class names suggest. In call order, with `nameSource` standing for
//! `EntityRenderer`'s own body — `entity.shouldShowName() || (hasCustomName()
//! && entity == crosshairPickEntity)` — and `living` for
//! `LivingEntityRenderer`'s:
//!
//! | renderer | rule |
//! |---|---|
//! | `EntityRenderer` (boat, minecart, item entity, …) | `nameSource` |
//! | `LivingEntityRenderer` | `living` |
//! | `MobRenderer` (every `Mob`) | `living && nameSource` |
//! | `AvatarRenderer` (players) | `living && nameSource` |
//! | `ArmorStandRenderer` | `isCustomNameVisible()`, and **nothing else** |
//!
//! Two of those are easy to get wrong and both are invertible:
//!
//! - **`LivingEntityRenderer` does not consult `nameSource` itself.** Read it
//!   alone and every mob in the world wears its type name, because
//!   `extractNameTags` then assigns `getNameTag(entity)` =
//!   `entity.getDisplayName()`, which is never null. `MobRenderer` and
//!   `AvatarRenderer` re-add the clause; they are what make an un-named mob
//!   silent. Note they spell the clause out rather than calling a shared
//!   helper, so it is genuinely duplicated in vanilla too.
//! - **`ArmorStandRenderer` is a full override**, not an extension. An armour
//!   stand's name ignores the sneak cut-off, every team rule, invisibility,
//!   the camera entity and `isVehicle` alike — `isCustomNameVisible()` is the
//!   entire predicate.
//!
//! # The early return inside the team branch
//!
//! The load-bearing shape of `LivingEntityRenderer.shouldShowName` is that the
//! team `switch` **returns**, so `hud.isHidden()`, `getCameraEntity()` and
//! `isVehicle()` are only ever reached by an entity with **no** team. A player
//! on a team keeps their nametag with F1 pressed, and a teamed horse keeps its
//! nametag while ridden. That is not a quirk of this transcription; it is what
//! the source does.
//!
//! # The crosshair clause (M70 stubbed it, M73 answers it)
//!
//! `crosshairPickEntity` needs an entity raycast, and M70 shipped while Rewo's
//! raycast was still voxel-only, so [`LabelInputs::is_crosshair_pick`] was a
//! real input the gate drove both ways while the live client fed it `false`.
//! [`crate::entity_pick`] now answers it — `Minecraft.pick` →
//! `LocalPlayer.raycastHitResult` → `ProjectileUtil.getEntityHitResult`, with
//! both interaction ranges resolved from their attributes rather than
//! hard-coded. The one piece still unimplemented is the `AttackRange` branch,
//! which is a different algorithm entirely and inert unless a spear is held;
//! that module's docs record it.
//!
//! It stays a plain field rather than a callback for the same reason every
//! other input here is one: the gate drives both branches directly, and
//! exactly one place — `live_cmd::resolve_crosshair_pick` — knows where the
//! value comes from.

/// `Team.Visibility`.
///
/// Mirrored here rather than imported so `rewo-world` keeps no dependency on
/// `rewo-net`, where the wire enum lives. The two are kept in step by
/// `rewo-net`'s conversion and a test on both sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameTagVisibility {
    Always,
    Never,
    HideForOtherTeams,
    HideForOwnTeam,
}

/// Which `shouldShowName` override an entity's renderer supplies.
///
/// Vanilla selects this by renderer class, which follows the entity's Java
/// type. Rewo has no renderer registry, so the caller maps it — see
/// [`LabelRenderer::is_living`] for the one equivalence that does the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelRenderer {
    /// `AvatarRenderer` — the player.
    Avatar,
    /// `MobRenderer` — every `Mob`, which is every living entity that is not
    /// a player or an armour stand.
    Mob,
    /// `ArmorStandRenderer`.
    ArmorStand,
    /// `EntityRenderer` — everything non-living.
    Other,
}

impl LabelRenderer {
    /// Whether this renderer's entity is a `LivingEntity`.
    ///
    /// Two things key off it: `LivingEntityRenderer.extractNameTags` reads the
    /// `NAME_TAG_DISTANCE` attribute where the base takes a flat 64.0, and a
    /// health bar exists only for something with health at all.
    pub fn is_living(self) -> bool {
        !matches!(self, LabelRenderer::Other)
    }
}

/// The entity's team, as much of it as `shouldShowName` reads.
///
/// **Team identity is by name.** Vanilla compares `PlayerTeam` object
/// references (`Team.isAlliedTo` is `other == null ? false : this == other`,
/// and `player.getTeam() == team` in `isInvisibleTo`), but `Scoreboard` keys
/// teams by name and holds exactly one object per name, so reference equality
/// and name equality coincide. Note this is *identity*, not alliance in any
/// looser sense: two different teams are never allied in vanilla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeamView<'a> {
    pub name: &'a str,
    /// `PlayerTeam.getNameTagVisibility()`.
    pub visibility: NameTagVisibility,
    /// `PlayerTeam.canSeeFriendlyInvisibles()` — the packed options byte's
    /// bit 1.
    pub see_friendly_invisibles: bool,
}

/// Everything the predicate reads, about one entity and about the viewer.
///
/// A plain struct of resolved values rather than a set of callbacks, so the
/// gate can drive every combination directly and the live call site is the
/// only place that has to know where each value comes from.
#[derive(Debug, Clone, Copy)]
pub struct LabelInputs<'a> {
    /// Which override applies.
    pub renderer: LabelRenderer,
    /// `EntityRenderState.distanceToCameraSq`.
    pub distance_sq: f64,
    /// The `extractNameTags` bound. `LivingEntityRenderer` passes
    /// `Attributes.NAME_TAG_DISTANCE`'s resolved value; the base passes
    /// [`DEFAULT_NAME_TAG_DISTANCE`].
    pub name_tag_distance: f64,
    /// `Entity.isDiscrete()` — which is `isShiftKeyDown()`, which is shared
    /// flag bit 1. Sneaking, in other words.
    pub is_discrete: bool,
    /// `Entity.isInvisible()` — shared flag bit 5.
    pub is_invisible: bool,
    /// `Entity.isVehicle()` — `!passengers.isEmpty()`, i.e. **something is
    /// riding this entity**. Not "this entity is riding something".
    pub is_vehicle: bool,
    /// `entity == minecraft.getCameraEntity()`.
    pub is_camera_entity: bool,
    /// `entity == minecraft.player`. Distinct from the above while
    /// spectating, and it guards a different clause.
    pub is_local_player: bool,
    /// `Minecraft.getInstance().gui.hud.isHidden()` — F1.
    pub hud_hidden: bool,
    /// `player.isSpectator()`, the viewer's own game mode.
    pub viewer_spectator: bool,
    /// The entity's team.
    pub team: Option<TeamView<'a>>,
    /// The viewer's team name.
    pub viewer_team: Option<&'a str>,
    /// `entity.shouldShowName()` — `true` for a player, `isCustomNameVisible()`
    /// for everything else.
    pub entity_should_show_name: bool,
    /// `entity.hasCustomName()`.
    pub has_custom_name: bool,
    /// `entity == entityRenderDispatcher.crosshairPickEntity`. See the module
    /// docs: Rewo feeds `false` and the gate drives both.
    pub is_crosshair_pick: bool,
}

/// `Entity.DEFAULT_NAME_TAG_DISTANCE`, and the literal
/// `EntityRenderer.extractNameTags` passes when no override supplies one.
pub const DEFAULT_NAME_TAG_DISTANCE: f64 = 64.0;

/// The sneak cut-off, **squared and hard-coded**.
///
/// The source reads
///
/// ```text
/// if (entity.isDiscrete()) {
///    float maxDist = 32.0F;
///    if (distanceToCameraSq >= 1024.0) {
///       return false;
///    }
/// }
/// ```
///
/// `maxDist` is assigned and never read — the constant that survives
/// compilation is the folded `1024.0`. That matters for more than tidiness:
/// this bound is **not** `Mth.square(nameTagDistance)`, so a server that
/// raises `NAME_TAG_DISTANCE` does not move the sneak cut-off with it. A
/// sneaking entity is capped at 32 blocks however far the attribute reaches.
pub const DISCRETE_MAX_DISTANCE_SQ: f64 = 1024.0;

/// `state.distanceToCameraSq < Mth.square(nameTagDistance)` — the outer gate in
/// `EntityRenderer.extractNameTags`, which runs before any override.
///
/// Written as a positive `<` rather than a negated `>=` so a NaN distance
/// (which compares false against everything) suppresses the label instead of
/// showing it.
pub fn within_name_tag_distance(i: &LabelInputs) -> bool {
    i.distance_sq < i.name_tag_distance * i.name_tag_distance
}

/// `Entity.isInvisibleTo(Player)`.
///
/// ```text
/// if (player.isSpectator()) return false;
/// Team team = this.getTeam();
/// return team != null && player != null && player.getTeam() == team
///     && team.canSeeFriendlyInvisibles() ? false : this.isInvisible();
/// ```
///
/// **The spectator arm is the one that reads backwards**, and a note in
/// `REWO_HEALTH_BAR_SPEC.md` rule 5 exists because of it: it returns `false`,
/// meaning *not* invisible, so a spectating viewer **sees** invisible
/// entities. There is no "spectators are hidden" rule here at all; the clause
/// people reach for is the separate `entity != getCameraEntity()` below.
///
/// The team arm requires the viewer to be on the **same** team — vanilla's
/// `==` on the team object, which is name equality here — not merely on some
/// allied one.
pub fn is_invisible_to_viewer(i: &LabelInputs) -> bool {
    if i.viewer_spectator {
        return false;
    }
    match (i.team, i.viewer_team) {
        (Some(t), Some(mine)) if t.name == mine && t.see_friendly_invisibles => false,
        _ => i.is_invisible,
    }
}

/// `LivingEntityRenderer.shouldShowName` — the suppression half, and the part
/// every floating label shares.
///
/// Does **not** include the `nameSource` clause; `MobRenderer` and
/// `AvatarRenderer` add that on top and [`should_show_name`] applies it.
pub fn living_should_show_name(i: &LabelInputs) -> bool {
    // The sneak cut-off. Note the guard is on `isDiscrete()` and the bound is
    // absolute — see `DISCRETE_MAX_DISTANCE_SQ`.
    if i.is_discrete && i.distance_sq >= DISCRETE_MAX_DISTANCE_SQ {
        return false;
    }

    let visible_to_player = !is_invisible_to_viewer(i);

    // The team branch **returns**, so nothing below it runs for a teamed
    // entity. `hud.isHidden()`, the camera entity and `isVehicle()` are all
    // unreachable from here.
    if !i.is_local_player {
        if let Some(team) = i.team {
            return match team.visibility {
                NameTagVisibility::Always => visible_to_player,
                NameTagVisibility::Never => false,
                // `team.isAlliedTo(myTeam)` — the viewer must be on this very
                // team. `canSeeFriendlyInvisibles` can carry it past an
                // invisible entity, which is the only place in the whole
                // predicate where invisibility is overridden by a team.
                NameTagVisibility::HideForOtherTeams => match i.viewer_team {
                    None => visible_to_player,
                    Some(mine) => {
                        team.name == mine && (team.see_friendly_invisibles || visible_to_player)
                    }
                },
                // `!team.isAlliedTo(myTeam)` — and no `canSeeFriendlyInvisibles`
                // escape on this arm, deliberately: it is not symmetric.
                NameTagVisibility::HideForOwnTeam => match i.viewer_team {
                    None => visible_to_player,
                    Some(mine) => team.name != mine && visible_to_player,
                },
            };
        }
    }

    !i.hud_hidden && !i.is_camera_entity && visible_to_player && !i.is_vehicle
}

/// `EntityRenderer.shouldShowName`'s own body — "is there a name to show".
///
/// A source-of-name test rather than a suppression test, which is why the
/// health bar does not take it.
fn name_source(i: &LabelInputs) -> bool {
    i.entity_should_show_name || (i.has_custom_name && i.is_crosshair_pick)
}

/// Whether a **nametag** may be drawn — the exact vanilla predicate, outer
/// distance gate included.
pub fn should_show_name(i: &LabelInputs) -> bool {
    if !within_name_tag_distance(i) {
        return false;
    }
    match i.renderer {
        // `MobRenderer` / `AvatarRenderer`: `super && nameSource`.
        LabelRenderer::Mob | LabelRenderer::Avatar => living_should_show_name(i) && name_source(i),
        // `ArmorStandRenderer`: `entity.isCustomNameVisible()` alone, which is
        // exactly what `LivingEntity.shouldShowName()` returns — so the same
        // input serves, and every suppression rule is skipped.
        LabelRenderer::ArmorStand => i.entity_should_show_name,
        // `EntityRenderer`: the base body.
        LabelRenderer::Other => name_source(i),
    }
}

/// Whether a **health bar** may be drawn, as far as visibility is concerned.
///
/// `REWO_HEALTH_BAR_SPEC.md` rule 5: living entities only, "suppressed by
/// everything that suppresses a nametag". The spec then enumerates invisible,
/// spectator and the name-render distance — the *suppression* rules — so this
/// is [`living_should_show_name`] behind the distance gate, and deliberately
/// **not** `name_source`: a mob with no name still has health, and gating a bar
/// on there being a name would hide it from every un-named mob in the world.
///
/// **One recorded divergence.** An armour stand takes these rules too, though
/// vanilla's `ArmorStandRenderer` skips them for its *name*. That override is
/// about names; the spec is the authority for bars, and letting an invisible
/// armour stand keep a health bar because of a naming quirk would be the
/// stranger answer.
///
/// The caller still applies the rest of rule 5 — a bar needs a max health an
/// `update_attributes` actually established (rule 4), which is what makes this
/// fail closed for anything that never reports one.
pub fn should_show_health_bar(i: &LabelInputs) -> bool {
    i.renderer.is_living() && within_name_tag_distance(i) && living_should_show_name(i)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A visible, un-teamed, un-sneaking mob at point-blank range with a
    /// visible custom name — every rule satisfied, so each test below can turn
    /// exactly one thing off.
    fn mob() -> LabelInputs<'static> {
        LabelInputs {
            renderer: LabelRenderer::Mob,
            distance_sq: 4.0,
            name_tag_distance: DEFAULT_NAME_TAG_DISTANCE,
            is_discrete: false,
            is_invisible: false,
            is_vehicle: false,
            is_camera_entity: false,
            is_local_player: false,
            hud_hidden: false,
            viewer_spectator: false,
            team: None,
            viewer_team: None,
            entity_should_show_name: true,
            has_custom_name: true,
            is_crosshair_pick: false,
        }
    }

    #[test]
    fn the_baseline_shows_both_labels() {
        let i = mob();
        assert!(should_show_name(&i));
        assert!(should_show_health_bar(&i));
    }

    #[test]
    fn the_discrete_cut_off_is_1024_squared_and_strict() {
        let mut i = mob();
        i.is_discrete = true;
        // Just inside: 31.99 blocks.
        i.distance_sq = DISCRETE_MAX_DISTANCE_SQ - 0.01;
        assert!(should_show_name(&i), "just inside 32 blocks must show");
        // Exactly at the bound. The comparison is `>=`, so 32.0 blocks is out.
        i.distance_sq = DISCRETE_MAX_DISTANCE_SQ;
        assert!(!should_show_name(&i), "the bound itself is excluded");
        // And not sneaking at the same distance is unaffected.
        i.is_discrete = false;
        assert!(should_show_name(&i));
    }

    #[test]
    fn the_sneak_cut_off_does_not_scale_with_the_name_tag_distance() {
        // The distinguishing property: raising the attribute moves the outer
        // gate and leaves the sneak bound where it was.
        let mut i = mob();
        i.name_tag_distance = 128.0;
        i.distance_sq = 40.0 * 40.0;
        assert!(should_show_name(&i), "not sneaking, inside 128");
        i.is_discrete = true;
        assert!(!should_show_name(&i), "sneaking is still capped at 32");
    }

    #[test]
    fn the_outer_distance_gate_is_strict_and_uses_the_attribute() {
        let mut i = mob();
        i.name_tag_distance = 64.0;
        i.distance_sq = 64.0 * 64.0 - 0.01;
        assert!(should_show_name(&i));
        i.distance_sq = 64.0 * 64.0;
        assert!(!should_show_name(&i), "`<` excludes the bound");
        i.name_tag_distance = 128.0;
        assert!(should_show_name(&i), "a raised attribute reaches further");
    }

    #[test]
    fn a_nan_distance_suppresses_rather_than_shows() {
        let mut i = mob();
        i.distance_sq = f64::NAN;
        assert!(!should_show_name(&i));
        assert!(!should_show_health_bar(&i));
    }

    #[test]
    fn an_invisible_entity_shows_neither_label() {
        let mut i = mob();
        i.is_invisible = true;
        assert!(!should_show_name(&i));
        assert!(!should_show_health_bar(&i));
    }

    #[test]
    fn a_spectating_viewer_sees_invisible_entities() {
        // The rule that reads backwards: `isInvisibleTo` returns false for a
        // spectator, so the label comes back.
        let mut i = mob();
        i.is_invisible = true;
        assert!(!should_show_name(&i));
        i.viewer_spectator = true;
        assert!(should_show_name(&i), "a spectator sees invisible entities");
    }

    #[test]
    fn a_ridden_entity_hides_its_labels() {
        let mut i = mob();
        i.is_vehicle = true;
        assert!(!should_show_name(&i));
        assert!(!should_show_health_bar(&i));
    }

    #[test]
    fn f1_hides_labels() {
        let mut i = mob();
        i.hud_hidden = true;
        assert!(!should_show_name(&i));
        assert!(!should_show_health_bar(&i));
    }

    #[test]
    fn the_camera_entity_shows_no_label() {
        let mut i = mob();
        i.is_camera_entity = true;
        assert!(!should_show_name(&i));
        assert!(!should_show_health_bar(&i));
    }

    #[test]
    fn a_team_short_circuits_hud_vehicle_and_camera() {
        // The early return. Each of these three alone suppresses an un-teamed
        // entity and none of them reaches a teamed one.
        let team = TeamView {
            name: "red",
            visibility: NameTagVisibility::Always,
            see_friendly_invisibles: false,
        };
        let cases: [(&str, fn(&mut LabelInputs)); 3] = [
            ("hud", |i| i.hud_hidden = true),
            ("vehicle", |i| i.is_vehicle = true),
            ("camera", |i| i.is_camera_entity = true),
        ];
        for (label, apply) in cases {
            let mut i = mob();
            apply(&mut i);
            assert!(!should_show_name(&i), "{label}: un-teamed must suppress");
            i.team = Some(team);
            assert!(
                should_show_name(&i),
                "{label}: the team switch returns before this is read"
            );
        }
    }

    #[test]
    fn visibility_never_hides_the_label_outright() {
        let mut i = mob();
        i.team = Some(TeamView {
            name: "red",
            visibility: NameTagVisibility::Never,
            see_friendly_invisibles: false,
        });
        assert!(!should_show_name(&i));
        assert!(!should_show_health_bar(&i));
    }

    #[test]
    fn hide_for_other_teams_needs_the_viewer_on_the_same_team() {
        let mut i = mob();
        i.team = Some(TeamView {
            name: "red",
            visibility: NameTagVisibility::HideForOtherTeams,
            see_friendly_invisibles: false,
        });
        // A teamless viewer is the pass-through case, not the hidden one.
        i.viewer_team = None;
        assert!(should_show_name(&i), "no team of my own -> visible");
        i.viewer_team = Some("blue");
        assert!(!should_show_name(&i), "another team -> hidden");
        i.viewer_team = Some("red");
        assert!(should_show_name(&i), "the same team -> visible");
    }

    #[test]
    fn hide_for_own_team_is_the_exact_inverse_on_the_teamed_arms() {
        let mut i = mob();
        i.team = Some(TeamView {
            name: "red",
            visibility: NameTagVisibility::HideForOwnTeam,
            see_friendly_invisibles: false,
        });
        i.viewer_team = None;
        assert!(should_show_name(&i), "no team of my own -> visible");
        i.viewer_team = Some("blue");
        assert!(should_show_name(&i), "another team -> visible");
        i.viewer_team = Some("red");
        assert!(!should_show_name(&i), "my own team -> hidden");
    }

    #[test]
    fn can_see_friendly_invisibles_carries_only_the_hide_for_other_teams_arm() {
        // The asymmetry: `HIDE_FOR_OTHER_TEAMS` reads
        // `canSeeFriendlyInvisibles || isVisibleToPlayer`, and
        // `HIDE_FOR_OWN_TEAM` has no such escape.
        let mut i = mob();
        i.is_invisible = true;
        i.viewer_team = Some("red");

        i.team = Some(TeamView {
            name: "red",
            visibility: NameTagVisibility::HideForOtherTeams,
            see_friendly_invisibles: true,
        });
        assert!(should_show_name(&i), "same team + the flag -> visible");

        i.team = Some(TeamView {
            name: "red",
            visibility: NameTagVisibility::HideForOtherTeams,
            see_friendly_invisibles: false,
        });
        assert!(!should_show_name(&i), "without the flag, invisible wins");
    }

    #[test]
    fn can_see_friendly_invisibles_also_flows_through_is_invisible_to() {
        // Two paths read the flag: `isInvisibleTo` (which is why an ALWAYS
        // team shows an invisible team-mate) and the HIDE_FOR_OTHER_TEAMS arm
        // above. This is the first.
        let mut i = mob();
        i.is_invisible = true;
        i.viewer_team = Some("red");
        i.team = Some(TeamView {
            name: "red",
            visibility: NameTagVisibility::Always,
            see_friendly_invisibles: true,
        });
        assert!(should_show_name(&i));
        assert!(!is_invisible_to_viewer(&i));
    }

    #[test]
    fn the_local_player_skips_the_team_branch() {
        // `if (entity != player)` guards it, so the local player falls through
        // to the tail — where `is_camera_entity` then hides them.
        let mut i = mob();
        i.renderer = LabelRenderer::Avatar;
        i.is_local_player = true;
        i.is_camera_entity = true;
        i.team = Some(TeamView {
            name: "red",
            visibility: NameTagVisibility::Always,
            see_friendly_invisibles: false,
        });
        assert!(
            !should_show_name(&i),
            "a team must not rescue the local player from the camera test"
        );
    }

    #[test]
    fn an_un_named_mob_is_silent_but_still_has_a_health_bar() {
        // `MobRenderer`'s extra conjunct against the health bar's lack of one.
        let mut i = mob();
        i.entity_should_show_name = false;
        i.has_custom_name = false;
        assert!(!should_show_name(&i), "no name source -> no nametag");
        assert!(
            should_show_health_bar(&i),
            "a bar does not need a name to exist"
        );
    }

    #[test]
    fn a_named_but_not_visible_mob_needs_the_crosshair() {
        let mut i = mob();
        i.entity_should_show_name = false;
        i.has_custom_name = true;
        assert!(!should_show_name(&i), "not looking at it");
        i.is_crosshair_pick = true;
        assert!(should_show_name(&i), "looking at it");
    }

    #[test]
    fn a_player_is_named_without_a_custom_name() {
        // `Player.shouldShowName()` returns a literal `true`.
        let mut i = mob();
        i.renderer = LabelRenderer::Avatar;
        i.has_custom_name = false;
        i.entity_should_show_name = true;
        assert!(should_show_name(&i));
    }

    #[test]
    fn an_armour_stand_ignores_every_suppression_rule_for_its_name() {
        let mut i = mob();
        i.renderer = LabelRenderer::ArmorStand;
        i.is_invisible = true;
        i.is_discrete = true;
        i.is_vehicle = true;
        i.hud_hidden = true;
        i.distance_sq = 50.0 * 50.0; // past the sneak bound, inside 64
        assert!(
            should_show_name(&i),
            "ArmorStandRenderer overrides the lot with isCustomNameVisible()"
        );
        // But not the outer distance gate, which is in `extractNameTags`.
        i.distance_sq = 70.0 * 70.0;
        assert!(!should_show_name(&i));
    }

    #[test]
    fn an_armour_stands_health_bar_does_take_the_suppression_rules() {
        // The recorded divergence, asserted so it cannot drift silently.
        let mut i = mob();
        i.renderer = LabelRenderer::ArmorStand;
        i.is_invisible = true;
        assert!(should_show_name(&i), "vanilla names it anyway");
        assert!(!should_show_health_bar(&i), "the spec suppresses the bar");
    }

    #[test]
    fn a_non_living_entity_takes_the_base_rule_and_never_a_health_bar() {
        let mut i = mob();
        i.renderer = LabelRenderer::Other;
        i.is_invisible = true;
        i.is_vehicle = true;
        i.hud_hidden = true;
        assert!(
            should_show_name(&i),
            "EntityRenderer.shouldShowName reads only the name source"
        );
        assert!(!should_show_health_bar(&i), "nothing non-living has one");
    }

    #[test]
    fn a_suppressed_label_suppresses_both_for_every_mob_and_avatar() {
        // The agreement property the two features have to share. Exhaustive
        // over every boolean input for the two renderers that carry both
        // labels: whenever the nametag is shown, the bar is too.
        let mut shown = 0;
        for renderer in [LabelRenderer::Mob, LabelRenderer::Avatar] {
            for bits in 0u32..(1 << 11) {
                let team_kind = bits & 3;
                let i = LabelInputs {
                    renderer,
                    distance_sq: 4.0,
                    name_tag_distance: DEFAULT_NAME_TAG_DISTANCE,
                    is_discrete: bits & (1 << 3) != 0,
                    is_invisible: bits & (1 << 4) != 0,
                    is_vehicle: bits & (1 << 5) != 0,
                    is_camera_entity: bits & (1 << 6) != 0,
                    is_local_player: bits & (1 << 7) != 0,
                    hud_hidden: bits & (1 << 8) != 0,
                    viewer_spectator: bits & (1 << 9) != 0,
                    team: (team_kind > 0).then(|| TeamView {
                        name: "red",
                        visibility: match team_kind {
                            1 => NameTagVisibility::Always,
                            2 => NameTagVisibility::HideForOtherTeams,
                            _ => NameTagVisibility::HideForOwnTeam,
                        },
                        see_friendly_invisibles: bits & (1 << 2) != 0,
                    }),
                    viewer_team: (bits & (1 << 10) != 0).then_some("red"),
                    entity_should_show_name: true,
                    has_custom_name: true,
                    is_crosshair_pick: false,
                };
                if should_show_name(&i) {
                    shown += 1;
                    assert!(
                        should_show_health_bar(&i),
                        "{renderer:?} bits={bits}: a shown name must carry a bar"
                    );
                }
            }
        }
        // Not vacuous: the sweep has to actually reach the shown case, or the
        // implication above holds for free. (M59's `e3`/`e4` passed on two
        // empty vectors for exactly this reason.)
        assert!(shown > 0, "the sweep never showed a name");
    }
}
