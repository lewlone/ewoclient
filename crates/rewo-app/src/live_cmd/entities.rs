use super::*;

/// Velvet-tinted linear colors for the capsule set.
pub(super) fn linear_rgb(r: u8, g: u8, b: u8) -> [f32; 3] {
    [
        srgb_to_linear(r as f32 / 255.0),
        srgb_to_linear(g as f32 / 255.0),
        srgb_to_linear(b as f32 / 255.0),
    ]
}

/// Camera basis vectors for nametag billboards, from MC-convention angles.
pub(crate) fn camera_basis(yaw_deg: f32, pitch_deg: f32) -> ([f32; 3], [f32; 3]) {
    let yaw = yaw_deg.to_radians();
    let pitch = pitch_deg.to_radians();
    let dir = Vec3::new(
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    );
    let right = dir.cross(Vec3::Y).normalize_or_zero();
    let up = right.cross(dir).normalize_or_zero();
    (right.to_array(), up.to_array())
}

/// Which of a profile's textures a fetch job is for (M60).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum TexKind {
    Skin,
    Cape,
}

/// One resolved player's textures: the atlas UV offset relocating the
/// default player quads onto the uploaded skin slot, the arm model, and the
/// cape slot's atlas origin.
///
/// The two arrive independently — a profile may carry a cape and no skin —
/// so an entry exists as soon as *either* lands, with the other's field at
/// its no-texture default.
#[derive(Clone, Copy, Default)]
pub(crate) struct PlayerSkin {
    pub(super) uv: [f32; 2],
    pub(super) slim: bool,
    /// Atlas origin of this player's cape slot, if one was uploaded.
    pub(super) cape: Option<(u32, u32)>,
    /// This player's tab-list face slot, if one was uploaded (M155).
    pub(super) face: Option<u8>,
}

pub(crate) type SkinRegistry = std::collections::HashMap<u128, PlayerSkin>;

/// The local player's own textures, staged for the inventory preview's
/// **second** entity pass (M36 skin, M64 cape).
///
/// The raw pixels are kept alongside the addresses because the preview pass
/// is built lazily — the first time the screen opens — so a texture can
/// arrive before there is anywhere to put it. Each address is filled in on
/// the first frame the pass exists and never recomputed.
#[derive(Default)]
pub(crate) struct PreviewTextures {
    /// 64x64 RGBA skin, and whether the profile names the slim model.
    pub skin: Option<(Vec<u8>, bool)>,
    /// 64x32 RGBA cape sheet.
    pub cape: Option<Vec<u8>>,
    /// `EntityDraw::skin_uv` once uploaded into the preview's atlas.
    pub skin_uv: Option<[f32; 2]>,
    /// `CapeDraw::origin` once uploaded into the preview's atlas.
    pub cape_origin: Option<(u32, u32)>,
}

/// Async player-texture loader: a worker thread fetches + decodes skin and
/// cape PNGs off the render/tick path; the main loop uploads each result
/// into the entity atlas and records its slot. They arrive rarely (once per
/// player at join), so the per-texture `wait_idle` in the upload is cheap.
pub(crate) struct SkinLoader {
    pub(super) req_tx: std::sync::mpsc::Sender<(u128, TexKind, String, bool)>,
    pub(super) res_rx: std::sync::mpsc::Receiver<(u128, TexKind, bool, Vec<u8>)>,
    pub(super) requested: std::collections::HashSet<(u128, TexKind)>,
    pub(super) registry: SkinRegistry,
}

impl SkinLoader {
    pub(super) fn new() -> Self {
        let (req_tx, req_rx) = std::sync::mpsc::channel::<(u128, TexKind, String, bool)>();
        let (res_tx, res_rx) = std::sync::mpsc::channel::<(u128, TexKind, bool, Vec<u8>)>();
        std::thread::Builder::new()
            .name("rewo-skin-fetch".into())
            .spawn(move || {
                while let Ok((uuid, kind, url, slim)) = req_rx.recv() {
                    let got = match kind {
                        TexKind::Skin => crate::skin_fetch::fetch_rgba64(&url),
                        TexKind::Cape => crate::skin_fetch::fetch_cape_rgba(&url),
                    };
                    match got {
                        Ok(rgba) => {
                            if res_tx.send((uuid, kind, slim, rgba)).is_err() {
                                return;
                            }
                        }
                        Err(e) => log::warn!("skin: fetch {url} failed: {e}"),
                    }
                }
            })
            .ok();
        Self {
            req_tx,
            res_rx,
            requested: std::collections::HashSet::new(),
            registry: SkinRegistry::new(),
        }
    }

    /// Queue this profile's texture fetches (once per UUID per kind).
    pub(super) fn request(&mut self, uuid: u128, info: &rewo_net::skins::SkinInfo) {
        for (kind, url) in [
            (TexKind::Skin, info.url.as_ref()),
            (TexKind::Cape, info.cape.as_ref()),
        ] {
            let Some(url) = url else { continue };
            if self.requested.insert((uuid, kind)) {
                let _ = self.req_tx.send((uuid, kind, url.clone(), info.slim));
            }
        }
    }

    /// Upload any fetched textures into the atlas + record their slots.
    pub(super) fn poll_uploads(&mut self, gpu: &mut Gpu, wr: &mut WorldRenderer) {
        while let Ok((uuid, kind, slim, rgba)) = self.res_rx.try_recv() {
            match kind {
                TexKind::Skin => {
                    // M155 — crop the tab-list face while the full sheet is in
                    // hand. This is the ONE place it is free: the 64x64 is
                    // already decoded here and is dropped at the end of the
                    // arm, so a later face would have to re-fetch it.
                    //
                    // 8x8 head at (8,8) beside 8x8 hat at (40,8). **The hat's u
                    // is 40, not 32** — 32 is the hat cube's side face.
                    if rgba.len() >= 64 * 64 * 4 {
                        let mut strip = vec![0u8; 16 * 8 * 4];
                        for row in 0..8usize {
                            for (col_off, src_x) in [(0usize, 8usize), (8, 40)] {
                                let src = ((8 + row) * 64 + src_x) * 4;
                                let dst = (row * 16 + col_off) * 4;
                                strip[dst..dst + 32].copy_from_slice(&rgba[src..src + 32]);
                            }
                        }
                        if let Some(slot) = wr.upload_tab_face(gpu, &strip) {
                            self.registry.entry(uuid).or_default().face = Some(slot);
                        }
                    }
                    if let Some(uv) = wr.upload_player_skin(gpu, &rgba) {
                        let e = self.registry.entry(uuid).or_default();
                        e.uv = uv;
                        e.slim = slim;
                        log::info!(
                            "skin: uploaded for {uuid:032x} ({} model)",
                            if slim { "slim" } else { "wide" }
                        );
                    }
                }
                TexKind::Cape => {
                    if let Some(o) = wr.upload_player_cape(gpu, &rgba) {
                        self.registry.entry(uuid).or_default().cape = Some(o);
                        log::info!("cape: uploaded for {uuid:032x} at {o:?}");
                    }
                }
            }
        }
    }
}

/// Tracks when each entity's gesture-driving state changed. The wire
/// carries only the *current* pose/state — vanilla clients time the rigs
/// from the transition instant, so we record it here. Both a wall-clock
/// second (the rig's time base) and the network tick of the transition are
/// kept: the tick is what the entity-event ownership rules compare against
/// (vanilla orders `AnimationState.start`/`.stop` by tick — see
/// [`resolve_mob_anim`]).
#[derive(Clone, Copy)]
pub(super) struct GestureEntry {
    pub(super) gesture: rewo_gpu::mobs::Gesture,
    pub(super) start_seconds: f32,
    pub(super) start_tick: i64,
}

#[derive(Default)]
pub(crate) struct GestureTracker {
    pub(super) map: std::collections::HashMap<i32, GestureEntry>,
}

impl GestureTracker {
    /// Record `wanted` for this entity at `now` seconds / `tick`; returns the
    /// active gesture, its age in seconds, and the network tick it started on.
    /// `head_start` pre-advances a *newly entered* gesture's clock (vanilla's
    /// SCARED `fastForward`). A repeated *same* gesture keeps its original
    /// transition tick — so metadata that re-arrives without an actual pose
    /// change is not a new transition.
    pub(super) fn update(
        &mut self,
        id: i32,
        wanted: Option<rewo_gpu::mobs::Gesture>,
        now: f32,
        head_start: f32,
        tick: i64,
    ) -> Option<(rewo_gpu::mobs::Gesture, f32, i64)> {
        match wanted {
            None => {
                self.map.remove(&id);
                None
            }
            Some(g) => {
                let e = match self.map.get(&id) {
                    Some(e) if e.gesture == g => *e,
                    _ => {
                        let e = GestureEntry {
                            gesture: g,
                            start_seconds: now - head_start,
                            start_tick: tick,
                        };
                        self.map.insert(id, e);
                        e
                    }
                };
                Some((g, now - e.start_seconds, e.start_tick))
            }
        }
    }
}

/// Elapsed seconds since a one-shot event's receipt tick, using vanilla's
/// `ageInTicks = tickCount + partialTick` convention: `(now_tick − start +
/// partial) · 0.05`. `None` passes through (the event never fired). Clamped
/// non-negative for the degenerate same-tick-receipt case.
pub(super) fn event_age_seconds(start_tick: Option<i64>, tick: i64, alpha: f32) -> Option<f32> {
    start_tick.map(|s| ((((tick - s) as f32) + alpha) * 0.05).max(0.0))
}

/// Resolve a mob's per-frame rig inputs from its wire pose/state plus the
/// one-shot entity-event side-table, applying the exact vanilla ownership
/// rules. Shared by the live collector and the `eventshot` oracle so those
/// rules are exercised through production code, not a copy.
///
/// - **Warden id 4 (attack)** calls `roarAnimationState.stop()` and never
///   restarts the roar until a fresh ROARING pose transition. So the metadata
///   roar is suppressed iff the attack event's receipt tick is at/after the
///   roar's transition tick — vanilla's start/stop tick ordering. The attack
///   rig itself still plays (through the returned `events`).
/// - **Armadillo id 64 (peek)** stops and re-`startIfStopped`s the SCARED
///   peek — the SAME shared `peekAnimationState` as the metadata 2.5 s hold —
///   so it re-clocks the existing SCARED gesture from age 0 rather than adding
///   a second rig. Only when the event landed during this SCARED episode
///   (receipt tick at/after the SCARED transition), else a stale event from a
///   previous roll can't disturb the current hold.
///
/// Returns the gesture (post-ownership-rules) and the per-`ModelEvent` ages.
pub(crate) fn resolve_mob_anim(
    kind: EntityModelKind,
    pose: u8,
    state: u8,
    attack_tick: Option<i64>,
    sonic_tick: Option<i64>,
    peek_tick: Option<i64>,
    gestures: &mut GestureTracker,
    id: i32,
    now: f32,
    tick: i64,
    alpha: f32,
) -> (
    Option<(rewo_gpu::mobs::Gesture, f32)>,
    [Option<f32>; rewo_gpu::mobs::ModelEvent::COUNT],
) {
    use rewo_gpu::mobs::Gesture;
    let events = [
        event_age_seconds(attack_tick, tick, alpha),
        event_age_seconds(sonic_tick, tick, alpha),
    ];
    let wanted = wanted_gesture(kind, pose, state);
    // Entering SCARED starts the peek at its held ball pose — vanilla
    // `fastForward(SCARED.animationDuration())` = 2.5 s.
    let head_start = if wanted == Some(Gesture::ArmadilloScared) { 2.5 } else { 0.0 };
    let resolved = gestures.update(id, wanted, now, head_start, tick);
    let gesture = resolved.and_then(|(g, age, start_tick)| match g {
        // Attack stopped the roar durably (until a fresh ROARING transition).
        Gesture::WardenRoar if attack_tick.is_some_and(|a| a >= start_tick) => None,
        // Peek re-clocks the SCARED hold from age 0.
        Gesture::ArmadilloScared if peek_tick.is_some_and(|p| p >= start_tick) => {
            Some((g, event_age_seconds(peek_tick, tick, alpha).unwrap_or(0.0)))
        }
        _ => Some((g, age)),
    });
    (gesture, events)
}

