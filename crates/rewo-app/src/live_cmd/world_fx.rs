use super::*;

/// Everything the live client needs to draw weather, built once.
///
/// The cloud mesh is **cached across frames**: it is position-independent (the
/// per-frame motion rides in the uniform), so vanilla rebuilds it only when the
/// camera crosses a cell boundary or changes which side of the deck it is on.
/// This mirrors `CloudRenderer`'s own `needsRebuild` / `prevCellX` bookkeeping.
pub struct WeatherAssets {
    pub(super) clouds: Option<rewo_gpu::clouds::CloudTexture>,
    /// The three `Biome` world-gen noises, built once per client as vanilla
    /// builds them once per JVM.
    pub(super) noise: rewo_world::weather::ClimateNoise,
    pub(super) directions: rewo_gpu::weather::ColumnDirections,
    /// The cached mesh and the state it was built for.
    pub(super) cached: Option<(Vec<[i32; 3]>, i32, i32, rewo_gpu::clouds::RelativeCameraPos)>,
    /// The eased rain-fog multiplier — state, because it ramps over ~5 ticks
    /// rather than following the rain level directly.
    pub(super) rain_fog: rewo_world::weather::RainFog,
}

impl WeatherAssets {
    pub fn new(baked: &assets::BakedAssets) -> Self {
        let clouds = baked.clouds.as_ref().map(|img| {
            rewo_gpu::clouds::CloudTexture::from_rgba(&img.rgba, img.w, img.h)
        });
        if clouds.is_none() {
            log::warn!("live: no environment/clouds.png in the jar bake — no cloud deck");
        }
        Self {
            clouds,
            noise: rewo_world::weather::ClimateNoise::new(),
            directions: rewo_gpu::weather::ColumnDirections::new(),
            cached: None,
            rain_fog: rewo_world::weather::RainFog::default(),
        }
    }
}

/// `weatherRadius`, vanilla's video option. Must stay ≤ 16: the 32×32 direction
/// table cannot address a column further out than that.
pub(super) const WEATHER_RADIUS: i32 = 10;
/// `cloudRange` — vanilla derives it from the render distance; Rewo pins it
/// rather than plumbing a video-options struct for one number.
pub(super) const CLOUD_RANGE_CHUNKS: i32 = 12;
/// `EnvironmentAttributes.FOG_START_DISTANCE` / `FOG_END_DISTANCE` defaults.
///
/// The rain offsets are applied to *these*, not to Rewo's own fog band. The two
/// are different things: Rewo's `set_fog` band is a render-distance fade that
/// dissolves the chunk edge into the sky, and vanilla's `total_fog_value` is
/// the `max` of that and a separate **environmental** term. Only the
/// environmental one is what rain thickens, which is why applying the offsets
/// to Rewo's tight band made rain half-fog the air ten blocks from the camera.
///
/// Neither built-in dimension overrides them, so the attribute defaults are the
/// real values; reading them per-dimension is a small follow-up.
pub(super) const ENV_FOG_START: f32 = 0.0;
pub(super) const ENV_FOG_END: f32 = 1024.0;

/// `FogCloudsEnd`, the distance at which the deck has faded out completely.
///
/// **An approximation, not a transcription.** Vanilla's comes from
/// `FogRenderer`, which Rewo does not have. It must comfortably exceed the
/// mesh's own reach or the deck is culled by its own fade — the furthest cell
/// is ~192 blocks out horizontally, and the deck can sit a couple of hundred
/// blocks overhead as well (192.33 above a y=-60 flat world is 250-odd). Set
/// too tight, clouds simply never appear; the first live shot did exactly that.
pub(super) const CLOUD_FOG_END: f32 = 1024.0;

/// Build the two passes. Clouds need no texture (the shader carries its six
/// face colours inline); rain and snow need both of theirs, and a missing one
/// means that precipitation simply does not draw.
pub(super) fn init_weather_if_present(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    wr.init_clouds(gpu)?;
    match (&baked.rain, &baked.snow) {
        (Some(rain), Some(snow)) => wr.init_weather(
            gpu,
            &rewo_gpu::weather::WeatherImage {
                rgba: &rain.rgba,
                w: rain.w,
                h: rain.h,
            },
            &rewo_gpu::weather::WeatherImage {
                rgba: &snow.rgba,
                w: snow.w,
                h: snow.h,
            },
        )?,
        _ => log::warn!("live: no rain/snow texture in the jar bake — no precipitation"),
    }
    match &baked.forcefield {
        Some(tex) => wr.init_border(
            gpu,
            &rewo_gpu::border::BorderImage {
                rgba: &tex.rgba,
                w: tex.w,
                h: tex.h,
            },
        )?,
        None => log::warn!("live: no forcefield.png in the jar bake — no world-border wall"),
    }
    Ok(())
}

