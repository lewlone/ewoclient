use super::*;

pub(super) fn entity_light(
    world: &rewo_world::World,
    x: f64,
    eye_y: f64,
    z: f64,
    state: &LightmapState,
) -> [f32; 3] {
    let (block, sky) = world.light_at(x.floor() as i32, eye_y.floor() as i32, z.floor() as i32);
    let mut rgb = sample(block, sky, state);
    // The shader's genuine `0/0` black-texel path yields NaN (see
    // `rewo_world::lightmap::sample`'s docs). Map ONLY nonfinite components to
    // 0 so a NaN can't poison the CPU entity-vertex transport. This is a CPU
    // vertex-safety guard, NOT a claimed vanilla rule: the production
    // terrain-store behaviour for that NaN is still to be pinned by the later
    // M13 Vulkan black-NaN readback oracle.
    for c in &mut rgb {
        if !c.is_finite() {
            *c = 0.0;
        }
    }
    rgb
}

/// The cycle state for a world-clock tick, defaulting to full daylight
/// before the first `set_time`.
pub(super) fn daylight_of(day_ticks: Option<i64>) -> rewo_world::daylight::SkyLighting {
    day_ticks.map_or(
        rewo_world::daylight::SkyLighting::DAY,
        rewo_world::daylight::sky_lighting,
    )
}

/// The active dimension's fixed light attributes, plus whether the Overworld
/// day timeline applies to them (M16).
///
/// `rewo_world::daylight::SkyLighting` is a set of **multipliers** (the
/// `Timelines` tracks modify a base value), so the two layers compose:
/// `sky_factor = dimension.sky_light_factor * timeline.light_factor`, and
/// likewise per channel for the sky-light colour. Vanilla decides whether a
/// track applies from the dimension type's `timelines` tag — the Overworld's
/// `#minecraft:in_overworld` contains `minecraft:day`, while
/// `#minecraft:in_nether` / `#minecraft:in_end` contain only
/// `#minecraft:universal` (the villager schedule), which carries no `visual/*`
/// track at all.
///
/// Rewo decodes whether that holder set contains `minecraft:day` from the
/// exact 26.2 built-in timeline tag reports. It is not inferred from fixed
/// time, skybox, or the registry name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DimensionLight {
    pub(super) ambient_color: [f32; 3],
    pub(super) sky_light_color: [f32; 3],
    pub(super) sky_light_factor: f32,
    /// `Timelines.OVERWORLD_DAY`'s tracks modify this dimension.
    pub(super) day_timeline: bool,
}

impl DimensionLight {
    /// Before login (and on every serverless path) there is no dimension: the
    /// `EnvironmentAttributes` codec defaults, with the day timeline applying.
    /// This is exactly the pre-M16 behaviour.
    pub(super) const UNRESOLVED: Self = Self {
        ambient_color: rewo_world::lightmap::DEFAULT_AMBIENT_COLOR,
        sky_light_color: [1.0, 1.0, 1.0],
        sky_light_factor: 1.0,
        day_timeline: true,
    };
}

pub(super) fn dimension_light(def: Option<&DimensionTypeDef>) -> DimensionLight {
    match def {
        None => DimensionLight::UNRESOLVED,
        Some(d) => DimensionLight {
            ambient_color: rgb24_to_vec3(d.ambient_light_color),
            sky_light_color: rgb24_to_vec3(d.sky_light_color),
            sky_light_factor: d.sky_light_factor,
            day_timeline: d.has_day_timeline,
        },
    }
}

/// The timeline multipliers that apply in this dimension. A fixed-time
/// dimension gets the identity set (`SkyLighting::DAY` is all-ones), so a stale
/// Overworld clock can never tint the Nether or the End — including across a
/// respawn, where `day_ticks` keeps ticking but the dimension changed.
pub(super) fn dimension_timeline(
    day_ticks: Option<i64>,
    dim: &DimensionLight,
) -> rewo_world::daylight::SkyLighting {
    if dim.day_timeline {
        daylight_of(day_ticks)
    } else {
        rewo_world::daylight::SkyLighting::DAY
    }
}