/// Wire state → gesture for the rigged kinds. Pose ordinals are
/// `Pose.java`'s ids; state ordinals are the `Sniffer.State` /
/// `ArmadilloState` enum orders (metadata index 17).
pub(super) fn wanted_gesture(kind: EntityModelKind, pose: u8, state: u8) -> Option<rewo_gpu::mobs::Gesture> {
    use rewo_gpu::mobs::Gesture::*;
    Some(match kind {
        EntityModelKind::Warden => match pose {
            11 => WardenRoar,
            12 => WardenSniff,
            13 => WardenEmerge,
            14 => WardenDig,
            _ => return None,
        },
        EntityModelKind::Frog => match pose {
            8 => FrogCroak,
            9 => FrogTongue,
            _ => return None,
        },
        EntityModelKind::Breeze => match pose {
            6 => BreezeJump,
            15 => BreezeSlide,
            16 => BreezeShoot,
            17 => BreezeInhale,
            _ => return None,
        },
        EntityModelKind::Sniffer => match state {
            1 => SnifferFeelingHappy,
            2 => SnifferScenting,
            3 => SnifferSniffing,
            4 => SnifferSearching,
            5 => SnifferDigging,
            6 => SnifferRising,
            _ => return None,
        },
        EntityModelKind::Armadillo => match state {
            1 => ArmadilloRoll,
            2 => ArmadilloScared,
            3 => ArmadilloUnroll,
            _ => return None,
        },
        _ => return None,
    })
}

/// Resolve whether one entity shows a floating health bar, and with what
/// numbers — the production seam shared by the live collector and the
/// `healthbarshot` oracle (M59).
///
/// `REWO_HEALTH_BAR_SPEC.md` owns the bar's *appearance*; this owns the two
/// questions below the line where a vanilla oracle exists — may a floating
/// label appear at all, and is the denominator real?
///
/// The order is deliberate, and every step returns `None` rather than a
/// number:
///
/// 1. **Living only.** [`rewo_world::attributes::resolve`] answers `None` when
///    the entity type is unknown, is absent from `DefaultAttributes.SUPPLIERS`
///    (it is not a `LivingEntity`), or has no such attribute. A boat has no
///    max health, so a boat gets no bar — with no `matches!` list to keep in
///    sync.
/// 2. **Name-tag distance.** `EntityRenderer.extractNameTags` gates the whole
///    label on `distanceToCameraSq < Mth.square(nameTagDistance)`, and in 26.x
///    `nameTagDistance` is itself an attribute —
///    `entity.getAttribute(Attributes.NAME_TAG_DISTANCE).getValue()`, a
///    `RangedAttribute` defaulting to **64.0** over `[0, 512]`. So it resolves
///    through exactly the machinery above, modifiers and clamp included.
/// 3. **Invisible.** `LivingEntityRenderer.shouldShowName` ends in
///    `... && isVisibleToPlayer && ...`, where `isVisibleToPlayer` is
///    `!entity.isInvisibleTo(player)` and, with no teams and a non-spectating
///    viewer, that is `!entity.isInvisible()` — shared-flag **5**.
/// 4. **A synced max.** Spec rule 4: only a max health an `update_attributes`
///    actually established draws a bar. [`rewo_world::attributes::Source`]
///    exists precisely so this can be asked, and **there is no fallback to
///    20.0**: the supplier's default is a real number for every living entity,
///    which is what makes a wrong denominator so easy to draw confidently.
///    Rewo cannot tell "the server never sent health" from "this mob has 1 HP"
///    either — `DATA_HEALTH_ID` is seeded at `1.0F` — so a bar with an
///    unverified denominator would be a confident lie in both directions.
///
/// Deliberately **not** gated on (each a documented gap, not an oversight):
/// scoreboard team name-tag visibility and `canSeeFriendlyInvisibles` (Rewo
/// decodes no teams), `isDiscrete()`'s 32-block sneak cut-off, `isVehicle()`,
/// and `hud.isHidden()`. The local player never reaches here — it is not in
/// the entity table — which is also the spec's exclusion.
/// `CapeLayer.submit`'s four gates and `AvatarRenderer.extractCapeState`'s
/// three angles, resolved for one entity on one frame (M60).
///
/// Shared by the renderer and `capeshot` so the oracle cannot grade a second
/// copy of these rules — M41 and M45 both shipped gates that had quietly
/// stopped testing their subject exactly that way.
///
/// The gates, in vanilla's order:
///
/// 1. `!state.isInvisible && state.showCape` — shared flag 5, and bit 0 of
///    the index-16 customisation mask.
/// 2. `skin.cape() != null` — here, whether a cape sheet was uploaded for
///    this profile.
/// 3. `!hasLayer(chestEquipment, WINGS)` — an equipped **elytra** replaces
///    the cape outright.
/// 4. not a gate but the same block: `hasLayer(chestEquipment, HUMANOID)`
///    shifts the cape clear of a chestplate.
///
/// Gates 3 and 4 are separate questions about the same slot, which is why
/// [`rewo_data::equipment::ArmorLayer::Wings`] had to exist: a **carved
/// pumpkin** occupies the chest slot in neither sense — it names no
/// equipment asset at all — so it suppresses nothing and shifts nothing,
/// and before M60 it was indistinguishable from an elytra.
pub(crate) fn resolve_cape(
    ents: &rewo_world::entities::EntityTable,
    id: i32,
    kind: EntityModelKind,
    alpha: f32,
    cape_origin: Option<(u32, u32)>,
    items: &rewo_data::items::Items,
    equipment: &rewo_data::equipment::EquipmentAssets,
) -> Option<rewo_gpu::entities::CapeDraw> {
    use rewo_data::equipment::ArmorLayer;
    // `CapeLayer` is added by `AvatarRenderer` alone — a zombie has a torso
    // and no cape layer.
    if !rewo_gpu::mobs::wears_cape(kind) {
        return None;
    }
    // Gate 1.
    if ents.is_invisible(id) || !ents.shows_cape(id) {
        return None;
    }
    // Gate 2.
    let origin = cape_origin?;
    // Gates 3 + 4 — one lookup of the chest item, two different questions.
    let chest = ents.armor(id)[1].and_then(|p| items.name(p.item));
    if chest.is_some_and(|n| equipment.has_layer(n, ArmorLayer::Wings)) {
        return None;
    }
    let chest_humanoid = chest.is_some_and(|n| equipment.has_layer(n, ArmorLayer::Humanoid));

    let e = ents.get(id)?;
    let a = rewo_world::cape::cape_angles(
        e.cloak_pos(alpha),
        e.render_pos(alpha),
        e.yaw,
        e.fall_fly_ticks() as f32 + alpha,
        // `bob` and `walkDistance` are structurally zero for every entity
        // Rewo renders: only `LocalPlayer.move` ever advances `walkDist`,
        // and the local player is not in this table. Passing literal zeros
        // rather than modelling them is exact, not an approximation — see
        // `rewo_world::cape::cape_angles`, which explains why `bob` alone
        // would have been the wrong thing to key off.
        0.0,
        0.0,
    );
    Some(rewo_gpu::entities::CapeDraw {
        origin,
        flap: a.flap,
        lean: a.lean,
        lean2: a.lean2,
        chest_humanoid,
        // M61. `None` whenever the wavy cape is switched off — the table
        // holds no chain at all then — and the renderer falls through to the
        // vanilla rigid slab. The simulation is *read* here, never advanced:
        // this runs once per frame and `interpolated` takes `&self`.
        wavy: resolve_wavy_cape(ents, id, alpha),
    })
}

/// The frame's interpolated cape spine, or `None` for the vanilla cape.
pub(super) fn resolve_wavy_cape(
    ents: &rewo_world::entities::EntityTable,
    id: i32,
    alpha: f32,
) -> Option<rewo_gpu::entities::CapeJoints> {
    let sim = ents.wavy_cape(id)?;
    let mut buf = [[0.0f32; 3]; rewo_gpu::entities::CAPE_MAX_JOINTS];
    let n = sim.interpolated(alpha, &mut buf);
    rewo_gpu::entities::CapeJoints::from_slice(&buf[..n])
}

/// Everything the label predicate reads about the **viewer**, gathered once
/// per frame rather than once per entity (M70).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LabelViewer<'a> {
    /// `minecraft.getCameraEntity()`. Fed by `set_camera` since M74, so it
    /// really does diverge from `local_player` while spectating — before that
    /// it was hard-wired to the player with a note saying Rewo never detaches
    /// the camera. Vanilla reads the two in different clauses, which is why
    /// they were kept apart even while they could not differ.
    pub camera_entity: Option<i32>,
    /// `minecraft.player`.
    pub local_player: Option<i32>,
    /// `gui.hud.isHidden()` — F1.
    pub hud_hidden: bool,
    /// `player.isSpectator()`.
    pub spectator: bool,
    /// `minecraft.player.getTeam()`.
    pub team: Option<&'a str>,
    /// `entityRenderDispatcher.crosshairPickEntity` (M73) — the id
    /// [`resolve_crosshair_pick`] returned for this frame, or `None`.
    ///
    /// One value per frame rather than a per-entity question, because vanilla
    /// resolves it exactly once: `Minecraft.pick` runs one raycast and
    /// `EntityRenderDispatcher.prepare` hands the single result to every
    /// renderer. M70 fed this a hard `false`.
    pub crosshair_pick: Option<i32>,
}

impl<'a> LabelViewer<'a> {
    /// Read the viewer half out of the session once.
    ///
    /// `crosshair_pick` is left `None` here and filled by the caller, because
    /// it needs the frame's interpolation factor and the pick tables — see
    /// [`resolve_crosshair_pick`].
    pub(crate) fn from_session(session: &'a PlaySession, hud_hidden: bool) -> LabelViewer<'a> {
        LabelViewer {
            // M74: the server's `set_camera`, falling back to the local
            // player when it has never sent one — which is exactly
            // `Minecraft.cameraEntity`, a field initialised to the player and
            // only ever reassigned.
            camera_entity: session.client_state.camera_entity_or(session.player_id),
            local_player: session.player_id,
            hud_hidden,
            spectator: session.own_game_mode().is_some_and(|g| g.is_spectator()),
            team: session.own_team(),
            crosshair_pick: None,
        }
    }
}

/// Everything [`resolve_crosshair_pick`] needs that is not the entity table —
/// the version's static tables, gathered once so the seam takes one argument
/// rather than five.
#[derive(Clone, Copy)]
pub(crate) struct PickTables<'a> {
    pub types: &'a EntityTypes,
    pub classes: &'a rewo_data::entity_types::EntityClasses,
    pub shapes: &'a rewo_data::entity_pick::EntityPickTable,
    /// `EntityTypeTags.REDIRECTABLE_PROJECTILE`, the tag
    /// `Projectile.isPickable()` reads.
    pub redirectable: &'a rewo_data::entity_pick::EntityTypeTag,
    pub attributes: &'a rewo_data::attributes::AttributeRegistry,
}

/// `Minecraft.pick` → `crosshairPickEntity` (M73) — the production seam shared
/// by the live collector and the `labelshot` oracle.
///
/// Shared for the same reason as [`label_inputs_from_table`]: the gate has to
/// grade the resolution the client actually renders through. M41 and M45 both
/// shipped gates that quietly stopped testing their subject by reimplementing
/// a slice of the app's setup, and M73's whole output is one entity id — the
/// easiest possible thing for a parallel derivation to get subtly wrong.
///
/// The four per-entity predicates, and where each comes from:
///
/// * **`isPickable()`** — the type's [`rewo_data::entity_pick::PickRule`],
///   evaluated against the one live input each rule needs. `Alive` is
///   unconditionally true here because Rewo's table *deletes* a removed
///   entity, so `!isRemoved()` holds for every row by construction.
/// * **`getPickRadius()`** — `1.0` for a pickable `Projectile`, else `0.0`.
///   The rule already carries which is which.
/// * **`canBePickedFromInside()`** — `true` for everything Rewo models; the
///   only override is `SulfurCube` carrying a body item, whose flag Rewo does
///   not decode. A wrong answer there costs an inside-pick on one mob.
/// * **`getRootVehicle() == except.getRootVehicle()`** — walked through the
///   riding graph from both ends.
///
/// Two inputs Rewo cannot evaluate and answers the *permissive* way, which is
/// the opposite of the usual house rule and is deliberate:
/// `Player.isSpectator()` for a **remote** player and `ArmorStand.isMarker()`.
/// Both are metadata Rewo does not decode; suppressing on them would make
/// every player and every armour stand unpickable, which is a far larger error
/// than the one it avoids. The local player's own spectator state *is* known
/// and is not the question — you are never your own crosshair target, because
/// `getEntities(except, …)` excludes the camera entity.
pub(crate) fn resolve_crosshair_pick(
    session: &PlaySession,
    tables: PickTables<'_>,
    eye: [f64; 3],
    dir: [f64; 3],
    alpha: f32,
) -> Option<rewo_world::entity_pick::EntityHit> {
    crosshair_pick_from_table(
        &session.world.entities,
        session.player_id?,
        [session.player.x, session.player.y, session.player.z],
        session.local_attributes(),
        tables,
        eye,
        dir,
        alpha,
        // `cameraEntity.pick(maxDistance, partialTicks, false)`.
        &|from, d, reach| session.target_block(from, d, reach).map(|h| h.distance),
    )
}