/// Rewo's own render distance, in chunks — the `local` half of
/// `Options.getEffectiveRenderDistance`. The server's cap is the other half and
/// arrives on `set_chunk_cache_radius`.
pub(super) const LOCAL_RENDER_DISTANCE_CHUNKS: i32 = 12;

/// This frame's world-border wall (M80).
///
/// `renderDistance` is `getEffectiveRenderDistance() * 16` and `depthFar` is
/// `Camera.update`'s `max(renderDistance * 4, cloudRange * 16)` — the wall's
/// half-height is literally the camera's far plane, so it always spans the
/// view vertically.
pub(super) fn apply_border(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    session: &PlaySession,
    partial_ticks: f32,
) {
    if !wr.border_ready() {
        return;
    }
    let eye = eye_f64(session);
    let render_distance =
        (session.view_area.effective_render_distance(LOCAL_RENDER_DISTANCE_CHUNKS) * 16) as f64;
    let depth_far = (render_distance as f32 * 4.0).max((CLOUD_RANGE_CHUNKS * 16) as f32);
    let extracted = session
        .border
        .extract(partial_ticks, eye[0], eye[2], render_distance)
        .map(|r| rewo_gpu::border::BorderState {
            min_x: r.min_x,
            max_x: r.max_x,
            min_z: r.min_z,
            max_z: r.max_z,
            tint: r.tint,
            alpha: r.alpha,
        });
    // The scroll is wall-clock, not tick-derived — `Util.getMillis()`, which is
    // `System.nanoTime() / 1_000_000`. A monotonic clock, so a wrapping `as
    // u64` of the elapsed millis is the same modulo-3000 sequence.
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let draw = extracted.map(|state| {
        rewo_gpu::border::BorderDraw::build(&state, eye, render_distance, depth_far, millis)
    });
    if let Err(e) = wr.set_border(gpu, draw.as_ref()) {
        log::warn!("live: border upload failed: {e}");
    }
}

/// This frame's cloud deck and precipitation.
///
/// Both are skipped cheaply when they cannot apply: a dimension whose
/// `cloud_color` alpha is zero gets no cloud draw at all (that is how the
/// Nether and the End have none), and a rain level of zero extracts no columns.
/// Headless-only knob: `REWO_FORCE_WEATHER=<rain>[,<thunder>]` overrides the
/// session's levels so the live weather path can be shot without an op'd bot
/// running `/weather`. The same shape as `REWO_FORCE_GESTURE` and `REWO_SUMMON`.
pub(super) fn forced_weather() -> Option<(f32, f32)> {
    // Read once: this runs several times per frame, and the knob is fixed at
    // startup (render-check sets it before the event loop starts).
    static FORCED: std::sync::OnceLock<Option<(f32, f32)>> = std::sync::OnceLock::new();
    *FORCED.get_or_init(parse_forced_weather)
}

pub(super) fn parse_forced_weather() -> Option<(f32, f32)> {
    let raw = crate::knobs::var("REWO_FORCE_WEATHER").ok()?;
    let mut parts = raw.split(',');
    let rain: f32 = parts.next()?.trim().parse().ok()?;
    let thunder: f32 = parts.next().and_then(|t| t.trim().parse().ok()).unwrap_or(0.0);
    Some((rain.clamp(0.0, 1.0), thunder.clamp(0.0, 1.0)))
}

/// The weather state this frame draws — the session's, unless the headless
/// knob overrides it.
pub(super) fn effective_weather(session: &PlaySession) -> rewo_world::weather::WeatherState {
    let mut w = session.weather;
    if let Some((rain, thunder)) = forced_weather() {
        w.set_rain(rain);
        w.set_thunder(thunder);
    }
    w
}

#[allow(clippy::too_many_arguments)]

// ---------------------------------------------------------------------------
// Particles (M37)
// ---------------------------------------------------------------------------