/// The dimension's skybox as the renderer's mode (`DimensionType.Skybox` ->
/// `LevelRenderer.addSkyPass`). An unresolved dimension keeps the Overworld
/// sky, which is the codec default for a missing `skybox` field anyway.
pub(super) fn sky_mode_of(def: Option<&DimensionTypeDef>) -> SkyMode {
    match def.map(|d| d.skybox) {
        Some(Skybox::None) => SkyMode::None,
        Some(Skybox::End) => SkyMode::End,
        Some(Skybox::Overworld) | None => SkyMode::Overworld,
    }
}

// M157 — `hideLightningFlash` is a real option now, read from `options.txt`.
//
// The constant that used to sit here said it was "a stated assumption rather
// than a transcription … the gate for it is the `EndFlash::hidden()`
// constructor, which exists so wiring an option later is a call-site change
// rather than a new branch." That is exactly what happened: the branch below
// takes the option instead of the literal and nothing else moved.

/// The End flash's contribution to one frame's lightmap
/// (`LightmapRenderStateExtractor.java:57-65`).
///
/// Zero — [`EndFlash::none`] — covers every dimension that is not the End,
/// which is why the neutral value is also the [`Default`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct EndFlash {
    /// `EndFlashState.getIntensity(partialTicks)`.
    pub(super) intensity: f32,
    /// `minecraft.gui.hud.getBossOverlay().shouldCreateWorldFog()` — a wither
    /// or dragon bar with `FLAG_FOG` set. It **divides the flash by three**
    /// rather than suppressing it.
    pub(super) boss_world_fog: bool,
}

impl EndFlash {
    /// No flash: not the End, or no `EndFlashState` on the level.
    pub(super) fn none() -> Self {
        Self::default()
    }

    /// The option is on, so vanilla adds nothing at all — distinct from an
    /// intensity that happens to be 0, because it holds through the peak.
    pub(super) fn hidden() -> Self {
        Self::default()
    }

    pub(super) fn sky_factor_bonus(self) -> f32 {
        if self.boss_world_fog {
            self.intensity / 3.0
        } else {
            self.intensity
        }
    }
}

/// `SkyRenderState.endFlash{Intensity,XAngle,YAngle}`
/// (`SkyRenderer.java:270-274`) — the render half, which unlike the lightmap's
/// is **not** gated on `hideLightningFlash`: that option hides the world's
/// brightening, not the flash itself.
pub(super) fn end_flash_render(
    session: &rewo_net::play::PlaySession,
    partial: f32,
) -> Option<(f32, f32, f32)> {
    let s = session.end_flash()?;
    Some((s.intensity(partial), s.x_angle(), s.y_angle()))
}

/// Resolve one frame's [`EndFlash`] from the session.
///
/// The three gates are vanilla's, in vanilla's order: the level must *have* a
/// flash state at all (only an END skybox does), the option must be off, and
/// only then is the boss-fog divisor chosen.
/// Read `options.txt` from the game directory (M157).
///
/// **The GAME directory, not a config dir** (`Options.java:1468`:
/// `new File(workingDirectory, "options.txt")`), which is what lets Rewo and a
/// vanilla client share one — and is why saving MERGES rather than rewrites.
///
/// An absent or unreadable file is the defaults, not an error: vanilla's
/// `load()` opens with `if (!this.optionsFile.exists()) return;`.
pub(super) fn options_path() -> std::path::PathBuf {
    std::path::PathBuf::from("options.txt")
}

pub(super) fn load_options() -> rewo_net::options::Options {
    match std::fs::read_to_string(options_path()) {
        Ok(text) => {
            let o = rewo_net::options::Options::parse(&text);
            log::info!("options: loaded {o:?}");
            o
        }
        Err(_) => rewo_net::options::Options::default(),
    }
}

/// Write the modelled options back, preserving every line Rewo does not
/// model. Dead code from M157 until M173 gave it callers (the cycle buttons
/// per click, the sliders on screen exit — vanilla's `OptionsSubScreen.
/// removed()`).
pub(super) fn save_options(o: rewo_net::options::Options) {
    let existing = std::fs::read_to_string(options_path()).unwrap_or_default();
    if let Err(e) = std::fs::write(options_path(), o.merge_into(&existing)) {
        log::warn!("options: could not save: {e}");
    }
}