/// `Entity.getRootVehicle()` — walk up `set_passengers`'s riding graph.
///
/// Read-only. The loop is bounded by a visit set because a malformed roster
/// could name a cycle, which vanilla's `while (result.isPassenger())` would
/// spin on forever.
pub(super) fn root_vehicle_of(ents: &rewo_world::entities::EntityTable, id: i32) -> i32 {
    let mut cur = id;
    let mut seen = std::collections::HashSet::new();
    while seen.insert(cur) {
        match ents.vehicle_of(cur) {
            Some(next) => cur = next,
            None => break,
        }
    }
    cur
}

/// The table-level half of [`resolve_crosshair_pick`], split out so a gate can
/// drive the real candidate construction without a live session (M73) — the
/// same split [`label_inputs_from_table`] has.
///
/// `block_ray` is `cameraEntity.pick(maxDistance, partialTicks, false)`: it
/// takes `(from, dir, reach)` and returns the distance to the block hit, or
/// `None` for a miss. A closure rather than a world reference so the oracle
/// can place a block at an exact distance and grade the reconciliation
/// directly.
#[allow(clippy::too_many_arguments)]
pub(crate) fn crosshair_pick_from_table(
    ents: &rewo_world::entities::EntityTable,
    camera: i32,
    camera_feet: [f64; 3],
    local_attributes: &rewo_world::attributes::EntityAttributes,
    tables: PickTables<'_>,
    eye: [f64; 3],
    dir: [f64; 3],
    alpha: f32,
    block_ray: &dyn Fn([f64; 3], [f64; 3], f64) -> Option<f64>,
) -> Option<rewo_world::entity_pick::EntityHit> {
    use rewo_data::entity_pick::PickRule;
    use rewo_world::entity_pick::{
        bounding_box, crosshair_pick, Candidate, DimensionInputs, InteractionRanges, PickInputs,
    };

    // Both ranges come from the camera entity's attributes. The local player
    // is not in the entity table (the server sends no `add_entity` for you),
    // so its snapshots are kept beside it — see `PlaySession::local_attributes`.
    let ranges = InteractionRanges::resolve(
        Some(local_attributes),
        Some("minecraft:player"),
        tables.attributes,
    );
    let camera_root = root_vehicle_of(ents, camera);
    let player_type = tables.types.id_of("minecraft:player");

    let mut candidates: Vec<Candidate> = Vec::new();
    for (id, e) in ents.iter() {
        if id == camera {
            continue; // `level.getEntities(except, …)`
        }
        let Some(shape) = tables.shapes.get(e.type_id) else {
            continue;
        };
        let projectile_pickable = tables.redirectable.contains(e.type_id);
        let pickable = match shape.rule {
            PickRule::Never => false,
            PickRule::Always => true,
            // `!isRemoved()`; a removed entity is not in this table.
            PickRule::Alive | PickRule::AliveUnlessSpectator | PickRule::AliveUnlessMarker => true,
            PickRule::RedirectableProjectile => projectile_pickable,
            // `super.isPickable() && !isInGround()`; the ground flag is not
            // decoded, so a landed arrow stays pickable.
            PickRule::RedirectableProjectileNotInGround => projectile_pickable,
        };
        let living = tables.classes.is_living(e.type_id);
        let scale = if living {
            rewo_world::attributes::resolve(
                ents.attributes(id),
                tables.types.name(e.type_id),
                "scale",
                tables.attributes,
            )
            .map_or(1.0, |(v, _)| v as f32)
        } else {
            1.0
        };
        let dims = DimensionInputs {
            width: shape.width,
            height: shape.height,
            living,
            avatar: Some(e.type_id) == player_type,
            pose: ents.pose(id),
            baby: ents.is_baby(id),
            scale,
        };
        candidates.push(Candidate {
            id,
            bb: bounding_box(e.render_pos(alpha), &dims),
            pickable,
            pick_radius: if matches!(
                shape.rule,
                PickRule::RedirectableProjectile | PickRule::RedirectableProjectileNotInGround
            ) && projectile_pickable
            {
                1.0
            } else {
                0.0
            },
            can_be_picked_from_inside: true,
            shares_root_vehicle: root_vehicle_of(ents, id) == camera_root,
        });
    }

    // The block half. `LocalPlayer.pick` casts the block ray to
    // `max(block, entity)`, **not** to the block range — a nearer mob has to
    // be able to shadow a block that is itself out of block reach.
    let block_hit = block_ray(eye, dir, ranges.max());
    // The camera entity's own box seeds the broad-phase search volume. The
    // local player is not in the table, so it is built from the player type's
    // own dimensions at the feet the physics tracks.
    let camera_bb = bounding_box(
        camera_feet,
        &DimensionInputs {
            width: 0.6,
            height: 1.8,
            living: true,
            avatar: true,
            pose: 0,
            baby: false,
            scale: 1.0,
        },
    );
    crosshair_pick(&PickInputs {
        eye,
        dir,
        camera_bb,
        ranges,
        block_hit_distance: block_hit,
        candidates: &candidates,
    })
}

/// Build one entity's [`rewo_world::label::LabelInputs`] — the production seam
/// shared by the live collector and the `labelshot` oracle (M70).
///
/// Shared for the same reason as [`resolve_attack_anim`]: the gate has to prove
/// the mapping the client actually renders through. M45 and M41 both shipped
/// gates that quietly stopped testing their subject by reimplementing a slice
/// of the app's setup.
///
/// **Renderer selection.** Vanilla picks the `shouldShowName` override by
/// renderer class. Rewo has no renderer registry, so: the player type maps to
/// `Avatar`, anything `rewo_world::attributes::resolve` can answer
/// `name_tag_distance` for maps to `Mob`, and everything else to `Other`. That
/// middle equivalence is `DefaultAttributes.SUPPLIERS`, which is keyed by
/// `EntityType<? extends LivingEntity>` — so having a supplier is having a
/// `LivingEntity` renderer. `LabelRenderer::ArmorStand` is transcribed and
/// unit-tested but **never selected here**, because Rewo models no armour
/// stand (`mobs.rs` renders it as a capsule); it exists so the ladder is
/// complete when one lands.
pub(crate) fn resolve_label_inputs<'a>(
    session: &'a PlaySession,
    id: i32,
    entity_name: Option<&str>,
    attr_reg: Option<&rewo_data::attributes::AttributeRegistry>,
    is_player: bool,
    distance_sq: f64,
    viewer: &LabelViewer<'a>,
) -> rewo_world::label::LabelInputs<'a> {
    label_inputs_from_table(
        &session.world.entities,
        id,
        entity_name,
        attr_reg,
        is_player,
        distance_sq,
        viewer,
        session.label_team_of(id),
    )
}

/// The table-level half of [`resolve_label_inputs`], split out so a gate can
/// drive the real input resolution without a live session (M70).
///
/// The caller supplies the entity's team, because that is the one input that
/// needs the scoreboard rather than the entity table.
#[allow(clippy::too_many_arguments)]
pub(crate) fn label_inputs_from_table<'a>(
    ents: &rewo_world::entities::EntityTable,
    id: i32,
    entity_name: Option<&str>,
    attr_reg: Option<&rewo_data::attributes::AttributeRegistry>,
    is_player: bool,
    distance_sq: f64,
    viewer: &LabelViewer<'a>,
    team: Option<rewo_world::label::TeamView<'a>>,
) -> rewo_world::label::LabelInputs<'a> {
    use rewo_world::label::{LabelInputs, LabelRenderer, DEFAULT_NAME_TAG_DISTANCE};
    // `LivingEntityRenderer.extractNameTags` reads
    // `Attributes.NAME_TAG_DISTANCE`; the base passes a literal 64.0. Resolving
    // it is also the living test, so the two answers come from one lookup.
    let resolved = attr_reg.and_then(|reg| {
        rewo_world::attributes::resolve(ents.attributes(id), entity_name, "name_tag_distance", reg)
    });
    let renderer = if is_player {
        LabelRenderer::Avatar
    } else if resolved.is_some() {
        LabelRenderer::Mob
    } else {
        LabelRenderer::Other
    };
    LabelInputs {
        renderer,
        distance_sq,
        name_tag_distance: resolved.map_or(DEFAULT_NAME_TAG_DISTANCE, |(d, _)| d),
        is_discrete: ents.is_discrete(id),
        is_invisible: ents.is_invisible(id),
        is_vehicle: ents.is_vehicle(id),
        is_camera_entity: viewer.camera_entity == Some(id),
        is_local_player: viewer.local_player == Some(id),
        hud_hidden: viewer.hud_hidden,
        viewer_spectator: viewer.spectator,
        team,
        viewer_team: viewer.team,
        // `entity.shouldShowName()` — `Player` overrides it to a literal
        // `true`; everything else inherits `isCustomNameVisible()`.
        entity_should_show_name: is_player || ents.is_custom_name_visible(id),
        has_custom_name: ents.custom_name(id).is_some(),
        // `entity == entityRenderDispatcher.crosshairPickEntity` (M73). M70
        // fed this a hard `false` because Rewo's raycast was voxel-only; the
        // frame's single pick now answers it — see `resolve_crosshair_pick`.
        is_crosshair_pick: viewer.crosshair_pick == Some(id),
    }
}

/// Both of an `EntityDraw`'s label fields, resolved together from one
/// predicate — the production seam shared by the live collector and the
/// `labelshot` oracle (M70).
///
/// This exists so "the nametag and the health bar agree" is a property of one
/// function rather than of two call sites that happen to line up. Before M70
/// they did not: the bar had a three-gate subset of `shouldShowName` and the
/// tag had none at all, so an invisible named mob showed a name and no bar.
///
/// `name` is the candidate string the caller already chose — a player's
/// profile name or anything else's metadata custom name. Whether it is *drawn*
/// is the predicate's answer, not the mere existence of the string.
pub(crate) fn resolve_labels<'a>(
    ents: &rewo_world::entities::EntityTable,
    id: i32,
    entity_name: Option<&str>,
    attr_reg: Option<&rewo_data::attributes::AttributeRegistry>,
    label: &rewo_world::label::LabelInputs<'_>,
    name: Option<&'a str>,
) -> (Option<&'a str>, Option<rewo_gpu::entities::HealthBar>) {
    let shown = rewo_world::label::should_show_name(label).then_some(name).flatten();
    // `None` without an attribute registry, which is the same fail-closed
    // answer the resolver gives for an unsynced max — a bar is never drawn on
    // a guess.
    let bar = attr_reg.and_then(|reg| resolve_health_bar(ents, id, entity_name, reg, label));
    (shown, bar)
}

