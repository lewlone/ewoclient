use super::*;

/// Borrow the baked HUD sprites into the renderer's view type.
pub(super) fn hud_sprite(sp: &assets::HudSprite) -> rewo_gpu::hud::HudSpriteData<'_> {
    rewo_gpu::hud::HudSpriteData {
        rgba: &sp.rgba,
        w: sp.w,
        h: sp.h,
    }
}

/// Borrow the container screen's textures out of the bake (M35). `None` when
/// the jar had none, which degrades to no screen rather than a crash.
pub(crate) fn container_sprites(
    baked: &assets::BakedAssets,
) -> Option<rewo_gpu::container::ContainerSpriteData<'_>> {
    let c = baked.container.as_ref()?;
    fn s(x: &rewo_data::assets::HudSprite) -> rewo_gpu::hud::HudSpriteData<'_> {
        rewo_gpu::hud::HudSpriteData {
            rgba: &x.rgba,
            w: x.w,
            h: x.h,
        }
    }
    Some(rewo_gpu::container::ContainerSpriteData {
        background: s(&c.background),
        highlight_back: s(&c.highlight_back),
        highlight_front: s(&c.highlight_front),
        tooltip_background: s(&c.tooltip_background),
        tooltip_frame: s(&c.tooltip_frame),
        bundle_slot: s(&c.bundle_slot),
        bundle_highlight_back: s(&c.bundle_highlight_back),
        bundle_highlight_front: s(&c.bundle_highlight_front),
        bundle_bar_border: s(&c.bundle_bar_border),
        bundle_bar_fill: s(&c.bundle_bar_fill),
        bundle_bar_full: s(&c.bundle_bar_full),
        menu_backgrounds: c.menu_backgrounds.iter().map(s).collect(),
        overlays: c.overlays.iter().map(s).collect(),
    })
}

/// Borrow the three button sheets out of the bake (M82). `None` when the jar
/// had none, which degrades to a screen with no button chrome — text still
/// draws, exactly as a missing HUD sprite degrades to no HUD.
pub(crate) fn widget_sprites(
    baked: &assets::BakedAssets,
) -> Option<rewo_gpu::screen::WidgetSpriteData<'_>> {
    let w = baked.widgets.as_ref()?;
    Some(rewo_gpu::screen::WidgetSpriteData {
        button: hud_sprite(&w.button),
        button_disabled: hud_sprite(&w.button_disabled),
        button_highlighted: hud_sprite(&w.button_highlighted),
        menu_background: hud_sprite(&w.menu_background),
        inworld_menu_background: hud_sprite(&w.inworld_menu_background),
        tabs: std::array::from_fn(|i| hud_sprite(&w.tabs[i])),
        scroller: hud_sprite(&w.scroller),
        scroller_background: hud_sprite(&w.scroller_background),
        slot: hud_sprite(&w.slot),
        stat_header: hud_sprite(&w.stat_header),
        stat_columns: std::array::from_fn(|i| hud_sprite(&w.stat_columns[i])),
        sort_up: hud_sprite(&w.sort_up),
        sort_down: hud_sprite(&w.sort_down),
        tab_header_background: hud_sprite(&w.tab_header_background),
        inworld_header_separator: hud_sprite(&w.inworld_header_separator),
        inworld_footer_separator: hud_sprite(&w.inworld_footer_separator),
        book_background: hud_sprite(&w.book_background),
        page_buttons: std::array::from_fn(|i| hud_sprite(&w.page_buttons[i])),
        slider: std::array::from_fn(|i| hud_sprite(&w.slider[i])),
        sign_boards: std::array::from_fn(|i| hud_sprite(&w.sign_boards[i])),
        hanging_sign_boards: std::array::from_fn(|i| hud_sprite(&w.hanging_sign_boards[i])),
        adv_window: hud_sprite(&w.adv_window),
        adv_tabs: std::array::from_fn(|i| hud_sprite(&w.adv_tabs[i])),
        adv_frames: std::array::from_fn(|i| hud_sprite(&w.adv_frames[i])),
        adv_boxes: std::array::from_fn(|i| hud_sprite(&w.adv_boxes[i])),
        adv_backgrounds: std::array::from_fn(|i| hud_sprite(&w.adv_backgrounds[i])),
    })
}