pub(super) fn end_flash_for(
    session: &rewo_net::play::PlaySession,
    partial: f32,
    options: &rewo_net::options::Options,
) -> EndFlash {
    let Some(state) = session.end_flash() else {
        return EndFlash::none();
    };
    if options.hide_lightning_flash {
        return EndFlash::hidden();
    }
    EndFlash {
        intensity: state.intensity(partial),
        boss_world_fog: session.boss_bars.should_create_world_fog(),
    }
}

/// Resolve the full camera `LightmapState` for one frame (M13).
///
/// The three independent clocks fold into one uniform: the day/night timeline
/// (`day_ticks` → sky factor + colour), the block-light flicker (already
/// advanced into `block_factor`), and the mob-effect `snapshot` (night vision +
/// darkness). `gamma` / `darkness_option` are the player's validated options.
/// Pure — the single seam the tests pin.
pub(super) fn resolve_lightmap(
    day_ticks: Option<i64>,
    dimension: Option<&DimensionTypeDef>,
    block_factor: f32,
    snapshot: rewo_net::effects::VisualEffectSnapshot,
    gamma: f32,
    darkness_option: f32,
    partial: f32,
    end_flash: EndFlash,
) -> LightmapState {
    let dim = dimension_light(dimension);
    let sky = dimension_timeline(day_ticks, &dim);
    let night_vision_factor = snapshot
        .night_vision_duration
        .map_or(0.0, |d| night_vision_intensity(d, partial));
    let (brightness_factor, darkness_scale) = darkness_lightmap(
        gamma,
        darkness_option,
        snapshot.darkness_blend_factor,
        snapshot.tick_count,
        partial,
    );
    LightmapState {
        // The timeline tracks are multipliers over the dimension's base, and
        // the End flash is **added on top** rather than folded into either
        // (`LightmapRenderStateExtractor.java:57-65`) — so it brightens a
        // dimension whose own `sky_light_factor` is 0, which the End's is.
        sky_factor: dim.sky_light_factor * sky.light_factor + end_flash.sky_factor_bonus(),
        block_factor,
        sky_light_color: std::array::from_fn(|c| dim.sky_light_color[c] * sky.light_color[c]),
        ambient_color: dim.ambient_color,
        brightness_factor,
        darkness_scale,
        night_vision_factor,
    }
}

/// Convert the CPU `LightmapState` into the GPU renderer's mirror. The two
/// structs carry identical fields (only `sky_light_color`/`sky_color` differ in
/// name), so this is a field-for-field copy.
/// The lightmap's sky colour is a linear 0..1 triple; `WeatherAttributes`
/// works in ARGB. These two convert between them **without** an sRGB transfer:
/// `SKY_LIGHT_COLOR` reaches the shader through `ARGB.vector3fFromRGB24`, a
/// plain `/255`, so a round trip through here must be the same plain scale or
/// clear weather would shift.
pub(super) fn linear_rgb_to_argb(c: [f32; 3]) -> i32 {
    let ch = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round() as i32) & 0xFF;
    (0xFFu32 as i32) << 24 | (ch(c[0]) << 16) | (ch(c[1]) << 8) | ch(c[2])
}

pub(super) fn argb_to_linear_rgb(c: i32) -> [f32; 3] {
    let ch = |s: u32| ((c as u32 >> s) & 0xFF) as f32 / 255.0;
    [ch(16), ch(8), ch(0)]
}

pub(super) fn to_world_lightmap(s: &LightmapState) -> WorldLightmapState {
    WorldLightmapState {
        sky_factor: s.sky_factor,
        block_factor: s.block_factor,
        sky_color: s.sky_light_color,
        ambient_color: s.ambient_color,
        brightness_factor: s.brightness_factor,
        darkness_scale: s.darkness_scale,
        night_vision_factor: s.night_vision_factor,
        // Disabled here; `apply_lightmap` sets the real band.
        env_fog: [1.0e9, 1.0e9 + 1.0],
    }
}