/// `REWO_HEALTH_BAR_SPEC.md` rules 4 and 5 — whether this entity gets a bar.
///
/// **M70 moved rule 5 out of here.** It used to be three hand-rolled gates
/// (living, name-tag distance, invisible) that were a strict subset of what
/// suppresses a nametag, and the nametag path had a *different* subset — namely
/// none. Both now go through [`rewo_world::label`], so "suppressed by
/// everything that suppresses a nametag" is true by construction rather than by
/// two lists happening to agree.
///
/// What stays here is rule 4, which is not a visibility question: a bar needs a
/// max health an `update_attributes` actually established. `Source::Default` is
/// rejected even though the supplier's 20.0 is a real number for every living
/// entity, because Rewo cannot tell "the server never sent health" from "this
/// mob has 1 HP" — `DATA_HEALTH_ID` is seeded at `1.0F` — so a bar with an
/// unverified denominator would be a confident lie in both directions.
pub(crate) fn resolve_health_bar(
    ents: &rewo_world::entities::EntityTable,
    id: i32,
    entity_name: Option<&str>,
    reg: &rewo_data::attributes::AttributeRegistry,
    label: &rewo_world::label::LabelInputs<'_>,
) -> Option<rewo_gpu::entities::HealthBar> {
    use rewo_world::attributes::{resolve, Source};
    // Rule 5, in one call, shared with the nametag.
    if !rewo_world::label::should_show_health_bar(label) {
        return None;
    }
    // Rule 4.
    let (max, source) = resolve(ents.attributes(id), entity_name, "max_health", reg)?;
    if source != Source::Synced {
        return None;
    }
    Some(rewo_gpu::entities::HealthBar {
        current: ents.death_state(id).health,
        max: max as f32,
    })
}

/// Resolve an entity's Allay dance render inputs — the production seam shared by
/// the live collector and the `danceshot` oracle, so the kind gate + the
/// counter → `(is_spinning, spinning_progress)` → [`rewo_gpu::mobs::AllayDance`]
/// mapping are the same code the gate proves. `Some` only for an Allay-kind
/// entity that is currently dancing; every other kind is inert here even if the
/// entity somehow carried a dance clock.
pub(crate) fn resolve_allay_dance(
    kind: EntityModelKind,
    entities: &rewo_world::entities::EntityTable,
    id: i32,
    alpha: f32,
) -> Option<rewo_gpu::mobs::AllayDance> {
    (kind == EntityModelKind::Allay)
        .then(|| entities.allay_dance_render(id, alpha))
        .flatten()
        .map(|(is_spinning, spinning_progress)| rewo_gpu::mobs::AllayDance {
            is_spinning,
            spinning_progress,
        })
}

/// Resolve an entity's combat-swing render inputs — the production seam shared
/// by the live collector and the `swingshot` oracle, so the
/// `ArmedEntityRenderState` extraction (`attackTime` / `attackArm` /
/// `swingAnimationType` / `ageScale`) is the same code the gate proves.
///
/// Deliberately **not** kind-gated: `extractArmedEntityRenderState` runs for
/// every armed entity, and the value also feeds CEM's `swing_progress`. Only a
/// model built from `HumanoidModel.createMesh` carries parts that pose from it,
/// so a mob simply ignores a non-zero `attackTime` (witnessed in the gate).
pub(crate) fn resolve_attack_anim(
    entities: &rewo_world::entities::EntityTable,
    id: i32,
    alpha: f32,
) -> rewo_gpu::mobs::SwingPose {
    use rewo_data::swing_anim::SwingAnimationType;
    use rewo_gpu::mobs::{SwingKind, SwingPose};
    use rewo_world::entities::HumanoidArm;
    // An input that could not be resolved exactly suppresses the whole pose —
    // `attack_time` 0 short-circuits `setupAttackAnimation` and publishes 0 to
    // CEM's `swing_progress`, which is the honest answer when the held item is
    // unknowable. A later exact equipment update lifts it.
    let Some(kind) = entities.swing_animation_type(id) else {
        return SwingPose {
            inputs_known: false,
            ..SwingPose::NONE
        };
    };
    if !entities.swing_inputs_known(id) {
        return SwingPose {
            inputs_known: false,
            ..SwingPose::NONE
        };
    }
    SwingPose {
        attack_time: entities.attack_anim(id, alpha),
        left_arm: entities.attack_arm(id) == HumanoidArm::Left,
        kind: match kind {
            SwingAnimationType::None => SwingKind::None,
            SwingAnimationType::Whack => SwingKind::Whack,
            SwingAnimationType::Stab => SwingKind::Stab,
        },
        // `LivingEntity.getAgeScale()` — `isBaby() ? 0.5 : 1.0`.
        age_scale: if entities.is_baby(id) { 0.5 } else { 1.0 },
        inputs_known: true,
    }
}

/// `AvatarRenderer.getArmPose` / `HumanoidMobRenderer.getArmPose` for both
/// arms, plus every `HumanoidRenderState` field the pose dispatch reads — the
/// *hold* baseline applied before `setupAttackAnimation`.
///
/// Shared by the live collector and the `swingshot` oracle for the same reason
/// as [`resolve_attack_anim`]: the gate must prove the mapping the client
/// actually renders through, not a parallel copy of it.
///
/// **Two different functions, selected by renderer.** This was collapsed to one
/// through M22 and is split here, because the two disagree on the common case:
///
/// - `AvatarRenderer.getArmPose` (players) runs the full eleven-pose ladder and
///   falls through to `ITEM`.
/// - `HumanoidMobRenderer.getArmPose` (every humanoid mob) checks only
///   STAB-while-swinging and the `minecraft:spears` tag, and otherwise returns
///   **`EMPTY`** — so an armed zombie's arm hangs at its walk pose, not 18°
///   higher. Subclasses layer on top: skeletons add `BOW_AND_ARROW`, the
///   drowned adds `THROW_TRIDENT`.
///
/// **Vanilla computes a pose per *hand*, then selects by arm.** Resolving
/// directly per arm is *not* the same function once two-handed poses exist:
/// `if (mainHandPose.isTwoHanded()) offHandPose = offHandItem.isEmpty() ? EMPTY
/// : ITEM` rewrites the off-hand pose from the main hand's, which has no
/// per-arm expression. So the per-hand shape is transcribed literally.
pub(crate) fn resolve_arm_poses(
    entities: &rewo_world::entities::EntityTable,
    id: i32,
    kind: rewo_gpu::mobs::EntityModelKind,
    spears: &rewo_data::item_tags::ItemTag,
    bow_item: Option<i32>,
    crossbow_item: Option<i32>,
) -> rewo_gpu::mobs::ArmPoses {
    use rewo_data::swing_anim::SwingAnimationType;
    use rewo_data::use_item::ItemUseAnimation as A;
    use rewo_gpu::mobs::{ArmPose, ArmPoses, EntityModelKind as K};
    use rewo_world::entities::{HandItem, HumanoidArm, InteractionHand};

    let swinging = entities.is_swinging(id);
    let is_avatar = matches!(kind, K::Player | K::PlayerSlim);
    let use_state = entities.use_state(id);
    let mut known = true;

    // `AvatarRenderer.getArmPose(avatar, itemInHand, hand)` — the per-hand
    // ladder, in vanilla's order. The order is load-bearing: the
    // charged-crossbow hold is tested *before* the use gate, so a crossbow that
    // is already charged holds rather than charges.
    let avatar_pose = |hand: InteractionHand, known: &mut bool| -> ArmPose {
        let held = match entities.hand_item(id, hand) {
            HandItem::Empty => return ArmPose::Empty,
            HandItem::Unknown => {
                *known = false;
                return ArmPose::Empty;
            }
            HandItem::Held(h) => h,
        };
        if !swinging && Some(held.item_id) == crossbow_item && held.charged {
            return ArmPose::CrossbowHold;
        }
        if use_state.poses_hand(hand) {
            // `switch (itemInHand.getUseAnimation())`. EAT, DRINK, BUNDLE and
            // NONE have no case, so they fall out of the switch and continue to
            // the STAB / spear-tag tail below — they are not poses.
            match held.use_profile.animation {
                A::Block => return ArmPose::Block,
                A::Bow => return ArmPose::BowAndArrow,
                A::Trident => return ArmPose::ThrowTrident,
                A::Crossbow => return ArmPose::CrossbowCharge,
                A::Spyglass => return ArmPose::Spyglass,
                A::TootHorn => return ArmPose::TootHorn,
                A::Brush => return ArmPose::Brush,
                A::Spear => return ArmPose::Spear,
                A::None | A::Eat | A::Drink | A::Bundle => {}
            }
        }
        if held.swing.kind == SwingAnimationType::Stab && swinging {
            ArmPose::Spear
        } else if spears.contains(held.item_id) {
            ArmPose::Spear
        } else {
            ArmPose::Item
        }
    };

    // `HumanoidMobRenderer.getArmPose(mob, arm)` plus the two subclass
    // overrides Rewo's kinds can reach. Takes an *arm*, not a hand: the mob
    // path never rewrites the off-hand pose, so there is nothing to express
    // per-hand.
    let mob_pose = |arm: HumanoidArm, known: &mut bool| -> ArmPose {
        // `AbstractSkeletonRenderer`: main arm && isAggressive && main hand is
        // a bow. Checked first because it short-circuits `super.getArmPose`.
        let skeletal = matches!(
            kind,
            K::Skeleton | K::Stray | K::Bogged | K::WitherSkeleton | K::Parched
        );
        if skeletal
            && entities.main_arm(id) == arm
            && entities.mob_state(id).is_aggressive()
            && entities
                .hand_item(id, InteractionHand::MainHand)
                .held()
                .is_some_and(|h| Some(h.item_id) == bow_item)
        {
            return ArmPose::BowAndArrow;
        }
        let held = match entities.item_by_arm(id, arm) {
            HandItem::Empty => return ArmPose::Empty,
            HandItem::Unknown => {
                *known = false;
                return ArmPose::Empty;
            }
            HandItem::Held(h) => h,
        };
        // `DrownedRenderer`: main arm && isAggressive && holding a trident.
        // Reached through the trident's own use animation rather than an item
        // id, which is exact — `TridentItem` is the only thing that answers
        // `ItemUseAnimation.TRIDENT`.
        if kind == K::Drowned
            && entities.main_arm(id) == arm
            && entities.mob_state(id).is_aggressive()
            && held.use_profile.animation == A::Trident
        {
            return ArmPose::ThrowTrident;
        }
        if held.swing.kind == SwingAnimationType::Stab && swinging {
            ArmPose::Spear
        } else if spears.contains(held.item_id) {
            ArmPose::Spear
        } else {
            // The mob path's fall-through is EMPTY, not ITEM.
            ArmPose::Empty
        }
    };

    let (right, left) = if is_avatar {
        let main = avatar_pose(InteractionHand::MainHand, &mut known);
        let mut off = avatar_pose(InteractionHand::OffHand, &mut known);
        // `if (mainHandPose.isTwoHanded()) offHandPose = offHandItem.isEmpty()
        //      ? EMPTY : ITEM;`
        if main.is_two_handed() {
            off = match entities.hand_item(id, InteractionHand::OffHand) {
                HandItem::Empty => ArmPose::Empty,
                _ => ArmPose::Item,
            };
        }
        // `return avatar.getMainArm() == arm ? mainHandPose : offHandPose;`
        if entities.main_arm(id) == HumanoidArm::Right {
            (main, off)
        } else {
            (off, main)
        }
    } else {
        (
            mob_pose(HumanoidArm::Right, &mut known),
            mob_pose(HumanoidArm::Left, &mut known),
        )
    };

    // `HumanoidMobRenderer.extractHumanoidRenderState` — a static helper, so
    // players carry these too (`AvatarRenderer:168` calls it).
    let charging = right == ArmPose::CrossbowCharge || left == ArmPose::CrossbowCharge;
    ArmPoses {
        right,
        left,
        right_handed: entities.main_arm(id) == HumanoidArm::Right,
        known,
        using_item: use_state.using,
        main_hand_used: use_state.hand == InteractionHand::MainHand,
        ticks_using_item: use_state.ticks_using_item_partial(0.0),
        // `CrossbowItem.getChargeDuration(entity.getUseItem(), entity)`. The
        // helper computes it unconditionally, but only the CROSSBOW_CHARGE pose
        // reads it, and an enchanted crossbow never gets this far.
        max_crossbow_charge: if charging {
            ArmPoses::CROSSBOW_CHARGE_DURATION
        } else {
            0.0
        },
    }
}