/// The live particle system plus the layer bookkeeping the pass needs.
///
/// The texture array the pass samples is the block textures followed by the
/// particle sprites, so a terrain shard (which must sample the *block*
/// texture, per `getParticleMaterial`) and a flame share one pipeline.
/// `sprite_base` is where the sprites start.
pub struct ParticleAssets {
    pub(super) sys: rewo_world::particles::ParticleSystem,
    pub(super) sprite_base: u32,
    /// Per kind: first layer and frame count, resolved once from the bake.
    pub(super) sets: Vec<(rewo_world::particles::ParticleKind, u32, u32)>,
    /// Last session tick the system was advanced for, so the 20 Hz simulation
    /// steps exactly once per game tick however fast frames arrive.
    ///
    /// `None` until the first frame that runs it: the session has usually
    /// ticked for several seconds by then (connect, chunk load, settle), and
    /// anchoring at 0 would make the first frame fast-forward every one of
    /// those ticks at once — which ages a whole burst past its lifetime before
    /// it is ever drawn.
    pub(super) last_tick: Option<u64>,
}

impl ParticleAssets {
    pub fn new(baked: &assets::BakedAssets) -> Option<Self> {
        use rewo_world::particles::ParticleKind as K;
        let sprites = baked.particles.as_ref()?;
        let sprite_base = baked.layers.len() as u32;
        let mut sets = Vec::new();
        for (kind, name) in [
            (K::Flame, "flame"),
            (K::Crit, "crit"),
            (K::Splash, "splash"),
            (K::Smoke, "smoke"),
            (K::Poof, "poof"),
        ] {
            let (off, n) = sprites.set(name)?;
            sets.push((kind, sprite_base + off, n));
        }
        Some(Self {
            // A fixed seed: the run is reproducible, which is the property the
            // M37 gate rests on. Vanilla's per-particle seeds are arbitrary, so
            // any seed is an equally valid vanilla outcome (REWO_PLAN §15, M37).
            sys: rewo_world::particles::ParticleSystem::new(0x5EED_1234),
            sprite_base,
            sets,
            last_tick: None,
        })
    }

    pub(super) fn layer_for(&self, kind: rewo_world::particles::ParticleKind, frame: u32) -> Option<u32> {
        self.sets
            .iter()
            .find(|(k, _, _)| *k == kind)
            .map(|(_, off, n)| off + frame.min(n.saturating_sub(1)))
    }
}

/// Build the combined texture array and hand it to the pass.
pub(super) fn init_particles_if_present(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    let Some(sprites) = baked.particles.as_ref() else {
        log::warn!("live: no particle sprites in the jar bake — no particles");
        return Ok(());
    };
    let mut layers: Vec<Vec<u8>> = baked.layers.clone();
    layers.extend(sprites.layers.iter().cloned());
    wr.init_particles(
        gpu,
        &rewo_gpu::particles::ParticleAtlas {
            layers: &layers,
            size: assets::TEX_SIZE,
        },
    )
}

/// Build the block-break crumbling pass from the jar's ten stage textures
/// (M81). A jar without them simply draws no cracks.
pub(super) fn init_crumbling_if_present(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    let Some(stages) = baked.destroy_stages.as_ref() else {
        log::warn!("live: no destroy_stage textures in the jar bake — no block-break overlay");
        return Ok(());
    };
    let size = stages[0].w.max(1);
    let layers: Vec<Vec<u8>> = stages.iter().map(|s| s.rgba.clone()).collect();
    wr.init_crumbling(gpu, &layers, size)
}

/// This frame's block-break decals (M81).
///
/// `extractBlockDestroyAnimation`'s two rules, both here: the **highest**
/// progress at a position wins (the store resolves that), and a position
/// further than 32 blocks from the camera is skipped — `distToCenterSqr(camX,
/// camY, camZ) > 1024.0`, measured from the block's **centre**, not its
/// corner.
pub(super) fn apply_crumbling(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    session: &PlaySession,
    baked: &assets::BakedAssets,
    eye: Vec3,
) {
    use rewo_gpu::crumbling::CrumblingVertex;
    let mut verts: Vec<CrumblingVertex> = Vec::new();
    for (pos, stage) in session.world.destruction.iter() {
        let (cx, cy, cz) = (
            pos[0] as f32 + 0.5,
            pos[1] as f32 + 0.5,
            pos[2] as f32 + 0.5,
        );
        let d2 = (cx - eye.x).powi(2) + (cy - eye.y).powi(2) + (cz - eye.z).powi(2);
        if d2 > 1024.0 {
            continue;
        }
        let state = session
            .world
            .block_state_at(pos[0], pos[1], pos[2]);
        for q in rewo_mesh::crumbling::block_decal_quads(
            &baked.render,
            &baked.models,
            state,
            pos,
        ) {
            let v = |i: usize| CrumblingVertex {
                pos: q.verts[i],
                uv: q.uv[i],
                stage: stage as u32,
            };
            // Two triangles, the same 0-1-2 / 0-2-3 winding the mesher uses.
            verts.extend_from_slice(&[v(0), v(1), v(2), v(0), v(2), v(3)]);
        }
    }
    if let Err(e) = wr.set_crumbling(gpu, &verts) {
        log::warn!("live: crumbling upload: {e}");
    }
}