pub(crate) fn hud_sprites(
    baked: &assets::BakedAssets,
) -> Option<rewo_gpu::hud::HudSpritesData<'_>> {
    let h = baked.hud.as_ref()?;
    Some(rewo_gpu::hud::HudSpritesData {
        hotbar: hud_sprite(&h.hotbar),
        selection: hud_sprite(&h.selection),
        crosshair: hud_sprite(&h.crosshair),
        heart_full: hud_sprite(&h.heart_full),
        heart_half: hud_sprite(&h.heart_half),
        heart_container: hud_sprite(&h.heart_container),
        // M155, in `HeartSprite` order. The array is what keeps the two
        // crates' orderings from drifting: they are written in crates that
        // do not depend on each other, so this is the one place the mapping
        // is spelled out.
        heart_extra: [
            hud_sprite(&h.heart_container_blinking),
            hud_sprite(&h.heart_full_blinking),
            hud_sprite(&h.heart_half_blinking),
            hud_sprite(&h.heart_absorbing_full),
            hud_sprite(&h.heart_absorbing_half),
        ],
        food_full: hud_sprite(&h.food_full),
        food_half: hud_sprite(&h.food_half),
        food_empty: hud_sprite(&h.food_empty),
        experience_bar_background: hud_sprite(&h.experience_bar_background),
        experience_bar_progress: hud_sprite(&h.experience_bar_progress),
        // M151 — `std::array::from_fn` rather than a `Vec` collect, because the
        // pass's own field is a `[Rect; 6]` and a length mismatch should be a
        // compile error rather than a runtime one.
        ping: std::array::from_fn(|i| hud_sprite(&h.ping[i])),
        // M168 — same discipline for every fixed-size set.
        player_hearts: std::array::from_fn(|i| hud_sprite(&h.player_hearts[i])),
        armor: std::array::from_fn(|i| hud_sprite(&h.armor[i])),
        air: std::array::from_fn(|i| hud_sprite(&h.air[i])),
        vehicle_hearts: std::array::from_fn(|i| hud_sprite(&h.vehicle_hearts[i])),
        food_hunger: std::array::from_fn(|i| hud_sprite(&h.food_hunger[i])),
        effect_background: hud_sprite(&h.effect_background),
        effect_background_ambient: hud_sprite(&h.effect_background_ambient),
        jump_bar: std::array::from_fn(|i| hud_sprite(&h.jump_bar[i])),
        effect_icons: h.effect_icons.iter().map(hud_sprite).collect(),
    })
}

pub(crate) fn layer_animations(
    baked: &assets::BakedAssets,
) -> Vec<rewo_gpu::world::LayerAnimation> {
    baked.animations.iter().map(to_gpu_animation).collect()
}

fn to_gpu_animation(a: &assets::AnimatedLayer) -> rewo_gpu::world::LayerAnimation {
    rewo_gpu::world::LayerAnimation {
        layer: a.layer,
        frames: a.frames.clone(),
        order: a.order.clone(),
        frametime: a.frametime,
        interpolate: a.interpolate,
    }
}

/// Install the bake's animated block layers AND the fluid flow array with its
/// animations (M197). Every renderer that draws terrain calls this one, so a
/// gate cannot install one half and miss the other.
pub(crate) fn install_animations(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    wr.set_animations(layer_animations(baked));
    wr.set_flow_textures(
        gpu,
        &baked.flow_layers,
        baked.flow_animations.iter().map(to_gpu_animation).collect(),
    )
}

/// MC-convention eye camera: yaw 0 faces +Z (south), yaw+ turns west,
/// pitch+ looks down.
pub(super) fn eye_view_proj(
    eye: Vec3,
    yaw_deg: f32,
    pitch_deg: f32,
    aspect: f32,
    fov_deg: f32,
) -> [[f32; 4]; 4] {
    eye_view_proj_hurt(eye, yaw_deg, pitch_deg, aspect, fov_deg, HurtTilt::NONE)
}

/// `GameRenderer.bobHurt`'s inputs, resolved for one frame (M81).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HurtTilt {
    /// `cameraState.entityRenderState.hurtTime` — `hurtTime - partialTicks`,
    /// and therefore **negative** on the frames after the clock hits zero.
    pub hurt_time: f32,
    /// `hurtDuration`, the divisor. 10 for every armed clock.
    pub hurt_duration: i32,
    /// `getHurtDir()` in degrees — 0 for anything that is not a player.
    pub hurt_dir: f32,
    /// `optionsRenderState.damageTiltStrength`, vanilla's accessibility slider
    /// (`UnitDouble`, default 1.0). Rewo's `no_damage_tilt` module drives it to
    /// 0, which is exactly what the slider's "off" end does.
    pub strength: f32,
}

impl HurtTilt {
    /// No tilt: an entity that has never been hurt.
    pub const NONE: Self = Self {
        hurt_time: 0.0,
        hurt_duration: 10,
        hurt_dir: 0.0,
        strength: 1.0,
    };
}