/// The synced mob state the M20 arm rigs read, plus the derived
/// `IllagerArmPose` — the client-side half of `getArmPose()` for each illager
/// class, which vanilla computes per subclass rather than syncing.
///
/// Shared by the live collector and the `swingshot` oracle for the same reason
/// as [`resolve_attack_anim`] and [`resolve_arm_poses`].
///
/// `bow_item` is `Items.BOW`'s protocol id; `None` (a client that could not
/// resolve it) leaves `holding_bow` false, which keeps the skeleton attack rig
/// *enabled* — the conservative direction, since suppressing it would hide a
/// real animation rather than show a wrong one.
pub(crate) fn resolve_mob_combat(
    entities: &rewo_world::entities::EntityTable,
    id: i32,
    kind: rewo_gpu::mobs::EntityModelKind,
    bow_item: Option<i32>,
) -> rewo_gpu::mobs::MobCombat {
    use rewo_gpu::mobs::{EntityModelKind as K, IllagerArmPose as P, MobCombat};
    use rewo_world::entities::{HandItem, HumanoidArm, InteractionHand};

    let st = entities.mob_state(id);
    let main = entities.hand_item(id, InteractionHand::MainHand);
    let main_hand_empty = matches!(main, HandItem::Empty);
    // `entity.getMainHandItem().is(Items.BOW)`.
    let holding_bow = main
        .held()
        .zip(bow_item)
        .is_some_and(|(i, bow)| i.item_id == bow);
    // `AbstractIllager.getArmPose()`, per subclass. The base class answers
    // CROSSED, which is also what a non-illager gets (and never reads).
    let illager_pose = match kind {
        // `Pillager`: charging → CROSSBOW_CHARGE; holding a crossbow →
        // CROSSBOW_HOLD; else aggressive ? ATTACKING : NEUTRAL.
        K::Pillager => {
            if st.charging_crossbow {
                P::CrossbowCharge
            } else if main
                .held()
                .is_some_and(|i| Some(i.item_id) == crossbow_item_id())
            {
                P::CrossbowHold
            } else if st.is_aggressive() {
                P::Attacking
            } else {
                P::Neutral
            }
        }
        // `Vindicator`: aggressive → ATTACKING; else celebrating ?
        // CELEBRATING : CROSSED.
        K::Vindicator => {
            if st.is_aggressive() {
                P::Attacking
            } else if st.celebrating {
                P::Celebrating
            } else {
                P::Crossed
            }
        }
        // `SpellcasterIllager` (Evoker): casting → SPELLCASTING; else
        // celebrating ? CELEBRATING : CROSSED.
        K::Evoker => {
            if st.is_casting_spell() {
                P::Spellcasting
            } else if st.celebrating {
                P::Celebrating
            } else {
                P::Crossed
            }
        }
        // `Illusioner` overrides it: casting → SPELLCASTING; else aggressive ?
        // BOW_AND_ARROW : CROSSED. Note it never celebrates.
        K::Illusioner => {
            if st.is_casting_spell() {
                P::Spellcasting
            } else if st.is_aggressive() {
                P::BowAndArrow
            } else {
                P::Crossed
            }
        }
        _ => P::Crossed,
    };
    MobCombat {
        aggressive: st.is_aggressive(),
        main_hand_empty,
        holding_bow,
        is_baby: entities.is_baby(id),
        main_arm_left: entities.main_arm(id) == HumanoidArm::Left,
        illager_pose,
    }
}

/// `Items.CROSSBOW`'s protocol id, resolved once. The pillager pose test is
/// `isHolding(Items.CROSSBOW)`, an item identity check like the skeleton's bow.
pub(super) fn crossbow_item_id() -> Option<i32> {
    CROSSBOW_ITEM.get().copied().flatten()
}

/// Set once at session setup; `None` until then (and on a client that cannot
/// resolve the item), which makes the CROSSBOW_HOLD arm unreachable rather
/// than guessed.
pub(crate) static CROSSBOW_ITEM: std::sync::OnceLock<Option<i32>> = std::sync::OnceLock::new();

/// The crosshair pick's two version tables (M73), resolved once at session
/// setup — per-type bounding-box dimensions with `isPickable()`, and the
/// `redirectable_projectile` tag that rule reads.
///
/// Statics for the same reason [`CROSSBOW_ITEM`] is one: both `collect_entities`
/// call sites need them and neither owns the loader. `None` until set, which
/// makes the pick return `None` rather than pick with a guessed hitbox.
pub(crate) static PICK_SHAPES: std::sync::OnceLock<rewo_data::entity_pick::EntityPickTable> =
    std::sync::OnceLock::new();
pub(crate) static REDIRECTABLE: std::sync::OnceLock<rewo_data::entity_pick::EntityTypeTag> =
    std::sync::OnceLock::new();

/// The frame's `crosshairPickEntity`, resolved from the session and the two
/// statics above — the one call the render path makes.
///
/// `None` whenever any input is missing (no attribute registry, no entity
/// classes, tables unset), which is the same fail-closed answer M70 shipped:
/// a name-tagged mob whose `CustomNameVisible` is unset simply stays silent.
pub(crate) fn frame_crosshair_pick(
    session: &PlaySession,
    etypes: &EntityTypes,
    alpha: f32,
) -> Option<i32> {
    let tables = PickTables {
        types: etypes,
        classes: session.entity_classes.as_deref()?,
        shapes: PICK_SHAPES.get()?,
        redirectable: REDIRECTABLE.get()?,
        attributes: session.attribute_registry.as_deref()?,
    };
    let eye = eye_f64(session);
    let dir = look_dir(session.player.yaw, session.player.pitch);
    resolve_crosshair_pick(session, tables, eye, dir, alpha).map(|h| h.id)
}

/// Convert the baked held-item models across the `rewo-data` → `rewo-gpu`
/// seam (M22). The two shapes are deliberately identical so this stays
/// mechanical; `rewo-gpu` keeps no `rewo-data` dependency, the same rule
/// `SwingKind` / `MobCombat` already follow.
pub(crate) fn to_gpu_held_items(src: &rewo_data::held_items::HeldItems) -> rewo_gpu::held::HeldItems {
    use rewo_gpu::held as g;
    let conv_t = |t: &rewo_data::item_models::DisplayTransform| g::DisplayTransform {
        rotation: t.rotation,
        translation: t.translation,
        scale: t.scale,
    };
    let conv_quads = |qs: &[rewo_data::held_items::HeldQuad]| -> Vec<g::HeldQuad> {
        qs.iter()
            .map(|q| g::HeldQuad {
                verts: q.verts,
                uv: q.uv,
                tex: q.tex,
                part: q.part,
                dir: q.dir,
            })
            .collect()
    };
    let conv_models = |m: &std::collections::HashMap<
        String,
        rewo_data::held_items::HeldItemModel,
    >| {
        m.iter()
            .map(|(k, m)| {
                (
                    k.clone(),
                    g::HeldItemModel {
                        quads: conv_quads(&m.quads),
                        right: conv_t(&m.right),
                        left: conv_t(&m.left),
                        ground: conv_t(&m.ground),
                        gui: conv_t(&m.gui),
                        first_right: conv_t(&m.first_right),
                        first_left: conv_t(&m.first_left),
                        from_block: m.from_block,
                        gui_quads: m.gui_quads.as_deref().map(conv_quads),
                    },
                )
            })
            .collect()
    };
    rewo_gpu::held::HeldItems {
        models: conv_models(&src.models),
        block_entities: conv_models(&src.block_entities),
        textures: src
            .textures
            .iter()
            .map(|t| g::HeldTexture {
                w: t.w,
                h: t.h,
                rgba: t.rgba.clone(),
            })
            .collect(),
    }
}

/// The item held in each arm (M22), as registry names — `[right, left]`.
///
/// `ArmedEntityRenderState` carries the stacks per *arm*, and the hand-to-arm
/// mapping is `getMainArm()`, which
/// [`rewo_world::entities::EntityTable::item_by_arm`] already implements. An
/// `Unknown` hand yields `None` for the same reason its swing is suppressed:
/// the client cannot know what it is holding, and drawing a guess is worse
/// than drawing nothing.
pub(crate) fn resolve_held_items<'a>(
    entities: &rewo_world::entities::EntityTable,
    id: i32,
    items: &'a rewo_data::items::Items,
) -> [Option<&'a str>; 2] {
    use rewo_world::entities::HumanoidArm;
    let mut out: [Option<&str>; 2] = [None, None];
    for (i, arm) in [(0usize, HumanoidArm::Right), (1usize, HumanoidArm::Left)] {
        if let Some(held) = entities.item_by_arm(id, arm).held() {
            out[i] = items.name(held.item_id);
        }
    }
    out
}

/// Snapshot every tracked entity into this frame's draw list. `alpha` is
/// the partial-tick blend (0..1). Players get the rose capsule + nametag;
/// everything else gets mauve, sized by the type table. `now` is the
/// render clock in seconds — the gesture rigs' time base.
/// `ItemEntity.bobOffs` — `this.random.nextFloat() * (float)Math.PI * 2`.
///
/// Vanilla rolls this in the entity's constructor from a non-deterministic
/// source and never transmits it, so there is no server value to reproduce and
/// two vanilla clients watching the same dropped stack disagree about its bob
/// phase. Deriving it from the entity id is therefore *as vanilla as vanilla*
/// — it is a valid roll — with the added property of being stable across
/// frames and reproducible in a gate.
///
/// The hash is the 64-bit splitmix64 finalizer, used only to decorrelate
/// consecutive entity ids; nothing depends on its exact value.
/// `ItemStack.hasFoil()` for what an entity holds in one hand (M45).
///
/// The flag comes off the wire with the equipment, because it lives only in
/// the component patch — there is nothing about the item *id* that says
/// whether a particular stack is enchanted.
/// The armour atlas key each slot wears, head first (M46).
///
/// Two lookups, and both are **prototype** data rather than wire data: the
/// item's `Equippable.assetId()` comes from the generated item table, and the
/// asset's layer names from the jar. The wire sends only an item id.
///
/// The **leggings take the inner sheet** and the other three the outer one —
/// `usesInnerModel` is `slot == LEGS`, which is the whole reason two layer
/// types exist.
/// What each armour slot draws, head first — the atlas key and tint of every
/// sub-layer that survives `getColorForLayer` (M46, dyed in M47).
/// The sprite path one worn piece's trim resolves to, or `None` if the piece
/// carries no trim (M48).
///
/// `ArmorTrim.layerAssetId`, with the two halves looked up in the registries
/// the server synced:
///
/// ```java
/// MaterialAssetGroup.AssetInfo materialAsset = material.assets().assetId(equipmentAsset);
/// return pattern.assetId().withPath(p -> layerAssetPrefix + "/" + p + "_" + materialAsset.suffix());
/// ```
///
/// The `assetId(equipmentAsset)` step is what stops an iron trim on iron
/// armour vanishing: the material's `override_armor_assets` sends that pairing
/// to `iron_darker`.
pub(crate) fn trim_sprite_path(
    session: &PlaySession,
    piece: &rewo_world::entities::WornPiece,
    equipment_asset: &str,
    layer: rewo_data::equipment::ArmorLayer,
) -> Option<String> {
    let (material_id, pattern_id) = piece.trim?;
    let material = session.trim_materials.get(material_id as usize)?;
    let pattern = session.trim_patterns.get(pattern_id as usize)?;
    let suffix = material.suffix_for(equipment_asset);
    // `LayerType.trimAssetPrefix()` — `"trims/entity/" + this.id`.
    let prefix = format!("trims/entity/{}", layer.dir());
    Some(rewo_net::trim_parse::layer_asset_path(
        &pattern.asset_id,
        &prefix,
        suffix,
    ))
}

/// Permute and upload every trim sprite this frame needs, returning where each
/// one landed (M48).
///
/// A pre-pass because the upload needs `&mut` on the renderer while
/// [`collect_entities`] hands out borrows of the session. `upload_trim` caches
/// by path, so the second frame — and every frame after — does no work beyond
/// the lookups.
pub(crate) fn ensure_trims(
    session: &PlaySession,
    items: &rewo_data::items::Items,
    trims: &rewo_data::equipment::TrimAssets,
    gpu: &mut Gpu,
    wr: &mut WorldRenderer,
) -> std::collections::HashMap<String, (u32, u32)> {
    use rewo_data::equipment::ArmorLayer;
    let mut out = std::collections::HashMap::new();
    if trims.is_empty() {
        return out;
    }
    for (id, _) in session.world.entities.iter() {
        let worn = session.world.entities.armor(id);
        for (i, piece) in worn.iter().enumerate() {
            let Some(piece) = piece else { continue };
            if piece.trim.is_none() {
                continue;
            }
            let Some(asset) = items
                .name(piece.item)
                .and_then(rewo_data::item_props_table::equip_asset)
            else {
                continue;
            };
            let layer = if i == 2 {
                ArmorLayer::Leggings
            } else {
                ArmorLayer::Humanoid
            };
            let Some(path) = trim_sprite_path(session, piece, asset, layer) else {
                continue;
            };
            if out.contains_key(&path) {
                continue;
            }
            // `<prefix>/<pattern>_<suffix>` splits back into the source file
            // and the palette: the source is named without the material,
            // because one greyscale sheet makes all seventeen permutations.
            let Some((stem, suffix)) = path.rsplit_once('_') else {
                continue;
            };
            let Some(o) = trims
                .permute(stem, suffix)
                .and_then(|(rgba, w, h)| wr.upload_entity_trim(gpu, &path, &rgba, w, h))
            else {
                continue;
            };
            out.insert(path, o);
        }
    }
    out
}