/// Drain the frame's spawn requests, advance the simulation on the game tick,
/// and hand the renderer this frame's quads.
///
/// The simulation steps on `session.ticks` rather than on frame time: vanilla's
/// `ParticleEngine.tick` runs once per 20 Hz client tick, and driving it from
/// the frame rate would make particles fall faster on a faster machine.
pub(super) fn apply_particles(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    session: &PlaySession,
    // Drained by the caller, so this takes the session immutably and does not
    // fight the frame's other borrows of it.
    events: Vec<rewo_world::particles::ParticleEvent>,
    p: &mut ParticleAssets,
    baked: &assets::BakedAssets,
    partial_ticks: f32,
    view: [[f32; 4]; 4],
) {
    use rewo_world::particles::{ParticleEvent, ParticleKind};

    // Collision shapes, from the same table the player's physics uses — so a
    // shard rests on a slab rather than sinking into it (§0.0 gotcha 2: this
    // must not key off the render fast-path).
    let collide = &session.collide;
    let world = &session.world;
    let shapes = |x: i32, y: i32, z: i32| -> &[[f32; 6]] {
        let state = world.block_state_at(x, y, z) as usize;
        collide.get(state).map(|v| v.as_slice()).unwrap_or(&[])
    };

    if !events.is_empty() {
        log::debug!("live: {} particle event(s)", events.len());
    }
    for ev in events {
        match ev {
            ParticleEvent::Command(cmd) => p.sys.spawn_from_packet(&cmd, &shapes),
            ParticleEvent::DestroyBlock { x, y, z, block_state } => {
                // Vanilla iterates the block's collision boxes; a shapeless
                // block spawns nothing.
                let shape = collide.get(block_state as usize).cloned().unwrap_or_default();
                p.sys.spawn_destroy_block(x, y, z, block_state, &shape, &shapes);
            }
        }
    }

    // Vanilla's `ParticleEngine.tick` runs once per client tick and a stalled
    // client simply misses ticks — it never fast-forwards. Cap the catch-up so
    // a hitch cannot age a burst out of existence in one frame.
    const MAX_CATCH_UP: u64 = 4;
    let last = *p.last_tick.get_or_insert(session.ticks);
    let steps = session.ticks.saturating_sub(last).min(MAX_CATCH_UP);
    for _ in 0..steps {
        p.sys.tick(&shapes);
    }
    p.last_tick = Some(session.ticks);

    if p.sys.is_empty() {
        let _ = wr.set_particles(gpu, &rewo_gpu::particles::ParticleDraw { verts: Vec::new() });
        return;
    }

    let quads: Vec<rewo_gpu::particles::ParticleQuad> = p
        .sys
        .particles
        .iter()
        .filter_map(|q| {
            // A terrain shard samples the broken block's own particle texture
            // and takes a quarter-window out of it (`uo`/`vo` in quarters);
            // everything else takes a whole sprite off the particle strip.
            let (layer, uv) = if q.kind == ParticleKind::Terrain {
                let l = *baked.particle_layer.get(q.block_state as usize)? as u32;
                if l == assets::NO_PARTICLE_LAYER as u32 {
                    return None;
                }
                (
                    l,
                    [
                        q.uo / 4.0,
                        q.vo / 4.0,
                        (q.uo + 1.0) / 4.0,
                        (q.vo + 1.0) / 4.0,
                    ],
                )
            } else {
                (p.layer_for(q.kind, q.sprite_frame)?, [0.0, 0.0, 1.0, 1.0])
            };
            let pos = q.render_pos(partial_ticks as f64);
            let (block_light, sky_light) = world.light_at(
                pos[0].floor() as i32,
                pos[1].floor() as i32,
                pos[2].floor() as i32,
            );
            Some(rewo_gpu::particles::ParticleQuad {
                pos,
                // `getQuadSize` is a HALF-extent in vanilla's quad expansion.
                size: q.quad_size_at(partial_ticks),
                color: [q.r_col, q.g_col, q.b_col, q.alpha],
                uv,
                layer,
                block_light,
                sky_light,
            })
        })
        .collect();

    let draw = rewo_gpu::particles::ParticleDraw::build(&quads, view);
    log::debug!(
        "live: particles alive={} quads={} verts={}",
        p.sys.len(),
        quads.len(),
        draw.verts.len()
    );
    if let Err(e) = wr.set_particles(gpu, &draw) {
        log::warn!("live: particle upload failed: {e}");
    }
}