/// `GameRenderer.bobHurt` — the camera lurch when you take a hit (M81).
///
/// ```text
/// float hurt = hurtTime;                       // already minus partialTicks
/// if (hurt < 0.0F) return;
/// hurt /= hurtDuration;
/// hurt = Mth.sin(hurt * hurt * hurt * hurt * (float) Math.PI);
/// float rr = hurtDir;
/// poseStack.mulPose(Axis.YP.rotationDegrees(-rr));
/// poseStack.mulPose(Axis.ZP.rotationDegrees(-hurt * 14.0 * damageTiltStrength));
/// poseStack.mulPose(Axis.YP.rotationDegrees(rr));
/// ```
///
/// Three things read backwards here:
///
/// * **It is not a lean, it is a conjugated roll.** `Ry(-rr) · Rz(θ) · Ry(rr)`
///   is a rotation about the axis `Ry(-rr) · ẑ` — so at `hurtDir` 0 the camera
///   *rolls* (the horizon tips), and at 90° it *pitches* (the camera nods).
///   `hurtDir` selects the plane, and the plane is a **camera-space** one,
///   because vanilla post-multiplies this onto the projection and leaves the
///   view matrix alone.
/// * **The easing is `sin(x⁴·π)`, in the fraction of the clock remaining.**
///   `hurtTime` counts *down* from 10, so `x` runs 1 → 0, and `sin(x⁴π)` is
///   zero at both ends with its peak at `x = 0.5^0.25 ≈ 0.841` — a fifth of a
///   second after the hit. A tilt that started at full strength and decayed
///   (the obvious reading) would snap rather than swing.
/// * **The guard is `< 0`, not `<= 0`.** The render-state field is
///   `hurtTime - partialTicks`, so on the frames after the last tick it goes
///   negative and the tilt is skipped outright; at exactly 0 it is still
///   evaluated, and `sin(0)` makes it a no-op anyway.
///
/// The death spin vanilla applies *before* the guard is not here: Rewo has no
/// first-person death camera, and reproducing half of `bobHurt` in the wrong
/// order would be worse than leaving that clause out and saying so.
pub(crate) fn bob_hurt(t: HurtTilt) -> Mat4 {
    if t.hurt_time < 0.0 || t.hurt_duration == 0 {
        return Mat4::IDENTITY;
    }
    let x = t.hurt_time / t.hurt_duration as f32;
    let hurt = (x * x * x * x * std::f32::consts::PI).sin();
    let rr = t.hurt_dir.to_radians();
    let tilt = (-hurt * 14.0 * t.strength).to_radians();
    Mat4::from_rotation_y(-rr) * Mat4::from_rotation_z(tilt) * Mat4::from_rotation_y(rr)
}

/// The frame's view-projection with the damage tilt folded in.
///
/// **`P · B · V`**, matching vanilla's `projectionMatrix.mul(bobStack)`
/// followed by a separately-set model-view: the bob sits between the
/// projection and the view, so it rotates in camera space rather than about a
/// world axis.
pub(crate) fn eye_view_proj_hurt(
    eye: Vec3,
    yaw_deg: f32,
    pitch_deg: f32,
    aspect: f32,
    fov_deg: f32,
    tilt: HurtTilt,
) -> [[f32; 4]; 4] {
    let view = eye_view(eye, yaw_deg, pitch_deg);
    let proj = Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        fov_deg.to_radians(),
        aspect.max(0.01),
        0.05,
    ));
    (proj * bob_hurt(tilt) * view).to_cols_array_2d()
}

/// The view matrix alone — extracted so the M37 particle billboards take their
/// right/up basis from exactly the matrix the frame is projected through,
/// rather than from a second construction that could drift from it.
pub(super) fn eye_view(eye: Vec3, yaw_deg: f32, pitch_deg: f32) -> Mat4 {
    let yaw = yaw_deg.to_radians();
    let pitch = pitch_deg.to_radians();
    let dir = Vec3::new(
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    );
    crate::view_math::look_to_rh(eye, dir, Vec3::Y)
}

/// `Camera.setup`'s fill of the camera entity's hurt fields (M81).
///
/// ```text
/// cameraState.entityRenderState.hurtDir      = livingEntity.getHurtDir();
/// cameraState.entityRenderState.hurtTime     = livingEntity.hurtTime - cameraEntityPartialTicks;
/// cameraState.entityRenderState.hurtDuration = livingEntity.hurtDuration;
/// ```
///
/// The camera entity is the local player, whose id the entity table does not
/// hold — but the hurt clock and direction are keyed by entity id and
/// `hurt_animation` addresses the local player by its own id, so both live in
/// the table's side maps regardless.
///
/// **`hurtTime` is the raw counter minus the partial tick**, which is why the
/// value handed on is a float and can be negative: `bobHurt` uses that
/// negativity as its own guard rather than clamping.
pub(super) fn local_hurt_tilt(session: &PlaySession, alpha: f32, strength: f32) -> HurtTilt {
    let Some(id) = session.player_id else {
        return HurtTilt::NONE;
    };
    let h = session.world.entities.hurt_state(id);
    HurtTilt {
        hurt_time: h.hurt_time as f32 - alpha,
        // Raw, not clamped: an unhurt entity's 0 is vanilla's 0, and
        // `bob_hurt` guards the division rather than inventing a divisor.
        hurt_duration: h.hurt_duration,
        hurt_dir: session.world.entities.hurt_dir(id),
        strength,
    }
}

