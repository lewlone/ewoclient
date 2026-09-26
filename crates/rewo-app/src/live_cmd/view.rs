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

pub(crate) fn hud_sprites(baked: &assets::BakedAssets) -> Option<rewo_gpu::hud::HudSpritesData<'_>> {
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
    baked
        .animations
        .iter()
        .map(|a| rewo_gpu::world::LayerAnimation {
            layer: a.layer,
            frames: a.frames.clone(),
            order: a.order.clone(),
            frametime: a.frametime,
        })
        .collect()
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
    Mat4::look_to_rh(eye, dir, Vec3::Y)
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

/// Per-frame mesh pump. Frees removed columns, uploads up to
/// `upload_budget` finished meshes from the worker pool, then submits
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
        match out.mesh {
            Some(mesh) if !gone => {
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
            _ => world_renderer.remove_column(gpu, out.cx, out.cz),
        }
    }

    // Submit dirty columns, nearest-to-player first so what you're looking
    // at appears soonest. A column already in flight stays dirty and
    // resubmits after its result lands (per-column ordering — see
    // `rewo_mesh::pool`).
    let mut dirty = session.take_dirty();
    if !dirty.is_empty() {
        let (px, pz) = (session.player.x as f32, session.player.z as f32);
        dirty.sort_by(|a, b| {
            let da = col_dist(*a, px, pz);
            let db = col_dist(*b, px, pz);
            da.partial_cmp(&db).unwrap()
        });
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
    args.audio || std::env::var("REWO_AUDIO").map(|v| v == "1").unwrap_or(false)
}