pub(super) fn apply_weather(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    session: &PlaySession,
    w: &mut WeatherAssets,
    partial_ticks: f32,
    // Frame time in ticks, for the rain-fog ease. `None` means "converge
    // immediately" — the headless path draws a single frame after settling,
    // where an eased multiplier would still be near zero.
    _delta_ticks: Option<f32>,
) {
    let eye = eye_f64(session);
    let game_time = session.game_time();
    let weather = effective_weather(session);

    // -- clouds --
    let dim = session.active_dimension_type.as_ref();
    // `WeatherAttributes` greys the cloud colour too — a rainy deck is a dark
    // grey one, not the clear-weather white at lower alpha.
    let color = {
        let mut a = weather_attributes(0, 0, session);
        a.apply(weather.rain_level(), weather.thunder_level());
        a.cloud_color
    };
    let height = dim
        .map(|d| d.cloud_height)
        .unwrap_or(rewo_world::dimension::DEFAULT_CLOUD_HEIGHT);
    // `ARGB.alpha(cloudColor) > 0` is vanilla's whole test — no dimension name
    // is consulted, and neither is one here.
    let cloud_alpha = ((color as u32) >> 24) & 0xFF;
    match (&w.clouds, cloud_alpha > 0) {
        (Some(tex), true) => {
            let placement = rewo_gpu::clouds::placement(
                eye,
                height,
                game_time,
                partial_ticks,
                tex.width,
                tex.height,
            );
            let key = (placement.cell_x, placement.cell_z, placement.relative_pos);
            let stale = !matches!(&w.cached, Some((_, cx, cz, rp)) if (*cx, *cz, *rp) == key);
            if stale {
                let faces = tex.build_mesh(
                    placement.relative_pos,
                    placement.cell_x,
                    placement.cell_z,
                    rewo_gpu::clouds::CloudStatus::Fancy,
                    rewo_gpu::clouds::radius_cells(CLOUD_RANGE_CHUNKS),
                );
                w.cached = Some((faces, key.0, key.1, key.2));
            }
            let faces = w.cached.as_ref().map(|c| c.0.clone()).unwrap_or_default();
            if let Err(e) = wr.set_clouds(
                gpu,
                &rewo_gpu::clouds::CloudDraw {
                    faces,
                    placement,
                    color_argb: color,
                    fog_clouds_end: CLOUD_FOG_END,
                camera: [eye[0] as f32, eye[1] as f32, eye[2] as f32],
                },
            ) {
                log::warn!("live: cloud upload failed: {e}");
            }
        }
        _ => {
            // No texture, or a transparent cloud colour: draw nothing rather
            // than leaving the previous dimension's deck hanging in the sky.
            let _ = wr.set_clouds(
                gpu,
                &rewo_gpu::clouds::CloudDraw {
                    faces: Vec::new(),
                    placement: rewo_gpu::clouds::placement(eye, height, game_time, 0.0, 1, 1),
                    color_argb: 0,
                    fog_clouds_end: 1.0,
                camera: [eye[0] as f32, eye[1] as f32, eye[2] as f32],
                },
            );
            w.cached = None;
        }
    }

    // -- rain and snow --
    // `ClientLevel.getSeaLevel()` comes from the spawn info; before the first
    // one arrives there is no world to rain on anyway.
    let sea_level = session.sea_level.unwrap_or(63);
    let extracted = session.world.extract_weather(
        &weather,
        &w.noise,
        eye,
        WEATHER_RADIUS,
        game_time,
        partial_ticks,
        sea_level,
    );
    let to_gpu = |c: &rewo_world::weather::ColumnInstance| rewo_gpu::weather::WeatherColumn {
        x: c.x,
        z: c.z,
        bottom_y: c.bottom_y,
        top_y: c.top_y,
        u_offset: c.u_offset,
        v_offset: c.v_offset,
        block_light: rewo_world::weather::light_block(c.light_coords) as u8,
        sky_light: rewo_world::weather::light_sky(c.light_coords) as u8,
    };
    let state = rewo_gpu::weather::WeatherRenderState {
        intensity: extracted.intensity,
        radius: extracted.radius,
        rain_columns: extracted.rain.iter().map(to_gpu).collect(),
        snow_columns: extracted.snow.iter().map(to_gpu).collect(),
    };
    if let Err(e) = wr.set_weather(
        gpu,
        &rewo_gpu::weather::WeatherDraw::build(&state, &w.directions, eye),
    ) {
        log::warn!("live: weather upload failed: {e}");
    }
}