pub(crate) fn armor_keys<'a>(
    session: &PlaySession,
    id: i32,
    items: &rewo_data::items::Items,
    equipment: &'a rewo_data::equipment::EquipmentAssets,
    trim_slots: &std::collections::HashMap<String, (u32, u32)>,
) -> [Option<rewo_gpu::entities::ArmorPiece<'a>>; 4] {
    use rewo_data::equipment::{color_for_layer, dye_argb, ArmorLayer};
    let worn = session.world.entities.armor(id);
    std::array::from_fn(|i| {
        let piece = worn[i]?;
        let asset = rewo_data::item_props_table::equip_asset(items.name(piece.item)?)?;
        // head 0, chest 1, legs 2, feet 3 — only the legs are inner.
        let layer = if i == 2 {
            ArmorLayer::Leggings
        } else {
            ArmorLayer::Humanoid
        };
        let dye = dye_argb(piece.dye);
        let defs = equipment.layers(asset, layer);
        if defs.len() > rewo_gpu::entities::MAX_ARMOR_SUBLAYERS {
            log::warn!(
                "armour: {asset} {:?} declares {} layers, drawing the first {}",
                layer,
                defs.len(),
                rewo_gpu::entities::MAX_ARMOR_SUBLAYERS
            );
        }
        let mut out = rewo_gpu::entities::ArmorPiece {
            // M50: `hasFoil`, decoded off the equipment packet's component
            // patch. One flag per piece — vanilla submits the foil once,
            // riding whichever layer draws first.
            foil: piece.foil,
            ..Default::default()
        };
        // The trim, if this piece has one and its sprite made it into the pool.
        out.trim = trim_sprite_path(session, &piece, asset, layer)
            .and_then(|p| trim_slots.get(&p).copied());
        let mut n = 0;
        for def in defs {
            let color = color_for_layer(def.dyeable, dye);
            // **Zero is not a black tint, it is no draw at all** — vanilla's
            // `if (color != 0)`. An undyed `onlyIfDyed` layer lands here.
            if color == 0 {
                continue;
            }
            if n == rewo_gpu::entities::MAX_ARMOR_SUBLAYERS {
                break;
            }
            out.layers[n] = Some((
                def.key.as_str(),
                [
                    ((color >> 16) & 0xFF) as f32 / 255.0,
                    ((color >> 8) & 0xFF) as f32 / 255.0,
                    (color & 0xFF) as f32 / 255.0,
                ],
            ));
            n += 1;
        }
        // Every layer suppressed is the same as no piece — but a trim alone
        // still draws, because vanilla submits it outside the layer loop.
        (n > 0 || out.trim.is_some()).then_some(out)
    })
}

pub(crate) fn held_foil(
    session: &PlaySession,
    id: i32,
    hand: rewo_world::entities::InteractionHand,
) -> bool {
    matches!(
        session.world.entities.hand_item(id, hand),
        rewo_world::entities::HandItem::Held(h) if h.glint
    )
}