/// M284 — the camera a frame renders from (`Camera.alignWithEntity`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CameraPose {
    pub(super) eye: [f64; 3],
    pub(super) yaw: f32,
    pub(super) pitch: f32,
}

impl CameraPose {
    pub(super) fn eye_vec(&self) -> Vec3 {
        Vec3::new(self.eye[0] as f32, self.eye[1] as f32, self.eye[2] as f32)
    }
}

/// `Avatar.POSES`' SLEEPING eye height (`EntityDimensions.fixed(0.2F,
/// 0.2F).withEyeHeight(0.2F)`).
pub(super) const SLEEPING_EYE_HEIGHT: f64 = 0.2f32 as f64;
/// `alignWithEntity`'s `move(0.0F, 0.15F, 0.0F)` for a sleeper: at pitch 0
/// the camera's up is the world's.
pub(super) const SLEEPING_CAMERA_LIFT: f64 = 0.15f32 as f64;

/// M284 — `setRotation(bedOrientation != null ? bedOrientation.toYRot() -
/// 180.0F : 0.0F, 0.0F)`'s yaw for a bed's `facing` (`Direction.toYRot`:
/// south 0, west 90, north 180, east 270); no bed gives 0.
pub(super) fn sleeping_camera_yaw(facing: Option<&str>) -> f32 {
    let rot = match facing {
        Some("south") => 0.0,
        Some("west") => 90.0,
        Some("north") => 180.0,
        Some("east") => 270.0,
        _ => return 0.0,
    };
    rot - 180.0
}

/// M284 — this frame's camera. Awake it is the player's eye and look; asleep
/// (`isSleeping`, a sleeping position set) `alignWithEntity` turns it to face
/// out of the bed's head with the pitch levelled, at the sleeping eye height
/// lifted 0.15. `BedBlock.getBedOrientation` reads `FACING` only from a bed
/// block (a sleeping position on anything else turns the camera to 0).
///
/// Only the render camera moves: the crosshair's pick uses the player's eye
/// and view vector (`Entity.pick`), as vanilla's does. Vanilla's camera also
/// eases its eye height half the gap a tick (`Camera.tick`); Rewo's camera
/// does not ease, so the drop into the bed is immediate.
///
/// M361 — and the position is the render position: last tick's start
/// blended into this tick's end by the partial tick `alpha`
/// ([`render_feet`]), as `Camera.setup` lerps `xo → getX()`. Until M361 the
/// camera stood on the tick position, so it moved twenty times a second
/// while every entity it looked at moved every frame: riding, the mount
/// (blended) shook against the view (stepped).
pub(super) fn camera_pose(
    session: &PlaySession,
    blocks: &rewo_data::blocks::Blocks,
    alpha: f32,
) -> CameraPose {
    let p = &session.player;
    let [x, y, z] = render_feet(session.player_prev, [p.x, p.y, p.z], alpha);
    if session.is_sleeping() {
        let facing = session.sleeping_pos().and_then(|(x, y, z)| {
            let st = session.world.block_state_at(x, y, z);
            blocks
                .block_name(st)
                .filter(|n| n.ends_with("_bed"))
                .and_then(|_| blocks.property(st, "facing"))
        });
        return CameraPose {
            eye: [x, y + SLEEPING_EYE_HEIGHT + SLEEPING_CAMERA_LIFT, z],
            yaw: sleeping_camera_yaw(facing),
            pitch: 0.0,
        };
    }
    CameraPose {
        eye: [x, y + (p.eye_y() - p.y), z],
        yaw: p.yaw,
        pitch: p.pitch,
    }
}

/// M361 — the local player's render position: `prev` (the tick's start)
/// blended toward `cur` (its end) by the partial tick, `Mth.lerp(partial,
/// xo, x)` per axis.
pub(super) fn render_feet(prev: [f64; 3], cur: [f64; 3], alpha: f32) -> [f64; 3] {
    let a = f64::from(alpha.clamp(0.0, 1.0));
    [0, 1, 2].map(|i| prev[i] + (cur[i] - prev[i]) * a)
}

pub(super) fn player_eye(session: &PlaySession) -> Vec3 {
    Vec3::new(
        session.player.x as f32,
        session.player.eye_y() as f32,
        session.player.z as f32,
    )
}

/// Is a finished mesh from a world that no longer exists?
///
/// A dimension change bumps `PlaySession::dimension_generation`, and jobs
/// meshed from the old world can still be in flight. Such an output must be
/// a pure no-op: it may not upload (its geometry belongs to another world)
/// and it may not *remove* either, because the same (cx, cz) may already
/// hold a freshly uploaded column from the current dimension.
pub(super) fn mesh_output_is_stale(out_generation: u64, current_generation: u64) -> bool {
    out_generation != current_generation
}