/// Push one resolved lightmap into the renderer: the full lightmap uniform plus
/// the day/night sky/fog gradient tint (a separate concern from the lightmap).
/// `set_lightmap_state` already carries the sky factor + colour, so this does
/// NOT also call `set_lightmap` — no duplicate set.
pub(super) fn apply_lightmap(
    wr: &mut WorldRenderer,
    state: &LightmapState,
    day_ticks: Option<i64>,
    dimension: Option<&DimensionTypeDef>,
    // M33: the rain and thunder levels, because `WeatherAttributes` modifies
    // SKY_LIGHT_FACTOR and SKY_LIGHT_COLOR — the world genuinely dims in a
    // storm, and without this the terrain stays at clear-weather brightness
    // under a black sky.
    weather: (f32, f32),
    // The environmental fog band this frame, already through the rain ramp.
    rain_fog: [f32; 2],
) {
    let mut lm = to_world_lightmap(state);
    let (rain, thunder) = weather;
    if rain > 0.0 {
        let mut a = rewo_world::weather::WeatherAttributes {
            sky_color: 0,
            fog_color: 0,
            cloud_color: 0,
            sky_light_level: 15.0,
            // The lightmap's own resolved colour, packed back to ARGB so the
            // attribute layer's `alphaBlend` sees what vanilla's would.
            sky_light_color: linear_rgb_to_argb(lm.sky_color),
            sky_light_factor: lm.sky_factor,
            star_brightness: 0.0,
            sunrise_sunset_color: 0,
        };
        a.apply(rain, thunder);
        lm.sky_factor = a.sky_light_factor;
        lm.sky_color = argb_to_linear_rgb(a.sky_light_color);
    }
    // The environmental fog band. `rain_fog` is the eased multiplier; a zero
    // one leaves the band disabled and the render-distance fade alone.
    lm.env_fog = rain_fog;
    wr.set_lightmap_state(lm);
    // The sky/fog gradient multiply is a day-timeline track too, so it is gated
    // on the same dimension test — otherwise a midnight Overworld clock would
    // black out the End's `#000000`-based sky and blue-shift its fog.
    let sky = dimension_timeline(day_ticks, &dimension_light(dimension));
    wr.set_sky_tint(sky.sky_color, sky.fog_color);
    // Set every frame, so a dimension change or respawn needs no other
    // bookkeeping and can never leave the previous world's skybox behind.
    wr.set_sky_mode(sky_mode_of(dimension));
}

/// M14: push the camera biome sky/fog base color. It composes with the existing
/// `set_sky_tint` day/night multiply inside the renderer (`sky_base * sky_tint`)
/// and is a per-frame uniform, so a biome/time change never remeshes. No biome
/// context (offline non-biome server) leaves the GPU's default fixed sky.
pub(super) fn apply_biome_sky_fog(wr: &mut WorldRenderer, session: &PlaySession) {
    let eye = eye_f64(session);
    // M33. Two distinct weather effects apply here, and only one of them is
    // `applyWeatherDarken`:
    //
    //   1. `WeatherAttributes` rewrites the resolved SKY and FOG colours before
    //      any renderer sees them — the sky blends most of the way to grey, the
    //      fog is multiplied down. This is what actually greys a rainy sky.
    //   2. `AtmosphericFogEnvironment.getBaseColor` then applies
    //      `applyWeatherDarken` to the SKY colour only, on top of (1).
    //
    // Applying (2) to the fog as well — which this did before — double-darkens
    // it with a curve that was never meant for it.
    let w = effective_weather(session);
    let (rain, thunder) = (w.rain_level(), w.thunder_level());
    let weathered = |sky: i32, fog: i32| -> (i32, i32) {
        let mut a = weather_attributes(sky, fog, session);
        a.apply(rain, thunder);
        (
            rewo_world::weather::apply_weather_darken(a.sky_color, rain, thunder),
            a.fog_color,
        )
    };
    if let Some(sky) = session.world.camera_sky(eye) {
        let fog = session.world.camera_fog(eye).unwrap_or(sky);
        let (sky, fog) = weathered(sky, fog);
        wr.set_sky_fog_base(argb_to_linear(sky), argb_to_linear(fog));
        return;
    }
    // No biome context (an offline / non-biome server): the positional layer
    // that normally carries the dimension base forward is absent, so read the
    // dimension's own `visual/sky_color` / `visual/fog_color` directly — but
    // ONLY when it actually sets them. The Nether sets NEITHER, and inventing a
    // base for it (black, or the Overworld's) is exactly the guess the
    // decompile does not license; `None` there leaves the GPU default.
    let def = session.active_dimension_type.as_ref();
    if let (Some(sky), Some(fog)) = (def.and_then(|d| d.sky_color), def.and_then(|d| d.fog_color)) {
        let (sky, fog) = weathered(sky, fog);
        wr.set_sky_fog_base(argb_to_linear(sky), argb_to_linear(fog));
    }
}