pub(crate) fn bob_offset_for(id: i32) -> f32 {
    let mut z = (id as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // The top 24 bits as a [0,1) float, matching `nextFloat`'s precision.
    let unit = (z >> 40) as f32 * 5.960_464_5e-8;
    unit * std::f32::consts::TAU
}

/// Resolve one entity's vanilla emissive-layer inputs (M52).
///
/// Both are real wire state now, so neither is the branch's "vanilla synched
/// default" placeholder any more:
///
/// * `tendril` — `Warden.getTendrilAnimation(partial)`. Entity event **61**
///   sets `tendrilAnimation = 10` and `Warden.tick` decrements it once per
///   client tick, so a continuous clock reads it as `max(0, 10 - elapsed) / 10`.
///   A warden that has heard nothing has no receipt tick and reads 0 — still,
///   dark tendrils, which is exactly what vanilla shows.
/// * `eyes_glow` — `Creaking.isActive()`, metadata index 17 BOOLEAN, whose
///   `defineSynchedData` default is `false`.
pub(super) fn emissive_state(
    session: &PlaySession,
    id: i32,
    kind: EntityModelKind,
    _now: f32,
) -> rewo_gpu::entities::EmissiveState {
    use rewo_world::entities::EntityEvent;
    let ents = &session.world.entities;
    let tendril = match kind {
        EntityModelKind::Warden => ents
            .event_start(id, EntityEvent::WardenTendril)
            .map_or(0.0, |start| {
                let elapsed = (session.ticks as i64 - start).max(0) as f32;
                ((WARDEN_TENDRIL_TICKS - elapsed) / WARDEN_TENDRIL_TICKS).clamp(0.0, 1.0)
            }),
        _ => 0.0,
    };
    rewo_gpu::entities::EmissiveState {
        tendril,
        eyes_glow: matches!(kind, EntityModelKind::Creaking) && ents.creaking_active(id),
    }
}

/// `Warden.tendrilAnimation`'s start value, and the divisor
/// `getTendrilAnimation` normalizes by.
pub(super) const WARDEN_TENDRIL_TICKS: f32 = 10.0;

/// Choose one entity's ETF texture variant. The properties are keyed by the
/// mob-texture key, so a mob whose model uses several textures varies on its
/// *first* — the one a pack's `<entity>.properties` names.
pub(super) fn etf_variant(
    etf: &rewo_data::etf::EtfPack,
    kind: EntityModelKind,
    e: &rewo_world::entities::EntityState,
    id: i32,
    session: &PlaySession,
    cube_size: Option<i32>,
) -> u16 {
    let Some(key) = rewo_gpu::mobs::MOBS
        .iter()
        .find(|d| d.kind == kind)
        .and_then(|d| d.textures.first())
    else {
        return 0;
    };
    let props = rewo_data::etf::EntityProps {
        uuid: e.uuid,
        name: session.world.entities.custom_name(id),
        baby: session.world.entities.is_baby(id),
        size: cube_size,
        y: e.y.floor() as i32,
        // Before the first time packet the world clock reads 0 (dawn), which is
        // the same assumption the renderer makes elsewhere.
        day_ticks: session.day_ticks.unwrap_or(0),
    };
    etf.pick(key, &props) as u16
}

/// Vanilla's own metadata-driven texture variant for one entity (M64).
///
/// 0 — the baked base texture — for a mob that has none, for one whose server
/// never sent one, and for one whose variant names a texture the jar does not
/// ship. That last case is the M57b rule: a variant we cannot resolve leaves
/// the mob on its vanilla sheet rather than painting an invented one.
///
/// **The value's units depend on the mob and the kind is what says which.**
/// Cat, wolf and frog carry a raw datapack-registry id, so this walks the
/// registry the server synced and joins on the *texture path* — never on the
/// id, which is the server's to choose (REWO_PLAN §0.0, and M16/M42's rule).
/// Horse, llama and axolotl carry an enum ordinal, so those are transcribed
/// tables, each with its own out-of-bounds strategy.
///
/// The wolf is the one that reads a second field: `Wolf.getTexture` picks
/// `assets.tame` over `assets.wild` on `isTame()`, which is bit 0x04 of the
/// index-18 byte. Its third sheet, `angry`, is not chosen here — see
/// `rewo_data::mob_variants`.
/// `SheepWoolUndercoatLayer.submit`'s gate (M68), transcribed:
///
/// ```java
/// if (!state.isInvisible && (state.isJebSheep || state.woolColor != DyeColor.WHITE) && !state.isBaby)
/// ```
///
/// Two things to read off it. **There is no `isSheared` test** — shearing
/// takes `SheepWoolLayer` and leaves this one, which is why a shorn dyed sheep
/// keeps colour where a shorn white one is bare. And `woolColor` defaults to
/// `WHITE`, so an un-synced sheep gets nothing.
///
/// `isJebSheep` is deliberately absent here: it selects `ColorLerper
/// .getLerpedColor`'s rainbow, which Rewo does not render at all, so
/// including the disjunct would draw the layer in a colour the fleece beside
/// it is not wearing. Left out rather than approximated.
///
/// Shared with the `--tint-check` gate so the gate cannot grade a rule the
/// client does not actually apply (M18's lesson).
pub(crate) fn undercoat_visible(kind: EntityModelKind, dye: Option<u8>, is_baby: bool) -> bool {
    kind == EntityModelKind::Sheep && dye.unwrap_or(0) != 0 && !is_baby
}

/// Which of the two tropical-fish meshes a packed variant selects (M68).
///
/// `TropicalFishRenderer.submit` assigns `this.model` from
/// `state.pattern.base()` before *every* submission, so the shape is not a
/// property of the entity type — it is a field of the synched int, and the
/// low bit at that (`Pattern.packedId` is `base.id | index << 8`). One wire
/// name, two `EntityModelKind`s, chosen by the caller: the `PlayerSlim`
/// shape. Shared with `--variant-check` so the gate grades the client's own
/// mapping.
pub(crate) fn fish_kind(v: rewo_data::mob_variants::FishVariant) -> EntityModelKind {
    match v.base {
        rewo_data::mob_variants::FishBase::Small => EntityModelKind::TropicalFish,
        rewo_data::mob_variants::FishBase::Large => EntityModelKind::TropicalFishLarge,
    }
}
pub(crate) fn vanilla_variant(
    kind: EntityModelKind,
    id: i32,
    session: &PlaySession,
) -> u16 {
    let Some(v) = session.world.entities.variant(id) else {
        return 0;
    };
    let registry = |defs: &[rewo_net::variant_parse::MobVariantDef], tame: bool| {
        usize::try_from(v)
            .ok()
            .and_then(|i| defs.get(i))
            .and_then(|d| d.texture(tame))
            .and_then(rewo_data::mob_variants::variant_id)
            .unwrap_or(0)
    };
    match kind {
        EntityModelKind::Cat => registry(&session.cat_variants, false),
        EntityModelKind::Wolf => {
            registry(&session.wolf_variants, session.world.entities.is_tame(id))
        }
        EntityModelKind::Frog => registry(&session.frog_variants, false),
        EntityModelKind::Axolotl => {
            rewo_data::mob_variants::variant_id(rewo_data::mob_variants::axolotl_texture(v))
                .unwrap_or(0)
        }
        EntityModelKind::Llama => {
            rewo_data::mob_variants::variant_id(rewo_data::mob_variants::llama_texture(v))
                .unwrap_or(0)
        }
        EntityModelKind::Horse => {
            rewo_data::mob_variants::variant_id(rewo_data::mob_variants::horse_texture(v))
                .unwrap_or(0)
        }
        // M68. The one variant here that is not a *base* texture swap: it
        // moves the fish's **pattern layer** onto one of its shape's six
        // sheets, leaving the body slot alone. The shape itself is already
        // decided (it chose this `kind`), so unpacking again is just reading
        // the other field of the same int.
        EntityModelKind::TropicalFish | EntityModelKind::TropicalFishLarge => {
            let f = rewo_data::mob_variants::FishVariant::unpack(v);
            rewo_data::mob_variants::fish_pattern_variant(f.base, f.pattern)
        }
        _ => 0,
    }
}

/// The leash ribbons to draw this frame (M170), one per leashed entity that
/// has a resolvable holder. Faithful to `EntityRenderer`'s single-leash branch:
/// `start = entity.pos + leashOffset.yRot(-bodyYaw)`, `end =
/// holder.getRopeHoldPosition`, `slack = true` (the `LeashState` default the
/// branch never overrides), light sampled at each end's eye. The quad-leash
/// (happy-ghast) branch is not modelled; nothing Rewo renders takes it.
///
/// Eye height is `height * 0.85` — the `EntityDimensions.of` default. Vanilla
/// carries per-type overrides (a player's is 1.62, not 1.53); the leashables
/// are mobs where the default is exact or near it, so the divergence is a small
/// vertical shift of one rope end, documented rather than tabulated.
pub(super) fn collect_leashes(
    session: &PlaySession,
    etypes: &EntityTypes,
    alpha: f32,
    lightmap: &LightmapState,
) -> Vec<rewo_gpu::leash::LeashVertex> {
    // `Vec3.yRot(angle)`: x' = x·cos + z·sin, z' = z·cos - x·sin.
    fn yrot(v: [f64; 3], angle: f32) -> [f64; 3] {
        let (s, c) = (angle.sin() as f64, angle.cos() as f64);
        [v[0] * c + v[2] * s, v[1], v[2] * c - v[0] * s]
    }
    // A holder's `getRopeHoldPosition` and the world point to sample its light
    // at (its eye). Returns `None` when the holder is not tracked.
    let hold = |holder: i32| -> Option<([f64; 3], [f64; 3])> {
        if Some(holder) == session.player_id {
            let feet = [session.player.x, session.player.y, session.player.z];
            let eye = session.player.eye_y();
            return Some((
                [feet[0], feet[1] + (eye - feet[1]) * 0.7, feet[2]],
                [feet[0], eye, feet[2]],
            ));
        }
        let h = session.world.entities.get(holder)?;
        let pos = h.render_pos(alpha);
        if etypes.name(h.type_id) == Some("minecraft:leash_knot") {
            // LeashFenceKnotEntity.getRopeHoldPosition = pos + (0, 0.2, 0).
            Some(([pos[0], pos[1] + 0.2, pos[2]], [pos[0], pos[1] + 0.2, pos[2]]))
        } else {
            let (_, hh) = etypes.dimensions(h.type_id);
            let heye = (hh * 0.85) as f64;
            Some((
                [pos[0], pos[1] + heye * 0.7, pos[2]],
                [pos[0], pos[1] + heye, pos[2]],
            ))
        }
    };

    let mut verts: Vec<rewo_gpu::leash::LeashVertex> = Vec::new();
    for (id, e) in session.world.entities.iter() {
        let Some(holder) = session.world.entities.leash_holder(id) else {
            continue;
        };
        let Some((end, end_eye)) = hold(holder) else {
            continue;
        };
        let pos = e.render_pos(alpha);
        let (w, ht) = etypes.dimensions(e.type_id);
        let eye = (ht * 0.85) as f64;
        // leashOffset = (0, eyeHeight, bbWidth * 0.4), rotated by -bodyYaw.
        let offset = yrot([0.0, eye, (w * 0.4) as f64], -e.yaw.to_radians());
        let start = [pos[0] + offset[0], pos[1] + offset[1], pos[2] + offset[2]];
        // Light at each eye (vanilla samples `getEyePosition`, not the rope
        // end), carried as PACKED components - the ribbon interpolates block
        // and sky separately and evaluates the lightmap curve per vertex.
        let start_packed = session.world.light_at(
            pos[0].floor() as i32,
            (pos[1] + eye).floor() as i32,
            pos[2].floor() as i32,
        );
        let end_packed = session.world.light_at(
            end_eye[0].floor() as i32,
            end_eye[1].floor() as i32,
            end_eye[2].floor() as i32,
        );
        verts.extend(rewo_gpu::leash::build_ribbon(
            start,
            end,
            true,
            start_packed,
            end_packed,
            &|b, s| rewo_world::lightmap::sample(b, s, lightmap),
        ));
    }
    verts
}

pub(super) fn collect_entities<'a>(
    session: &'a PlaySession,
    etypes: &EntityTypes,
    alpha: f32,
    gestures: &mut GestureTracker,
    now: f32,
    skins: &SkinRegistry,
    lightmap: &LightmapState,
    spears: &rewo_data::item_tags::ItemTag,
    bow_item: Option<i32>,
    item_names: &'a rewo_data::items::Items,
    // Armour layer definitions, for resolving what each entity wears into an
    // atlas key (M46).
    equipment: &'a rewo_data::equipment::EquipmentAssets,
    // Where this frame's trim sprites landed in the pool (M48).
    trim_slots: &std::collections::HashMap<String, (u32, u32)>,
    // The resource pack's ETF random-entity rules (M52). Empty without a pack,
    // in which case every entity keeps its vanilla texture.
    etf: &rewo_data::etf::EtfPack,
    // `Minecraft.getInstance().gui.hud.isHidden()` — F1 (M70). Suppresses
    // every floating label on an un-teamed entity, and nothing on a teamed
    // one, because the team switch returns first.
    hud_hidden: bool,
    // `entityRenderDispatcher.crosshairPickEntity` (M73) — resolved once by
    // the caller, because vanilla resolves it once: `Minecraft.pick` runs one
    // raycast and `EntityRenderDispatcher.prepare` hands the single result to
    // every renderer.
    crosshair_pick: Option<i32>,
) -> Vec<EntityDraw<'a>> {
    let player_color = linear_rgb(0xE5, 0xB8, 0xC5); // accent rose
    let mob_color = linear_rgb(0x9A, 0x80, 0x87); // text mauve
                                                  // M59: the health bar's two gates that need session-wide state — the
                                                  // attribute registry (max health, name-tag distance) and the camera
                                                  // position `EntityRenderDispatcher.distanceToSqr` measures from, which is
                                                  // the *eye*, not the feet.
    let attr_reg = session.attribute_registry.as_deref();
    let eye = player_eye(session);
    // M70: the viewer half of the label predicate — camera entity, F1, game
    // mode and the viewer's own team. Read once per frame, not per entity.
    let mut viewer = LabelViewer::from_session(session, hud_hidden);
    viewer.crosshair_pick = crosshair_pick;
                                                  // Headless-only verification knob: `REWO_FORCE_LIMB=swing,amount`
                                                  // pins every player's walk pose so a still-target PNG can prove the
                                                  // limb-swing mechanism deterministically (a live walker's phase at
                                                  // capture time is timing-dependent). One-shot; zero-cost when unset.
    let force_limb: Option<(f32, f32)> = crate::knobs::var("REWO_FORCE_LIMB").ok().and_then(|s| {
        let mut it = s.split(',');
        Some((
            it.next()?.trim().parse().ok()?,
            it.next()?.trim().parse().ok()?,
        ))
    });
    // Headless-only knob: `REWO_FORCE_HEAD=<degrees>` cranks every mob's head
    // yaw to body-yaw + this offset, so a PNG can prove head-look turns the
    // head independently of the body without depending on live server AI.
    let force_head: Option<f32> = crate::knobs::var("REWO_FORCE_HEAD")
        .ok()
        .and_then(|s| s.trim().parse().ok());
    // Headless-only knob: `REWO_FORCE_GESTURE=<name>[,<age_s>]` pins every
    // gesture-rigged mob into that state (mobshot names, e.g.
    // "warden_roar,1.5") — deterministic gesture PNGs without server AI.
    let force_gesture: Option<(rewo_gpu::mobs::Gesture, f32)> =
        crate::knobs::var("REWO_FORCE_GESTURE").ok().and_then(|s| {
            let mut it = s.split(',');
            let g = rewo_gpu::mobs::Gesture::from_name(it.next()?.trim())?;
            let age = it.next().and_then(|a| a.trim().parse().ok()).unwrap_or(0.0);
            Some((g, age))
        });
    let mut out = Vec::new();
    for (id, e) in session.world.entities.iter() {
        let p = e.render_pos(alpha);
        let name = etypes.name(e.type_id).unwrap_or("");
        let is_player = e.type_id == etypes.player_id;
        // A player with a resolved skin wears it (slim → the Alex model);
        // otherwise the default wide Steve.
        let player_skin = if is_player { skins.get(&e.uuid) } else { None };
        let kind = if is_player {
            match player_skin {
                Some(ps) if ps.slim => EntityModelKind::PlayerSlim,
                _ => EntityModelKind::Player,
            }
        } else {
            rewo_gpu::mobs::kind_for_entity_name(name)
        };
        // M68: one wire name, two meshes.
        let fish = (kind == EntityModelKind::TropicalFish).then(|| {
            rewo_data::mob_variants::FishVariant::unpack(
                session.world.entities.variant(id).unwrap_or(0),
            )
        });
        let kind = fish.map_or(kind, |f| fish_kind(f));
        // M24b: a dropped stack. `ItemEntity.DATA_ITEM` arrives as metadata
        // index 8 with the ITEM_STACK serializer; an entity with one renders
        // as the item and nothing else, so the model kind is never consulted.
        // Gated on the type actually being `minecraft:item`: nothing else puts
        // an ITEM_STACK at slot 8, but the gate makes that explicit rather
        // than relying on it.
        let is_item_entity = Some(e.type_id) == etypes.id_of("minecraft:item");
        let ground_stack = is_item_entity
            .then(|| session.world.entities.item_stack(id))
            .flatten();

        // Slime / magma-cube size (metadata index 16, vanilla default 1;
        // the model + bbox scale linearly by it). Our slime model is baked
        // at the size-2 look, so scale_mul = size/2 (size 2 → 1.0).
        let cube_size = matches!(kind, EntityModelKind::Slime | EntityModelKind::MagmaCube)
            .then(|| session.world.entities.size(id).unwrap_or(2).clamp(1, 32));
        let (mut w, mut h) = match cube_size {
            Some(sz) => (0.51 * sz as f32, 0.51 * sz as f32),
            None => etypes.dimensions(e.type_id),
        };
        let mut scale_mul = cube_size.map_or(1.0, |sz| sz as f32 / 2.0);
        // Baby mobs (ageable / zombie families) render at ~half scale.
        // Uniform approximation — vanilla keeps the head proportionally
        // larger (a per-part transform), deferred.
        if !is_player && session.world.entities.is_baby(id) {
            scale_mul *= 0.5;
            w *= 0.5;
            h *= 0.5;
        }
        let (limb_swing, limb_amount) = force_limb.unwrap_or_else(|| e.limb());
        // Gesture + wire-event rigs: pose/state → rig, timed from the observed
        // change; one-shot events (warden attack/sonic boom, armadillo peek)
        // resolved against their receipt ticks with the exact vanilla ownership
        // rules (roar-stop, peek re-clock). A forced gesture (headless knob)
        // bypasses the tracker and carries no events.
        let (gesture, events) = if let Some(fg) = force_gesture {
            (Some(fg), [None; rewo_gpu::mobs::ModelEvent::COUNT])
        } else {
            use rewo_world::entities::EntityEvent;
            let ents = &session.world.entities;
            resolve_mob_anim(
                kind,
                ents.pose(id),
                ents.gesture_state(id),
                ents.event_start(id, EntityEvent::WardenAttack),
                ents.event_start(id, EntityEvent::WardenSonicBoom),
                ents.event_start(id, EntityEvent::ArmadilloPeek),
                gestures,
                id,
                now,
                session.ticks as i64,
                alpha,
            )
        };
        // Armadillo shell swap (vanilla `shouldHideInShell` per state):
        // ROLLING balls up after 5 ticks, SCARED always, UNROLLING opens
        // at tick 26.
        let shell = kind == EntityModelKind::Armadillo
            && match gesture {
                Some((rewo_gpu::mobs::Gesture::ArmadilloRoll, age)) => age > 0.25,
                Some((rewo_gpu::mobs::Gesture::ArmadilloScared, _)) => true,
                Some((rewo_gpu::mobs::Gesture::ArmadilloUnroll, age)) => age < 1.3,
                _ => false,
            };
        // Allay dance (index-16 `DATA_DANCING` metadata → client counters).
        // Shared with the `danceshot` oracle so the kind gate + counter mapping
        // are exercised by the gate and can't regress here silently.
        let allay_dance = resolve_allay_dance(kind, &session.world.entities, id, alpha);
        // Combat swing (`ClientboundAnimatePacket` → the swing clock). Shared
        // with the `swingshot` oracle for the same reason as the dance above.
        let attack = resolve_attack_anim(&session.world.entities, id, alpha);
        // The hold pose the swing is layered onto. Same sharing rule as above.
        let arm_poses = resolve_arm_poses(
            &session.world.entities,
            id,
            kind,
            spears,
            bow_item,
            crossbow_item_id(),
        );
        // M20: the synced mob state the undead / skeleton / illager rigs read.
        let mob = resolve_mob_combat(&session.world.entities, id, kind, bow_item);
        // M70: the label-visibility predicate, resolved once and consumed by
        // both the nametag and the health bar so the two cannot disagree.
        // `EntityRenderDispatcher.distanceToSqr` measures from the camera,
        // which is the *eye*, not the feet.
        let dx = p[0] - eye.x as f64;
        let dy = p[1] - eye.y as f64;
        let dz = p[2] - eye.z as f64;
        let label = resolve_label_inputs(
            session,
            id,
            etypes.name(e.type_id),
            attr_reg,
            is_player,
            dx * dx + dy * dy + dz * dz,
            &viewer,
        );
        let (label_name, label_health) = resolve_labels(
            &session.world.entities,
            id,
            etypes.name(e.type_id),
            attr_reg,
            &label,
            // A player shows their profile name; anything else its metadata
            // custom name.
            if is_player {
                session.world.entities.name_of(e.uuid)
            } else {
                session.world.entities.custom_name(id)
            },
        );
        out.push(EntityDraw {
            pos: [p[0] as f32, p[1] as f32, p[2] as f32],
            width: w,
            height: h,
            color: if is_player { player_color } else { mob_color },
            // M70: both floating labels now hang off one predicate, resolved
            // together by `resolve_labels` so they cannot disagree. *Whether*
            // either is drawn is `shouldShowName`, not the mere existence of a
            // string — which is all it used to be.
            name: label_name,
            health: label_health,
            kind,
            yaw: e.yaw,
            // M24: `state.deathTime = entity.deathTime > 0 ? deathTime + partial : 0`.
            death_time: session.world.entities.death_state(id).render_death_time(alpha),
            // M24b: a dropped stack. `Some` makes the renderer draw the item
            // instead of a model — `ItemEntityRenderer` has no body of its own.
            ground_item: ground_stack.and_then(|(i, _, _)| item_names.name(i)),
            // `ItemStack.hasFoil()` (M45), decoded at the wire because it
            // lives only in the component patch.
            armor: armor_keys(session, id, item_names, equipment, trim_slots),
            held_glint: [
                held_foil(session, id, rewo_world::entities::InteractionHand::MainHand),
                held_foil(session, id, rewo_world::entities::InteractionHand::OffHand),
            ],
            ground_glint: ground_stack.is_some_and(|(_, _, foil)| foil),
            ground_count: ground_stack.map_or(0, |(_, n, _)| n),
            // `ItemEntity.bobOffs` is `random.nextFloat() * 2 * PI`, rolled in
            // the constructor and never sent. Derived from the entity id here
            // so it is stable per entity; there is no server value it could
            // match, because vanilla's own clients each roll their own.
            bob_offset: bob_offset_for(id),
            ground_seed: ground_stack.map_or(0, |(i, _, _)| i),
            // A live item entity turns on the shared clock; only a pickup
            // animation freezes it (M81).
            ground_age: None,
            head_yaw: force_head.map_or(e.head_yaw, |off| e.yaw + off),
            pitch: e.pitch,
            limb_swing,
            limb_amount,
            gesture,
            events,
            shell,
            allay_dance,
            attack,
            arm_poses,
            mob,
            // M21: `hasRedOverlay` — the damage flash.
            // `hasRedOverlay = hurtTime > 0 || deathTime > 0` — the whole
            // disjunction as of M24; M21 shipped only the first term.
            hurt: session.world.entities.has_red_overlay(id),
            // M22: what each arm is holding.
            held: resolve_held_items(&session.world.entities, id, item_names),
            skin_uv: player_skin.map(|ps| ps.uv),
            scale_mul,
            mount: None,
            anim_id: (id & 0xffff) as f32,
            light: entity_light(&session.world, p[0], p[1] + h as f64 * 0.85, p[2], lightmap),
            // M52. Vanilla's synched defaults: the warden's tendril countdown
            // rides entity_event 61 and the creaking's glow rides metadata
            // IS_ACTIVE — both resolved below.
            emissive: emissive_state(session, id, kind, now),
            // Two things can move a mob off its baked texture, and vanilla's
            // own wins. M64's variant is what the *server* says this cat or
            // horse is; M57b's ETF rule is a pack randomising the base
            // texture — and a black cat is not drawing that texture at all,
            // so the two never mean the same slot. Vanilla first, ETF only
            // where vanilla has nothing to say.
            variant: match vanilla_variant(kind, id, session) {
                0 if !etf.is_empty() => etf_variant(etf, kind, e, id, session, cube_size),
                v => v,
            },
            // The sheep's wool colour (`Sheep.DATA_WOOL_ID`). `None` is
            // vanilla's `DyeColor.WHITE` default, which still tints.
            dye: session.world.entities.wool_color(id),
            // …and bit 0x10 of the same byte (M64), which drops the fleece
            // rather than recolouring it.
            sheared: session.world.entities.is_sheared(id),
            // M68: `SheepWoolUndercoatLayer.submit`'s gate.
            undercoat: undercoat_visible(
                kind,
                session.world.entities.wool_color(id),
                session.world.entities.is_baby(id),
            ),
            // M68: the fish's two dyes, `[body, pattern]`.
            fish_dye: fish.map(|f| [f.body_color, f.pattern_color]),
            // M60. `player_skin` is this profile's uploaded textures; its
            // `cape` is `None` both when the profile carries no cape and
            // when one is still in flight, and either way vanilla's second
            // gate says draw nothing.
            cape: resolve_cape(
                &session.world.entities,
                id,
                kind,
                alpha,
                player_skin.and_then(|ps| ps.cape),
                item_names,
                equipment,
            ),
        });
    }
    // Drop tracker entries for despawned entities (recycled server ids
    // must not inherit a stale gesture clock).
    gestures
        .map
        .retain(|id, _| session.world.entities.get(*id).is_some());
    out
}