/// How long swapchain/depth recreate failures are tolerated before the
/// client gives up: this many in a row AND at least [`RECREATE_GIVE_UP_S`]
/// since the first of them. A recreate fails transiently (a minimise or
/// restore, a display or driver hiccup, a surface not yet ready) and the next
/// frame's `render` asks again via `NeedsRecreate`, so exiting on the first
/// failure closed the client for a recoverable glitch.
pub(super) const RECREATE_GIVE_UP_FAILURES: u32 = 10;
pub(super) const RECREATE_GIVE_UP_S: f32 = 2.0;

/// Counts consecutive recreate failures. Pure: the caller supplies the clock.
#[derive(Default, Debug)]
pub(super) struct RecreateGuard {
    failures: u32,
    first_failure_s: f32,
}

impl RecreateGuard {
    /// A recreate (or a clean frame) succeeded.
    pub(super) fn ok(&mut self) {
        self.failures = 0;
    }

    /// A recreate failed at `now_s`; `true` = give up and exit. Each failure
    /// is the caller's to log.
    pub(super) fn failed(&mut self, now_s: f32) -> bool {
        if self.failures == 0 {
            self.first_failure_s = now_s;
        }
        self.failures += 1;
        self.failures >= RECREATE_GIVE_UP_FAILURES
            && now_s - self.first_failure_s >= RECREATE_GIVE_UP_S
    }
}

/// What to do with a finished, non-stale mesh job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MeshOutcome {
    /// Upload the new mesh.
    Upload,
    /// The column baked to nothing, or is gone: free its GPU buffers.
    Remove,
    /// The mesher panicked: leave whatever the column already shows.
    KeepExisting,
}

/// A panic is NOT "baked to nothing": `MeshOutput::mesh` is `None` for both,
/// and treating them alike (`remove_column`) let one panic blank a column
/// that was rendering fine.
pub(super) fn classify_mesh_output(panicked: bool, has_mesh: bool, gone: bool) -> MeshOutcome {
    if panicked {
        MeshOutcome::KeepExisting
    } else if has_mesh && !gone {
        MeshOutcome::Upload
    } else {
        MeshOutcome::Remove
    }
}

/// Per-frame mesh pump. Frees removed columns, uploads up to
/// `upload_budget` finished meshes from the worker pool, then submits
/// One log line every 10 s with the terrain arena's fill and how many columns
/// wait for space — the measurement the fixed `MAX_VERTS` / `MAX_INDICES` caps
/// need, since a deferred column is a hole in the world.
fn log_arena_fill(wr: &WorldRenderer) {
    thread_local! {
        static LAST: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    }
    let due = LAST.with(|l| {
        let now = Instant::now();
        let due = l
            .get()
            .is_none_or(|t| now.duration_since(t).as_secs() >= 10);
        if due {
            l.set(Some(now));
        }
        due
    });
    if !due {
        return;
    }
    let ((v, vmax), (i, imax)) = wr.arena_usage();
    log::info!(
        "world: arena {v}/{vmax} verts ({:.0}%), {i}/{imax} indices ({:.0}%), {} columns, {} deferred",
        v as f64 * 100.0 / vmax as f64,
        i as f64 * 100.0 / imax as f64,
        wr.column_count(),
        wr.deferred_column_count()
    );
}