/// The resolved visual attributes weather rewrites, gathered for one frame.
///
/// The cloud, star and sky-light entries come along because
/// `WeatherAttributes` modifies all of them together; callers take the fields
/// they need. `sky_light_level` is carried but unused — Rewo's lightmap is
/// driven by `sky_light_factor` and `sky_light_color`, and `SKY_LIGHT_LEVEL`
/// feeds `Level.skyDarken`, which is a mob-spawning input rather than a
/// rendering one.
pub(super) fn weather_attributes(
    sky: i32,
    fog: i32,
    session: &PlaySession,
) -> rewo_world::weather::WeatherAttributes {
    let dim = session.active_dimension_type.as_ref();
    rewo_world::weather::WeatherAttributes {
        sky_color: sky,
        fog_color: fog,
        cloud_color: dim.map(|d| d.cloud_color).unwrap_or(0),
        sky_light_level: 15.0,
        sky_light_color: dim
            .map(|d| d.sky_light_color)
            .unwrap_or(rewo_world::dimension::DEFAULT_SKY_LIGHT_COLOR),
        sky_light_factor: dim
            .map(|d| d.sky_light_factor)
            .unwrap_or(rewo_world::dimension::DEFAULT_SKY_LIGHT_FACTOR),
        star_brightness: 1.0,
        sunrise_sunset_color: 0,
    }
}

/// Weather's two effects on the celestials.
///
/// `SkyRenderer` fades the sun and moon by `1 - rainLevel`, and — separately,
/// through `WeatherAttributes` — the stars are **set to zero**, not dimmed.
pub(super) fn apply_weather_to_celestial(
    cel: &mut rewo_gpu::celestial::CelestialState,
    session: &PlaySession,
) {
    let w = effective_weather(session);
    let (rain, thunder) = (w.rain_level(), w.thunder_level());
    cel.rain_brightness = rewo_world::weather::rain_brightness(rain);
    let mut a = weather_attributes(0, 0, session);
    a.star_brightness = cel.star_brightness;
    a.apply(rain, thunder);
    cel.star_brightness = a.star_brightness;
}

/// Advance the rain-fog ease and return this frame's environmental fog band.
///
/// `delta_ticks` of `None` converges immediately — the headless path draws a
/// single frame after settling, where an eased multiplier would still be near
/// zero and would grade a storm that has not arrived.
pub(super) fn rain_fog_band(
    session: &PlaySession,
    w: &mut WeatherAssets,
    delta_ticks: Option<f32>,
) -> [f32; 2] {
    let weather = effective_weather(session);
    let eye = eye_f64(session);
    let (bx, by, bz) = (
        eye[0].floor() as i32,
        eye[1].floor() as i32,
        eye[2].floor() as i32,
    );
    // Sky light gates it entirely — below 9 there is no rain fog, which is why
    // stepping into a cave during a storm clears the air. A biome that never
    // rains still thickens, at half strength.
    let (_, sky_light) = session.world.light_at(bx, by, bz);
    let rains_here = session
        .world
        .climate_at(bx, by, bz)
        .map(|c| c.has_precipitation)
        .unwrap_or(true);
    match delta_ticks {
        Some(dt) => w
            .rain_fog
            .update(weather.rain_level(), sky_light, rains_here, dt),
        None => w
            .rain_fog
            .converge(weather.rain_level(), sky_light, rains_here),
    }
    if w.rain_fog.multiplier() <= 0.0 {
        // Disabled: everything is nearer than the start, so the environmental
        // term contributes nothing and the render-distance band decides alone.
        return [1.0e9, 1.0e9 + 1.0];
    }
    let (start, end) = w.rain_fog.apply(ENV_FOG_START, ENV_FOG_END);
    [start, end]
}

// -- M34: hotbar item icons ---------------------------------------------------