/// Opaque ARGB int (biome sky/fog color, sRGB) → linear RGB the GPU sky base
/// wants (the SRGB attachment re-encodes on store).
pub(super) fn argb_to_linear(argb: i32) -> [f32; 3] {
    let r = ((argb >> 16) & 0xFF) as f32 / 255.0;
    let g = ((argb >> 8) & 0xFF) as f32 / 255.0;
    let b = (argb & 0xFF) as f32 / 255.0;
    [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)]
}

pub(super) fn init_celestial_if_present(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    if let Some(cel) = &baked.celestial {
        wr.init_celestial(gpu, &to_gpu_celestial(cel))?;
    }
    // The End skybox texture comes from the same jar bake. Absent -> the End
    // draws no sky, which `WorldRenderer::end_sky_ready` reports honestly
    // rather than substituting an invented colour.
    if let Some(img) = &baked.end_sky {
        wr.init_end_sky(
            gpu,
            &EndSkyImage {
                rgba: &img.rgba,
                w: img.w,
                h: img.h,
            },
        )?;
    } else {
        log::warn!("live: no end_sky.png in the jar bake — the End will render no sky");
    }
    // The end-portal shader samples BOTH end_sky.png and end_portal.png (M32).
    // Missing either means no portal draws, which is honest — the alternative
    // was M28f's single static layer, which looked like a portal and was not.
    if let (Some(sky), Some(por)) = (&baked.end_sky, &baked.end_portal) {
        wr.init_end_portal(
            gpu,
            &rewo_gpu::end_portal::PortalImage {
                rgba: &sky.rgba,
                w: sky.w,
                h: sky.h,
            },
            &rewo_gpu::end_portal::PortalImage {
                rgba: &por.rgba,
                w: por.w,
                h: por.h,
            },
        )?;
    } else {
        log::warn!("live: no end_sky/end_portal texture — end portals will not render");
    }
    Ok(())
}

pub(super) fn to_gpu_celestial(cel: &assets::CelestialTextures) -> CelestialTextures<'_> {
    fn img(i: &assets::DecodedImage) -> CelestialImage<'_> {
        CelestialImage {
            rgba: &i.rgba,
            w: i.w,
            h: i.h,
        }
    }
    CelestialTextures {
        sun: img(&cel.sun),
        moons: std::array::from_fn(|k| img(&cel.moons[k])),
        end_flash: img(&cel.end_flash),
    }
}

/// Exact clear-weather Overworld celestial timeline. Before the first server
/// time packet, noon matches the existing `SkyLighting::DAY` fallback.
pub(super) fn celestial_state_of(day_ticks: Option<i64>) -> CelestialState {
    let c = rewo_world::celestial::celestial_at(day_ticks.unwrap_or(6000));
    let argb = c.sunrise_sunset_color;
    let ch = |sh: u32| ((argb >> sh) & 0xFF) as f32 / 255.0;
    CelestialState {
        sun_angle: c.sun_angle_rad(),
        moon_angle: c.moon_angle_rad(),
        star_angle: c.star_angle_rad(),
        star_brightness: c.star_brightness,
        moon_phase: c.moon_phase,
        sunrise_rgba: [
            srgb_to_linear(ch(16)),
            srgb_to_linear(ch(8)),
            srgb_to_linear(ch(0)),
            ch(24),
        ],
        rain_brightness: 1.0,
    }
}