/// Borrow the baked mob-texture table into the entity pass's view type.
/// Initialize the entity pass, optionally overriding mob models from an
/// OptiFine CEM resource pack (`--pack` / `REWO_PACK`). Shared by the
/// headless and windowed live paths (M9).
///
/// Also loads the pack's ETF texture variants (M52) — models and textures come
/// out of the same zip — and returns its random-entity rules for the draw
/// builder to pick with. Empty when there is no pack.
pub(crate) fn init_entities_maybe_cem(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
    pack: &Option<PathBuf>,
) -> Result<rewo_data::etf::EtfPack, String> {
    let pack = pack
        .clone()
        .or_else(|| std::env::var("REWO_PACK").ok().map(PathBuf::from));
    let etf = match pack {
        Some(path) => {
            let cem = crate::cem_pack::load_cem_overrides(&path)?;
            let etf = rewo_data::etf::load_pack(&path).unwrap_or_else(|e| {
                // A pack with unreadable random-entity data still gets its
                // models; the mobs simply keep vanilla textures.
                log::warn!("live: ETF load failed ({e}) — vanilla textures");
                rewo_data::etf::EtfPack::default()
            });
            log::info!(
                "live: pack {} → {} model overrides, {} texture-variant rules",
                path.display(),
                cem.len(),
                etf.rules.len()
            );
            wr.init_entities_with_cem(
                gpu,
                font_data(baked),
                entity_textures_with(baked, &etf),
                cem,
            )?;
            etf
        }
        None => {
            wr.init_entities(gpu, font_data(baked), entity_textures(baked))?;
            rewo_data::etf::EtfPack::default()
        }
    };
    // **After** the pass exists — `init_entities` builds it, and installing
    // the glint first would have nothing to install into. M44 learned this
    // one the expensive way on the hand pass, where the order was reversed
    // and the shimmer silently never drew.
    if let Some(g) = baked.glint.as_ref() {
        wr.init_entity_glint(gpu, &g.rgba, g.w, g.h)?;
    }
    // M50: the worn-armour foil's own sheet, same ordering rule.
    if let Some(g) = baked.armor_glint.as_ref() {
        wr.init_entity_armor_glint(gpu, &g.rgba, g.w, g.h)?;
    }
    Ok(etf)
}

/// M175 — the generated isBaby swaps resolved to (kind index, adult key,
/// baby key), filtered to pairs whose BOTH sheets are in this bake. The GPU
/// side turns each into a per-slot UV offset.
pub(super) fn baby_swap_pairs(baked: &assets::BakedAssets) -> Vec<(u16, &'static str, &'static str)> {
    rewo_data::baby_texture_table::BABY_SWAPS
        .iter()
        .filter_map(|swap| {
            let baked_keys = || {
                baked
                    .mob_textures
                    .iter()
                    .any(|t| t.key == swap.baby_key)
            };
            let adult_key = adult_key_of(swap)?;
            if !baked_keys() {
                return None;
            }
            // `kind_for_entity_name` wants the NAMESPACED wire name (its
            // strip_prefix is the whole lookup); the table stores short
            // names, so re-namespace here — the first cut passed the short
            // name, every row resolved to Capsule, and the swaps were inert.
            let kind = rewo_gpu::mobs::kind_for_entity_name(&format!(
                "minecraft:{}",
                swap.entity
            ));
            Some((kind.index() as u16, adult_key, swap.baby_key))
        })
        .collect()
}

/// The MOB_TEXTURE_SPECS key of a swap's adult sheet — `None` when the adult
/// path is not one Rewo bakes at all (then no model uses it and there is
/// nothing to offset).
pub(crate) fn adult_key_of(swap: &rewo_data::baby_texture_table::BabySwap) -> Option<&'static str> {
    let rel = swap.adult_path.strip_prefix("textures/").unwrap_or(swap.adult_path);
    rewo_data::assets::mob_texture_key(rel)
}

pub(crate) fn entity_textures(baked: &assets::BakedAssets) -> MobTextures<'_> {
    MobTextures {
        // M64: vanilla's own metadata-driven alternates are always present —
        // they are jar textures, not a pack's, so they do not wait for one.
        // `entity_textures_with` appends a pack's ETF alternates after these;
        // the two live in disjoint id bands.
        variants: vanilla_variants(baked),
        baby_swaps: baby_swap_pairs(baked),
        entries: baked
            .mob_textures
            .iter()
            .map(|t| MobTexEntry {
                key: t.key,
                w: t.w,
                h: t.h,
                rgba: &t.rgba,
            })
            // M46: the armour sheets share the entity atlas. They are 64x32
            // rather than 64x64, so the shelf packer fits them alongside the
            // mob textures with no special case.
            .chain(baked.equipment.textures.iter().map(|t| MobTexEntry {
                key: &t.key,
                w: t.w,
                h: t.h,
                rgba: &t.rgba,
            }))
            .collect(),
    }
}

/// `entity_textures` plus a pack's ETF alternates (M52), which the entity pass
/// packs into the same atlas and addresses by variant id.
pub(crate) fn entity_textures_with<'a>(
    baked: &'a assets::BakedAssets,
    etf: &'a rewo_data::etf::EtfPack,
) -> MobTextures<'a> {
    MobTextures {
        variants: vanilla_variants(baked)
            .into_iter()
            .chain(
                etf.textures
                    .iter()
                    .map(|t| rewo_gpu::entities::VariantTexEntry {
                        base_key: t.key,
                        index: t.index,
                        w: t.w,
                        h: t.h,
                        rgba: &t.rgba,
                    }),
            )
            .collect(),
        ..entity_textures(baked)
    }
}

/// Vanilla's metadata-driven alternates as atlas entries (M64).
///
/// They are appended *after* the base textures in the packer's input, exactly
/// as a pack's ETF alternates are, so every existing texel address is
/// unchanged and `mobshot --check` still grades the geometry it graded before.
pub(super) fn vanilla_variants(baked: &assets::BakedAssets) -> Vec<rewo_gpu::entities::VariantTexEntry<'_>> {
    baked
        .mob_variant_textures
        .iter()
        .map(|t| rewo_gpu::entities::VariantTexEntry {
            base_key: t.key,
            index: t.index as u32,
            w: t.w,
            h: t.h,
            rgba: &t.rgba,
        })
        .collect()
}

pub(crate) fn font_data(baked: &assets::BakedAssets) -> Option<FontData<'_>> {
    baked.font.as_ref().map(|f: &BakedFont| FontData {
        atlas: &f.atlas,
        size: f.atlas_size,
        cell: f.cell,
        advance: &f.advance,
        white_texel: f.white_texel,
    })
}