/// fresh snapshots for dirty columns. Returns how many meshes uploaded.
pub(super) fn pump_meshing(
    session: &mut PlaySession,
    gpu: &mut Gpu,
    world_renderer: &mut WorldRenderer,
    pool: &mut MeshPool,
    upload_budget: usize,
    // M164 — raised to the largest `carried_fluid_cells` seen. Read by `r48`;
    // ignored otherwise.
    carried_fluid_cells: &mut u32,
) -> Result<usize, String> {
    for (cx, cz) in session.take_removed() {
        world_renderer.remove_column(gpu, cx, cz);
    }
    // Columns that did not fit the arena go back in once space frees; columns
    // whose upload batch failed to submit are re-meshed.
    world_renderer.retry_deferred_columns(gpu)?;
    log_arena_fill(world_renderer);
    let lost = world_renderer.take_lost_columns();
    if !lost.is_empty() {
        session.requeue_dirty(lost);
    }

    // Drain finished jobs — uploads are metered, removals are free.
    let mut uploaded = 0;
    while uploaded < upload_budget {
        let Some(out) = pool.try_recv() else { break };
        // Dimension changed while its job was in flight → drop it whole,
        // before any gone/upload/remove decision: the coords mean nothing in
        // the new world, and removing them could free a current column.
        if mesh_output_is_stale(out.generation, session.dimension_generation) {
            continue;
        }
        // Column forgotten while its job was in flight → don't resurrect it.
        let gone = session.world.column(out.cx, out.cz).is_none();
        let retry = pool.retry_after_panic(&out);
        match classify_mesh_output(out.panicked, out.mesh.is_some(), gone) {
            MeshOutcome::KeepExisting => {
                // The mesher panicked: the column's previous mesh is still
                // the best picture of it, so it stays. One retry (the pool
                // decides), then nothing until the column changes.
                if retry {
                    session.requeue_dirty(vec![(out.cx, out.cz)]);
                }
            }
            MeshOutcome::Upload => {
                let Some(mesh) = out.mesh else { continue };
                *carried_fluid_cells = (*carried_fluid_cells).max(mesh.carried_fluid_cells);
                world_renderer.upload_column(
                    gpu,
                    out.cx,
                    out.cz,
                    bytemuck::cast_slice(&mesh.vertices),
                    &mesh.indices,
                    bytemuck::cast_slice(&mesh.tvertices),
                    &mesh.tindices,
                    mesh.y_min,
                    mesh.y_max,
                )?;
                uploaded += 1;
            }
            MeshOutcome::Remove => world_renderer.remove_column(gpu, out.cx, out.cz),
        }
    }

    // Submit dirty columns, nearest-to-player first so what you're looking
    // at appears soonest. A column already in flight stays dirty and
    // resubmits after its result lands (per-column ordering — see
    // `rewo_mesh::pool`).
    let mut dirty = session.take_dirty();
    if !dirty.is_empty() {
        let (px, pz) = (session.player.x as f32, session.player.z as f32);
        sort_columns_nearest_first(&mut dirty, px, pz);
        let mut deferred = Vec::new();
        for (cx, cz) in dirty {
            if session.world.column(cx, cz).is_none() {
                // Nothing to mesh (matches the old `None` arm's removal).
                world_renderer.remove_column(gpu, cx, cz);
                continue;
            }
            if !pool.submit(session.dimension_generation, &session.world, cx, cz) {
                deferred.push((cx, cz));
            }
        }
        session.requeue_dirty(deferred);
    }
    Ok(uploaded)
}

/// Order columns nearest-to-`(px, pz)` first. `total_cmp`, not
/// `partial_cmp().unwrap()`: a non-finite player position (a hostile or
/// corrupt teleport) makes every distance NaN, and a panic in the frame loop
/// is the worst way to find that out. NaN distances order by their bits
/// (all to one end), which is harmless.
pub(super) fn sort_columns_nearest_first(cols: &mut [(i32, i32)], px: f32, pz: f32) {
    cols.sort_by(|a, b| col_dist(*a, px, pz).total_cmp(&col_dist(*b, px, pz)));
}

pub(super) fn col_dist((cx, cz): (i32, i32), px: f32, pz: f32) -> f32 {
    let x = cx as f32 * 16.0 + 8.0 - px;
    let z = cz as f32 * 16.0 + 8.0 - pz;
    x * x + z * z
}

// -- headless ---------------------------------------------------------------

/// M131 — the sound model, built from the asset store when it is unpacked.
///
/// **No device is opened unless `audio` is set**, which needs both the
/// `--audio` flag and a binary built `--features audio` (M143). Without one
/// `LiveSounds` carries only its `SilentDevice`, and what runs is vanilla's
/// resolution, gain/pitch/attenuation arithmetic, channel budget and reclaim
/// clock — so the pipeline M63/M64/M66 built is exercised on the live path
/// instead of only in tests. Before this, `PlaySession::take_sound_events` had
/// **no caller anywhere** and the decoded queue filled to its cap and rotated
/// forever.
///
/// **A failure to open is logged, not fatal.** A client whose speakers are
/// unplugged is still a client, and every gate runs this path deliberately. The
/// error carries whether the binary *could* have made a sound at all, which is
/// a different problem from a device that will not open.
///
/// A missing asset store is not an error **in an ordinary run**: an empty index
/// makes every event resolve to `UnknownEvent`, which is silence, and a machine
/// with no unpacked assets should still be able to fly around a world.
///
/// **Under `--render-check` it is fatal, and that asymmetry is the point.** An
/// empty index is behaviourally identical to totally broken resolution — every
/// sound resolves to nothing, every counter reads zero, and the run is green
/// because it asserted nothing. `sounds_json`'s own test module already records
/// this trap being found by a mutation battery: replacing the whole loader with
/// an empty index left a test green *because it merely SKIPped*. A gate that
/// degrades to a no-op on the one machine where it matters is worse than no
/// gate, so the strict path fails closed and says which file it wanted.
/// `pub(crate)` so `soundshot` can drive the **production** loader rather than
/// a hand-assembled index — M45's `install_shapes` rule, that a gate
/// reimplementing a slice of the app's setup misses whatever the app adds to it.
pub(crate) fn build_sounds(
    version: &str,
    registry: &rewo_data::sound_events::SoundEvents,
    strict: bool,
    audio: bool,
) -> LiveSounds {
    let sounds = match rewo_data::sounds_json::load_for_version(version) {
        Ok(idx) => idx,
        Err(e) if strict => {
            panic!(
                "live --render-check: no sounds.json ({e}). The sound witnesses                  cannot distinguish an empty index from broken resolution, so                  this fails rather than passing vacuously. Unpack the {version}                  assets, or run without --render-check."
            )
        }
        Err(e) => {
            log::info!("live: no sounds.json ({e}); the sound model will resolve nothing");
            rewo_data::sounds_json::SoundsIndex::new()
        }
    };
    let mut live = LiveSounds::new(sounds, registry.clone());
    // M157 — seed the frequency from `options.txt`, as a PULL.
    //
    // Vanilla does the same and has to: `OptionInstance.set` skips its
    // `onValueUpdate` while `!Minecraft.isRunning()`, so loading the file at
    // start-up fires no callbacks, and `MusicManager`'s constructor reads the
    // option itself (`MusicManager.java:30`). A client that waited for the
    // callback would start every session at DEFAULT whatever the file said.
    let opts = load_options();
    live.set_music_frequency(opts.music_frequency);
    // M173 — seed the eleven volume sliders the same way: the store WITHOUT
    // the refresh (file load fires no callbacks — the M161 rule; nothing is
    // playing yet anyway).
    for source in rewo_net::sounds::SoundSource::ALL {
        live.seed_category_volume(source, opts.sound_volume(source));
    }
    if audio {
        attach_backend(&mut live, crate::audio_backend::open(version));
    }
    live
}

/// Attach an opened backend, or say why there is none.
///
/// **Split out from `build_sounds` so it has a seam.** Whether `open` succeeds
/// depends on a real device, and no test in this project opens one — so a test
/// can never reach the success path *through* `build_sounds`. Taking the
/// already-resolved `Result` moves the only interesting decision (attach on
/// `Ok`, complain on `Err`) into something a fake backend can drive, and leaves
/// the untestable part as the single call to `open` above. M97's lesson: logic
/// living where no test can reach it is untestable, so move the logic.
pub(super) fn attach_backend(live: &mut LiveSounds, opened: Result<Box<dyn ChannelSink>, String>) {
    match opened {
        Ok(sink) => live.attach_sink(sink),
        // ERROR rather than warn: the user asked for audio by name, and a
        // silent downgrade is exactly the "green but nothing happened" outcome
        // this subsystem cannot afford — nothing downstream can tell a client
        // with no backend from a device that is muted.
        Err(e) => log::error!("audio: --audio was requested and no device was opened: {e}"),
    }
}

/// Whether this run wants audio: the flag, or `REWO_AUDIO=1`.
///
/// The env fallback follows `--wavy-cape`/`REWO_WAVY_CAPE`, and exists for the
/// same reason: the launcher spawns `rewo` with a fixed argument list, so a
/// per-session toggle has to arrive through the environment.
pub(super) fn wants_audio(args: &LiveArgs) -> bool {
    args.audio
        || std::env::var("REWO_AUDIO")
            .map(|v| v == "1")
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M197: the flow array's contract, where the three crates meet — the
    /// layer bit the mesher sets is the one both terrain shaders test, and the
    /// bake's flow frame size is the renderer's array size.
    #[test]
    fn the_flow_layer_contract_holds_across_the_crates() {
        assert_eq!(rewo_mesh::FLOW_LAYER_BIT, 0x8000);
        for (name, src) in [
            (
                "world.frag",
                include_str!("../../../rewo-gpu/shaders/world.frag"),
            ),
            (
                "water.frag",
                include_str!("../../../rewo-gpu/shaders/water.frag"),
            ),
        ] {
            assert!(
                src.contains("(v_layer & 0x8000u) != 0u") && src.contains("v_layer & 0x7FFFu"),
                "{name} does not test the flow bit"
            );
        }
        assert_eq!(
            rewo_data::assets::FLOW_TEX_SIZE,
            rewo_gpu::world::FLOW_TEX_SIZE
        );
    }

    /// M197: `Mth.atan2` exists twice (rewo-world for the mesher, rewo-gpu for
    /// the locator bar, which cannot see each other); they agree to the bit.
    #[test]
    fn the_two_mth_atan2_ports_agree() {
        for i in -40..=40 {
            for j in -40..=40 {
                let (y, x) = (f64::from(i) / 7.0, f64::from(j) / 3.0);
                let a = rewo_world::lightmap::mth_atan2(y, x);
                let b = rewo_gpu::locator_bar::mth_atan2(y, x);
                assert!(
                    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()),
                    "{y} {x}"
                );
            }
        }
    }

    /// `pump_meshing` hands `rewo-mesh`'s vertices to `rewo-gpu` as bytes, and
    /// the terrain arena counts vertices as `bytes / world::VERTEX_STRIDE`
    /// with a pipeline stride of the same constant. The two crates cannot see
    /// each other, so the size contract is pinned here, where both meet.
    #[test]
    fn mesh_vertex_size_is_the_world_vertex_stride() {
        assert_eq!(
            std::mem::size_of::<rewo_mesh::MeshVertex>() as u64,
            rewo_gpu::world::VERTEX_STRIDE
        );
    }

    /// A mesher panic keeps the column's existing mesh; only a real empty
    /// bake (or a forgotten column) frees it.
    #[test]
    fn a_panic_keeps_the_existing_mesh_and_an_empty_bake_removes_it() {
        assert_eq!(
            classify_mesh_output(true, false, false),
            MeshOutcome::KeepExisting
        );
        assert_eq!(
            classify_mesh_output(false, false, false),
            MeshOutcome::Remove
        );
        assert_eq!(classify_mesh_output(false, true, true), MeshOutcome::Remove);
        assert_eq!(
            classify_mesh_output(false, true, false),
            MeshOutcome::Upload
        );
    }

    #[test]
    fn a_transient_recreate_failure_is_retried_not_fatal() {
        let mut g = RecreateGuard::default();
        // A burst of failures inside the grace window never gives up...
        for i in 0..(RECREATE_GIVE_UP_FAILURES * 3) {
            assert!(!g.failed(i as f32 * 0.001), "failure {i}");
        }
        // ...and one success forgets them all.
        g.ok();
        for i in 0..RECREATE_GIVE_UP_FAILURES - 1 {
            assert!(!g.failed(100.0 + i as f32));
        }
    }

    #[test]
    fn a_persistent_recreate_failure_gives_up_after_the_count_and_the_time() {
        let mut g = RecreateGuard::default();
        let step = RECREATE_GIVE_UP_S / RECREATE_GIVE_UP_FAILURES as f32 * 1.5;
        let mut gave_up_at = None;
        for i in 0..(RECREATE_GIVE_UP_FAILURES * 2) {
            if g.failed(5.0 + i as f32 * step) {
                gave_up_at = Some(i + 1);
                break;
            }
        }
        let n = gave_up_at.expect("a persistent failure must end the client");
        assert!(n >= RECREATE_GIVE_UP_FAILURES, "not before the count");
    }

    #[test]
    fn nearest_first_orders_finite_positions() {
        let mut v = vec![(5, 5), (0, 0), (-1, 0), (3, -3)];
        sort_columns_nearest_first(&mut v, 8.0, 8.0);
        // Column (0,0) is centred on (8,8): distance 0.
        assert_eq!(v[0], (0, 0));
        let d: Vec<f32> = v.iter().map(|&c| col_dist(c, 8.0, 8.0)).collect();
        assert!(d.windows(2).all(|w| w[0] <= w[1]), "{d:?}");
    }

    #[test]
    fn nearest_first_tolerates_non_finite_player() {
        // Every distance is NaN (or inf-inf): the old `partial_cmp().unwrap()`
        // panicked here. Order is unspecified, but the sort must return and
        // keep every column.
        for (px, pz) in [
            (f32::NAN, 0.0),
            (0.0, f32::NAN),
            (f32::INFINITY, f32::NEG_INFINITY),
        ] {
            let mut v = vec![(5, 5), (0, 0), (-1, 0), (3, -3)];
            sort_columns_nearest_first(&mut v, px, pz);
            v.sort();
            assert_eq!(v, vec![(-1, 0), (0, 0), (3, -3), (5, 5)]);
        }
    }

    /// M284 — `alignWithEntity`'s sleeper: `toYRot() - 180` per bed facing
    /// (south 0, west 90, north 180, east 270), 0 with no bed, and the eye
    /// at the SLEEPING pose's 0.2 lifted 0.15.
    #[test]
    fn the_sleeping_camera_faces_out_of_the_bed() {
        assert_eq!(sleeping_camera_yaw(Some("south")), -180.0);
        assert_eq!(sleeping_camera_yaw(Some("west")), -90.0);
        assert_eq!(sleeping_camera_yaw(Some("north")), 0.0);
        assert_eq!(sleeping_camera_yaw(Some("east")), 90.0);
        assert_eq!(sleeping_camera_yaw(None), 0.0);
        assert_eq!(sleeping_camera_yaw(Some("up")), 0.0);
        let dy = SLEEPING_EYE_HEIGHT + SLEEPING_CAMERA_LIFT;
        assert!((dy - 0.35).abs() < 1e-6, "{dy}");
        // The look a west-facing bed gives points east (+x): the sleeper's
        // head is at the bed's head and the view runs down its length.
        let dir = look_dir(sleeping_camera_yaw(Some("west")), 0.0);
        assert!(
            (dir[0] - 1.0).abs() < 1e-6 && dir[2].abs() < 1e-6,
            "{dir:?}"
        );
    }
}
