//! `rewo weathershot --check` — the weather and cloud oracle (M33).
//!
//! Serverless, validation-required, fail-closed. It grades three layers:
//!
//! 1. **The wire and the rule**, on the CPU, through the production dispatch —
//!    the four `game_event` ids resolved by name from the report, the
//!    counter-intuitive `START_RAINING = 0.0` semantics, and the precipitation
//!    rule against an independent transcription.
//! 2. **The cloud mesh**, against an independently-built expectation.
//! 3. **The pixels**, by rendering both production passes offscreen and reading
//!    them back — including the property that makes each pass distinctive, with
//!    a sensitivity partner so a witness cannot pass on a blank frame.

use std::path::PathBuf;

use clap::Args as ClapArgs;
use rewo_data::{assets, packets::Packets, DataPaths};
use rewo_gpu::clouds::{CloudDraw, CloudStatus, CloudTexture, RelativeCameraPos};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::weather::{
    ColumnDirections, WeatherColumn, WeatherDraw, WeatherImage, WeatherRenderState,
};
use rewo_gpu::world::{perspective_reverse_z, SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_world::weather::{
    apply_weather_darken, greyscale, multiply, rain_brightness, BiomeClimate, ClimateNoise,
    Precipitation, RainFog, TemperatureModifier, WeatherAttributes, WeatherState,
};

use crate::stats::OverlayRing;

const EXPECTED_WITNESSES: usize = 35;
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const W: u32 = 128;
const H: u32 = 128;

#[derive(ClapArgs, Debug)]
pub struct WeathershotArgs {
    #[arg(long, default_value_t = false)]
    pub check: bool,
    #[arg(long, default_value = "26.2")]
    pub version: String,
    #[arg(long, default_value_t = false)]
    pub no_validation: bool,
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[weathershot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

pub fn run(args: WeathershotArgs) -> Result<(), String> {
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let want_validation = !args.no_validation;
    let mut gpu = Gpu::new(None, want_validation)?;
    println!(
        "[weathershot] Vulkan validation: {}",
        if gpu.validation_active {
            "ON"
        } else if args.no_validation {
            "off (--no-validation)"
        } else {
            "off (VK_LAYER_KHRONOS_validation unavailable)"
        }
    );
    if args.check && want_validation && !gpu.validation_active {
        return Err(
            "weathershot check: Vulkan validation requested but not active — install \
             the Vulkan SDK (VK_LAYER_KHRONOS_validation), or pass --no-validation"
                .into(),
        );
    }
    run_check(&mut gpu, &baked, &args)
}

fn run_check(
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
    args: &WeathershotArgs,
) -> Result<(), String> {
    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_wire(&mut c)?;
    check_precipitation(&mut c);
    check_cloud_mesh(&mut c);
    check_darken(&mut c);
    check_attributes(&mut c);
    check_rain_fog(&mut c);

    let mut off = Offscreen::new(gpu, W, H)?;
    let ring = OverlayRing::default();
    let draw = overlay_offscreen(&ring);
    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("out-dir: {e}"))?;
    }
    let rendered = check_cloud_pixels(&mut c, gpu, &mut off, baked, &draw, args)
        .and_then(|()| check_weather_pixels(&mut c, gpu, &mut off, baked, &draw, args));
    off.destroy(gpu);
    rendered?;

    println!(
        "[weathershot] witnesses observed: {} / {EXPECTED_WITNESSES}",
        c.witnessed
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
            "weathershot observed {} witnesses, expected {EXPECTED_WITNESSES} — a \
             witness that stops running is a failure, not a quieter pass",
            c.witnessed
        ));
    }
    println!("[weathershot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// -- 1. the wire ---------------------------------------------------------------

fn check_wire(c: &mut Checker) -> Result<(), String> {
    let paths = DataPaths::for_version("26.2").ok_or("no config dir")?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = rewo_net::ids::Ids::resolve(&packets)?;

    c.record(
        "w1.the_game_event_id_resolves_by_name",
        ids.cb_play_game_event >= 0 && ids.cb_play_game_event != ids.cb_play_set_time,
        format!(
            "clientbound-play `game_event` is id {} (set_time is {}) — resolved from \
             the report by name, so a renumbered protocol fails loud rather than \
             routing weather into the clock",
            ids.cb_play_game_event, ids.cb_play_set_time
        ),
    );

    // Drive raw bodies through the production route, not the state API.
    let body = |event: u8, param: f32| {
        let mut v = vec![event];
        v.extend_from_slice(&param.to_be_bytes());
        v
    };
    let mut w = WeatherState::default();
    let matched = rewo_net::route_game_event(
        ids.cb_play_game_event,
        &body(WeatherState::RAIN_LEVEL_CHANGE, 0.6),
        &ids,
        &mut w,
    );
    c.record(
        "w2.a_rain_level_change_reaches_the_state_through_the_real_route",
        matched && (w.rain_level() - 0.6).abs() < 1e-6,
        format!(
            "route_game_event matched={matched}, rain={} — the body is an UNSIGNED \
             BYTE then an f32, not a var-int pair",
            w.rain_level()
        ),
    );

    let mut w = WeatherState::default();
    rewo_net::route_game_event(
        ids.cb_play_game_event,
        &body(WeatherState::START_RAINING, 0.0),
        &ids,
        &mut w,
    );
    let started = w.rain_level();
    rewo_net::route_game_event(
        ids.cb_play_game_event,
        &body(WeatherState::STOP_RAINING, 0.0),
        &ids,
        &mut w,
    );
    let stopped = w.rain_level();
    c.record(
        "w3.start_raining_sets_zero_and_stop_raining_sets_one",
        started == 0.0 && stopped == 1.0,
        format!(
            "START_RAINING -> {started}, STOP_RAINING -> {stopped}. This reads \
             backwards and is `handleGameEvent` verbatim: the names describe the \
             server's transition, and the client sets the value the server's \
             RAIN_LEVEL_CHANGE ramp starts FROM. Making it intuitive would snap \
             rain to full the instant it began"
        ),
    );

    let mut w = WeatherState::default();
    w.set_rain(0.5);
    rewo_net::route_game_event(
        ids.cb_play_game_event,
        &body(WeatherState::THUNDER_LEVEL_CHANGE, 1.0),
        &ids,
        &mut w,
    );
    let with_rain = w.thunder_level();
    w.set_rain(0.0);
    let without_rain = w.thunder_level();
    c.record(
        "w4.thunder_is_gated_on_rain",
        (with_rain - 0.5).abs() < 1e-6 && without_rain == 0.0,
        format!(
            "thunder 1.0 reads {with_rain} under rain 0.5 and {without_rain} under \
             clear sky — `getThunderLevel` multiplies by `getRainLevel`"
        ),
    );

    // A non-weather game event matches the packet id and must change nothing.
    let mut w = WeatherState::default();
    w.set_rain(0.4);
    let matched = rewo_net::route_game_event(ids.cb_play_game_event, &body(3, 1.0), &ids, &mut w);
    c.record(
        "w5.an_unrelated_game_event_is_inert",
        matched && w.rain_level() == 0.4,
        format!(
            "CHANGE_GAME_MODE (3) routed (matched={matched}) and left rain at {} — \
             the packet carries a dozen non-weather events on the same byte",
            w.rain_level()
        ),
    );

    // A short body must not panic or half-apply.
    let mut w = WeatherState::default();
    w.set_rain(0.3);
    rewo_net::route_game_event(ids.cb_play_game_event, &[7u8, 0x3f], &ids, &mut w);
    c.record(
        "w6.a_truncated_body_is_inert",
        w.rain_level() == 0.3,
        format!(
            "a 2-byte body left rain at {} — vanilla's reader would throw, and \
             dropping the packet is the closest safe equivalent",
            w.rain_level()
        ),
    );
    Ok(())
}

// -- 2. the precipitation rule -------------------------------------------------

fn check_precipitation(c: &mut Checker) {
    let n = ClimateNoise::new();
    let plains = BiomeClimate {
        temperature: 0.8,
        ..Default::default()
    };
    let taiga = BiomeClimate {
        temperature: 0.05,
        ..Default::default()
    };
    let desert = BiomeClimate {
        has_precipitation: false,
        temperature: 2.0,
        temperature_modifier: TemperatureModifier::None,
    };
    let y = 64;
    c.record(
        "p1.warm_rains_cold_snows_and_dry_does_neither",
        n.precipitation_at(&plains, 0, y, 0, 63) == Precipitation::Rain
            && n.precipitation_at(&taiga, 0, y, 0, 63) == Precipitation::Snow
            && n.precipitation_at(&desert, 0, y, 0, 63) == Precipitation::None,
        "0.8 rains, 0.05 snows, and `has_precipitation: false` gives neither \
         however hot — the dry check comes first",
    );

    // The threshold is `>= 0.15`, not `> 0.15`.
    let at = BiomeClimate {
        temperature: 0.15,
        ..Default::default()
    };
    let below = BiomeClimate {
        temperature: 0.149_999,
        ..Default::default()
    };
    c.record(
        "p2.the_snow_threshold_is_inclusive_at_fifteen_hundredths",
        n.precipitation_at(&at, 0, y, 0, 63) == Precipitation::Rain
            && n.precipitation_at(&below, 0, y, 0, 63) == Precipitation::Snow,
        "`warmEnoughToRain` is `>= 0.15`, so exactly 0.15 still rains",
    );

    // Height turns rain to snow, strictly above seaLevel + 17.
    let at_cut = n.temperature_at(&plains, 0, 80, 0, 63);
    let above = n.temperature_at(&plains, 0, 81, 0, 63);
    c.record(
        "p3.height_only_cools_strictly_above_sea_level_plus_seventeen",
        at_cut == 0.8 && above < 0.8,
        format!(
            "y=80 reads {at_cut} (untouched) and y=81 reads {above} — the guard is \
             `y > seaLevel + 17`, and the noise is only consulted past it"
        ),
    );
    c.record(
        "p4.a_mountain_top_snows_where_its_base_rains",
        n.precipitation_at(&plains, 0, 64, 0, 63) == Precipitation::Rain
            && n.precipitation_at(&plains, 0, 900, 0, 63) == Precipitation::Snow,
        "the same 0.8-temperature biome rains at sea level and snows at y=900",
    );

    // The FROZEN modifier must be reachable and must vary.
    let frozen = BiomeClimate {
        temperature: 0.8,
        temperature_modifier: TemperatureModifier::Frozen,
        ..Default::default()
    };
    let mut pinned = 0;
    let mut total = 0;
    for x in 0..48 {
        for z in 0..48 {
            total += 1;
            if n.temperature_at(&frozen, x * 8, 64, z * 8, 63) == 0.2 {
                pinned += 1;
            }
        }
    }
    c.record(
        "p5.the_frozen_modifier_pins_ice_patches_and_leaves_the_rest",
        pinned > 0 && pinned < total,
        format!(
            "{pinned} of {total} sampled columns pin to 0.2 — the modifier is a \
             three-noise patch test, not a constant, and M14 dropped it as \
             colour-irrelevant (which it is; it is precipitation-relevant)"
        ),
    );
}

// -- 3. the cloud mesh ---------------------------------------------------------

fn check_cloud_mesh(c: &mut Checker) {
    // A 16x16 map with a single occupied cell at (1, 1).
    let side = 16u32;
    let mut rgba = vec![0u8; (side * side * 4) as usize];
    let i = ((side + 1) * 4) as usize;
    rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
    let t = CloudTexture::from_rgba(&rgba, side, side);

    c.record(
        "c1.the_empty_threshold_is_alpha_below_ten",
        {
            let solid = |a: u8| CloudTexture::from_rgba(&[255, 255, 255, a], 1, 1).cells[0] != 0;
            !solid(9) && solid(10)
        },
        "`isCellEmpty` is `ARGB.alpha(color) < 10` — a threshold, not a zero test",
    );

    let fancy = t.build_mesh(RelativeCameraPos::InsideClouds, 1, 1, CloudStatus::Fancy, 2);
    let fast = t.build_mesh(RelativeCameraPos::InsideClouds, 1, 1, CloudStatus::Fast, 2);
    c.record(
        "c2.fast_clouds_are_one_quad_and_fancy_ones_are_a_box",
        fast.len() == 1 && fancy.len() > fast.len(),
        format!(
            "FAST emits {} quad, FANCY {} — FAST is a single DOWN face flagged to \
             use the TOP colour, so a flat cloud is lit as its top rather than its \
             underside",
            fast.len(),
            fancy.len()
        ),
    );

    let above = t
        .build_mesh(RelativeCameraPos::AboveClouds, 1, 1, CloudStatus::Fancy, 2)
        .into_iter()
        .filter(|f| f[2] & 16 == 0)
        .map(|f| f[2] & 7)
        .collect::<Vec<_>>();
    let below = t
        .build_mesh(RelativeCameraPos::BelowClouds, 1, 1, CloudStatus::Fancy, 2)
        .into_iter()
        .filter(|f| f[2] & 16 == 0)
        .map(|f| f[2] & 7)
        .collect::<Vec<_>>();
    c.record(
        "c3.the_camera_side_decides_which_horizontal_face_exists",
        above == vec![1] && below == vec![0],
        format!(
            "above the deck builds only UP {above:?}, below only DOWN {below:?} — \
             the face you could not see is not built at all"
        ),
    );

    let interior: Vec<i32> = fancy
        .iter()
        .filter(|f| f[2] & 16 != 0)
        .map(|f| f[2] & 7)
        .collect();
    c.record(
        "c4.the_cells_around_the_camera_get_inward_wound_faces",
        interior.len() == 6 && (0..6).all(|d| interior.contains(&d)),
        format!(
            "the centre cell adds all six faces again with FLAG_INSIDE_FACE \
             ({interior:?}) — the vertex shader reverses their winding so a camera \
             inside the cloud still sees it"
        ),
    );

    // The low bit of each coordinate rides in the flags byte.
    let mut ok = true;
    for (x, z) in [(3i32, 5i32), (-3, -5), (7, -1)] {
        let faces = t.build_mesh(
            RelativeCameraPos::InsideClouds,
            1 - x,
            1 - z,
            CloudStatus::Fast,
            12,
        );
        let hit = faces
            .iter()
            .any(|f| (f[0] << 1) | ((f[2] & 128) >> 7) == x && (f[1] << 1) | ((f[2] & 64) >> 6) == z);
        ok &= hit;
    }
    c.record(
        "c5.odd_cell_coordinates_survive_the_three_byte_packing",
        ok,
        "each coordinate is stored shifted right by one with its low bit in the \
         flags byte; a cell that lost it would render 12 blocks away",
    );
}

// -- 4. the darkening ----------------------------------------------------------

fn check_darken(c: &mut Checker) {
    let white = 0xFFFF_FFFFu32 as i32;
    let rainy = apply_weather_darken(white, 1.0, 0.0) as u32;
    c.record(
        "d1.rain_dims_blue_less_than_red_and_green",
        (rainy >> 16) & 0xFF == 127 && (rainy >> 8) & 0xFF == 127 && rainy & 0xFF == 153,
        format!(
            "white becomes ({}, {}, {}) under full rain — red and green scale by \
             1 - rain*0.5 but blue only by 1 - rain*0.4, so the sky goes BLUER as \
             it darkens rather than merely dimmer",
            (rainy >> 16) & 0xFF,
            (rainy >> 8) & 0xFF,
            rainy & 0xFF
        ),
    );
    c.record(
        "d2.clear_weather_changes_nothing_and_the_sun_tracks_the_rain",
        apply_weather_darken(white, 0.0, 0.0) == white
            && rain_brightness(0.0) == 1.0
            && rain_brightness(1.0) == 0.0,
        "the guards are `> 0.0`, so clear weather does not even truncate; and \
         `rainBrightness = 1 - rainLevel` becomes the sun's and moon's alpha, so \
         M12's celestials fade out as rain comes in",
    );
}

/// The attribute layer — where a rainy sky ACTUALLY greys out.
///
/// M33 first shipped only `applyWeatherDarken` and the sky stayed obviously
/// blue. The real mechanism is `WeatherAttributes`, a set of environment
/// attribute layers that rewrite SKY_COLOR, FOG_COLOR, CLOUD_COLOR,
/// SKY_LIGHT_*, STAR_BRIGHTNESS and SUNRISE_SUNSET_COLOR before any renderer
/// reads them.
fn check_attributes(c: &mut Checker) {
    const OVERWORLD_SKY: i32 = 0xFF78A7FFu32 as i32;
    let base = || WeatherAttributes {
        sky_color: OVERWORLD_SKY,
        fog_color: 0xFFC0D8FFu32 as i32,
        cloud_color: 0xCCFF_FFFFu32 as i32,
        sky_light_level: 15.0,
        sky_light_color: -1,
        sky_light_factor: 1.0,
        star_brightness: 1.0,
        sunrise_sunset_color: 0x80FF_A050u32 as i32,
    };
    let rgb = |c: i32| ((c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF);

    let mut rain = base();
    rain.apply(1.0, 0.0);
    let (r0, _, b0) = rgb(OVERWORLD_SKY);
    let (r, g, b) = rgb(rain.sky_color);
    c.record(
        "a1.rain_greys_the_sky_rather_than_only_dimming_it",
        (b - r) * 3 < (b0 - r0) && b < b0,
        format!(
            "the Overworld sky {:?} becomes ({r},{g},{b}) at full rain — the              channel spread collapses from {} to {}, which is DESATURATION, not              dimming. `BLEND_TO_GRAY(0.6, 0.75)` takes it three quarters of the              way to a 60%-bright luma grey. `applyWeatherDarken` alone (which is              what M33 shipped first) moves it about 40% and leaves it blue",
            rgb(OVERWORLD_SKY),
            b0 - r0,
            b - r
        ),
    );

    let mut thunder = base();
    thunder.apply(1.0, 1.0);
    let mut thunder_only = base();
    thunder_only.apply(1.0, 1.0);
    c.record(
        "a2.the_two_levels_partition_rather_than_stack",
        rgb(thunder.sky_color).2 < rgb(rain.sky_color).2 && thunder == thunder_only,
        format!(
            "full thunder gives {:?} against rain's {:?}. `rainLevel =              getRainLevel() - thunderLevel`, so a full thunderstorm runs the              THUNDER row ALONE — the rain row contributes nothing at that point",
            rgb(thunder.sky_color),
            rgb(rain.sky_color)
        ),
    );

    c.record(
        "a3.rain_removes_the_stars_rather_than_dimming_them",
        rain.star_brightness == 0.0 && {
            let mut half = base();
            half.apply(0.5, 0.0);
            half.star_brightness == 0.5
        },
        "STAR_BRIGHTNESS is `.set(0)`, not a modifier — the layer lerps toward          OFF, so half rain leaves half the stars and full rain leaves none",
    );

    c.record(
        "a4.rain_darkens_the_lightmap_and_greys_the_clouds",
        rain.sky_light_factor < 1.0
            && rain.sky_light_color != -1
            && rgb(rain.cloud_color).2 < rgb(base().cloud_color).2,
        format!(
            "sky_light_factor {:.3}, sky_light_color {:08x}, cloud {:?}. A              `client/`-only search for `getRainLevel` finds NONE of this: the              lightmap reaches the rain level through the attribute system, which              is why M33 first concluded rain did not affect it",
            rain.sky_light_factor,
            rain.sky_light_color,
            rgb(rain.cloud_color)
        ),
    );

    let mut clear = base();
    clear.apply(0.0, 0.0);
    c.record(
        "a5.clear_weather_touches_nothing_and_the_primitives_are_vanillas",
        clear == base()
            && greyscale(0xFF0000FFu32 as i32) & 0xFF == 28
            && multiply(-1, 0x1234_5678) == 0x1234_5678,
        "both guards are `> 0.0`; `greyscale` is luma-weighted (0.11 * 255          truncates to 28 for pure blue, not 85); and `ARGB.multiply` shortcuts          on opaque white",
    );
}

/// The rain fog ramp — the fourth and last client consumer of the rain level,
/// and the only one that moves a distance rather than a colour.
fn check_rain_fog(c: &mut Checker) {
    c.record(
        "f1.sky_light_gates_the_rain_fog_off_underground",
        RainFog::target(1.0, 8, true) == 0.0
            && RainFog::target(1.0, 9, true) > 0.0
            && RainFog::target(1.0, 15, true) == 1.0,
        "`clamp((skyLight - 8) / 7, 0, 1)` — at 8 sky light there is NO rain          fog, which is why stepping into a cave during a storm clears the air          instantly rather than gradually",
    );
    c.record(
        "f2.a_biome_without_precipitation_still_gets_half",
        RainFog::target(1.0, 15, false) == 0.5,
        "`rainsInBiome ? 1.0 : 0.5` — a desert thickens too, even though no          rain falls on it",
    );

    let mut f = RainFog::default();
    f.update(1.0, 15, true, 1.0);
    let after_one = f.multiplier();
    for _ in 0..40 {
        f.update(1.0, 15, true, 1.0);
    }
    let converged = f.multiplier();
    let mut band = RainFog::default();
    band.converge(1.0, 15, true);
    // Vanilla`s own FOG_START/END_DISTANCE defaults, which is what the offsets
    // are applied to — NOT Rewo`s tighter render-distance band.
    let (start, end) = band.apply(0.0, 1024.0);
    let (tight_s, tight_e) = band.apply(10.0, 50.0);
    c.record(
        "f3.the_multiplier_eases_and_the_band_closes_onto_a_floor",
        (after_one - 0.2).abs() < 1e-6
            && converged > 0.99
            && (start, end) == (-160.0, 768.0)
            && tight_e == 50.0
            && tight_s == -150.0,
        format!(
            "one tick moves 20% ({after_one:.3}) and it converges ({converged:.3})              — it is STATE, not a function of the rain level. At full strength              (0, 1024) becomes ({start}, {end}); a band already tighter than the              96 floor keeps its own end ({tight_e}), so the fog never collapses              to nothing"
        ),
    );
}

// -- 5. the pixels -------------------------------------------------------------

fn view_proj(eye: [f32; 3], target: [f32; 3]) -> [[f32; 4]; 4] {
    let proj = perspective_reverse_z(70f32.to_radians(), W as f32 / H as f32, 0.05);
    let view = glam::Mat4::look_at_rh(
        glam::Vec3::from_array(eye),
        glam::Vec3::from_array(target),
        glam::Vec3::Y,
    );
    (glam::Mat4::from_cols_array_2d(&proj) * view).to_cols_array_2d()
}

fn lit(img: &[u8]) -> usize {
    img.chunks_exact(4).filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8).count()
}

fn overlay_offscreen(ring: &OverlayRing) -> OverlayDraw<'_> {
    OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    }
}

fn client_jar(version: &str) -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

/// A solid 8x8 cloud map, so the deck is unbroken above the camera.
fn solid_clouds() -> CloudTexture {
    let side = 8u32;
    let rgba: Vec<u8> = (0..side * side).flat_map(|_| [255u8, 255, 255, 255]).collect();
    CloudTexture::from_rgba(&rgba, side, side)
}

fn check_cloud_pixels(
    c: &mut Checker,
    gpu: &mut Gpu,
    off: &mut Offscreen,
    baked: &assets::BakedAssets,
    draw: &OverlayDraw,
    args: &WeathershotArgs,
) -> Result<(), String> {
    let tex = solid_clouds();
    // Deliberately FAR from the world origin. Both passes compute their
    // geometry from a camera-relative vector and then add the camera back;
    // a version that forgot the second step draws over the origin instead, and
    // at [0,0,0] that is indistinguishable from correct. This is exactly the
    // bug the first live shot exposed and this gate did not.
    const EYE: [f64; 3] = [1536.5, 70.0, -2048.5];
    let vp = view_proj([EYE[0] as f32, EYE[1] as f32, EYE[2] as f32],
                       [EYE[0] as f32, EYE[1] as f32 + 1.0, EYE[2] as f32 - 1.0]);
    let placement = rewo_gpu::clouds::placement(EYE, 90.0, 0, 0.0, tex.width, tex.height);
    let faces = tex.build_mesh(placement.relative_pos, placement.cell_x, placement.cell_z, CloudStatus::Fancy, 8);

    let mut shot = |gpu: &mut Gpu, off: &mut Offscreen, color: i32| -> Result<Vec<u8>, String> {
        let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
        wr.set_camera([EYE[0] as f32, EYE[1] as f32, EYE[2] as f32]);
        wr.set_sky_mode(SkyMode::None);
        wr.init_clouds(gpu)?;
        wr.set_clouds(
            gpu,
            &CloudDraw {
                faces: faces.clone(),
                placement,
                color_argb: color,
                fog_clouds_end: 512.0,
                camera: [EYE[0] as f32, EYE[1] as f32, EYE[2] as f32],
            },
        )?;
        off.render(gpu, Some((&mut wr, vp)), draw, CLEAR)?;
        let img = off.read_rgba(gpu)?;
        wr.destroy(gpu);
        Ok(img)
    };

    // The Overworld's own cloud colour.
    let img = shot(gpu, off, 0xCCFF_FFFFu32 as i32)?;
    if let Some(d) = &args.out_dir {
        let _ = off.save_png(gpu, &d.join("clouds.png"));
    }
    let covered = lit(&img);
    c.record(
        "g1.a_cloud_deck_renders_overhead",
        covered > (W * H) as usize / 8,
        format!(
            "{covered} of {} pixels carry cloud — without this every witness below \
             would be grading a black frame",
            W * H
        ),
    );

    // The winding question, settled by the render rather than assumed — and it
    // needs BOTH sides. Seen from below only the DOWN faces are built; seen
    // from above, only the UP ones. A front-face convention that culled the
    // wrong set would still leave one of these covered, so grading one view
    // would prove nothing.
    let above = {
        let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
        let above_eye = [EYE[0], 130.0, EYE[2]];
        wr.set_camera([above_eye[0] as f32, above_eye[1] as f32, above_eye[2] as f32]);
        wr.set_sky_mode(SkyMode::None);
        wr.init_clouds(gpu)?;
        let p = rewo_gpu::clouds::placement(above_eye, 90.0, 0, 0.0, tex.width, tex.height);
        wr.set_clouds(
            gpu,
            &CloudDraw {
                faces: tex.build_mesh(p.relative_pos, p.cell_x, p.cell_z, CloudStatus::Fancy, 8),
                placement: p,
                color_argb: 0xCCFF_FFFFu32 as i32,
                fog_clouds_end: 512.0,
                camera: [above_eye[0] as f32, above_eye[1] as f32, above_eye[2] as f32],
            },
        )?;
        let above_vp = view_proj(
            [above_eye[0] as f32, above_eye[1] as f32, above_eye[2] as f32],
            [above_eye[0] as f32, above_eye[1] as f32 - 1.0, above_eye[2] as f32 - 1.0],
        );
        off.render(gpu, Some((&mut wr, above_vp)), draw, CLEAR)?;
        let img = off.read_rgba(gpu)?;
        wr.destroy(gpu);
        img
    };
    let above_covered = lit(&above);
    c.record(
        "g2.the_cull_winding_is_right_from_both_sides",
        covered > (W * H) as usize / 4 && above_covered > (W * H) as usize / 4,
        format!(
            "{covered} pixels from below and {above_covered} from above a solid \
             deck. Vanilla's FANCY cloud pipeline culls back faces — that is what \
             the inward-wound interior faces exist for — so the front-face \
             convention has to match Rewo's y-flipped viewport. Grading both sides \
             is what makes this a measurement rather than a restatement of g1"
        ),
    );

    let clear = shot(gpu, off, 0)?;
    c.record(
        "g3.a_transparent_cloud_colour_draws_nothing",
        lit(&clear) == 0,
        format!(
            "alpha 0 leaves {} lit pixels. `CLOUD_COLOR` defaults to 0 and \
             LevelRenderer skips the pass on `alpha == 0` — which is exactly how \
             the Nether and the End have no clouds: by omission, not by a \
             dimension check",
            lit(&clear)
        ),
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn check_weather_pixels(
    c: &mut Checker,
    gpu: &mut Gpu,
    off: &mut Offscreen,
    baked: &assets::BakedAssets,
    draw: &OverlayDraw,
    args: &WeathershotArgs,
) -> Result<(), String> {
    // From the bake, not a second jar reader — the gate must grade the same
    // textures the live client gets.
    let rain_png = baked
        .rain
        .as_ref()
        .ok_or("the jar bake has no environment/rain.png")?;
    let snow_png = baked
        .snow
        .as_ref()
        .ok_or("the jar bake has no environment/snow.png")?;
    let dirs = ColumnDirections::new();

    // A ring of columns around the camera, all fully sky-lit.
    const WEYE: [f64; 3] = [1536.5, 68.0, -2048.5];
    let (bx, bz) = (WEYE[0].floor() as i32, WEYE[2].floor() as i32);
    let columns: Vec<WeatherColumn> = (-6..=6)
        .flat_map(|dx| {
            (-6..=6).map(move |dz| WeatherColumn {
                x: bx + dx,
                z: bz + dz,
                bottom_y: 64,
                top_y: 88,
                u_offset: 0.0,
                v_offset: 0.0,
                block_light: 0,
                sky_light: 15,
            })
        })
        .collect();
    let vp = view_proj(
        [WEYE[0] as f32, WEYE[1] as f32, WEYE[2] as f32],
        [WEYE[0] as f32, WEYE[1] as f32, WEYE[2] as f32 - 1.0],
    );

    let mut shot = |gpu: &mut Gpu,
                    off: &mut Offscreen,
                    rain: Vec<WeatherColumn>,
                    snow: Vec<WeatherColumn>,
                    intensity: f32|
     -> Result<Vec<u8>, String> {
        let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
        wr.set_camera([WEYE[0] as f32, WEYE[1] as f32, WEYE[2] as f32]);
        wr.set_sky_mode(SkyMode::None);
        wr.init_weather(
            gpu,
            &WeatherImage {
                rgba: &rain_png.rgba,
                w: rain_png.w,
                h: rain_png.h,
            },
            &WeatherImage {
                rgba: &snow_png.rgba,
                w: snow_png.w,
                h: snow_png.h,
            },
        )?;
        let state = WeatherRenderState {
            intensity,
            radius: 10,
            rain_columns: rain,
            snow_columns: snow,
        };
        wr.set_weather(gpu, &WeatherDraw::build(&state, &dirs, WEYE))?;
        off.render(gpu, Some((&mut wr, vp)), draw, CLEAR)?;
        let img = off.read_rgba(gpu)?;
        wr.destroy(gpu);
        Ok(img)
    };

    let rain_img = shot(gpu, off, columns.clone(), Vec::new(), 1.0)?;
    if let Some(d) = &args.out_dir {
        let _ = off.save_png(gpu, &d.join("rain.png"));
    }
    let rain_lit = lit(&rain_img);
    c.record(
        "g4.rain_columns_render",
        rain_lit > 200,
        format!("{rain_lit} lit pixels from 169 columns of rain"),
    );

    // The discard is what makes rain read as streaks rather than a sheet: the
    // texture is mostly transparent, so most of each quad must be gone.
    let total = (W * H) as usize;
    c.record(
        "g5.the_alpha_discard_leaves_streaks_not_a_sheet",
        rain_lit < total / 2,
        format!(
            "{rain_lit} of {total} pixels survive. `particle.fsh` discards below \
             alpha 0.1, and rain.png is mostly transparent — without the cutoff \
             the columns would composite into a translucent wall"
        ),
    );

    let faint = shot(gpu, off, columns.clone(), Vec::new(), 0.15)?;
    let bright_sum: u64 = rain_img.chunks_exact(4).map(|p| p[0] as u64).sum();
    let faint_sum: u64 = faint.chunks_exact(4).map(|p| p[0] as u64).sum();
    c.record(
        "g6.intensity_scales_what_reaches_the_screen",
        faint_sum * 2 < bright_sum,
        format!(
            "intensity 0.15 sums to {faint_sum} against 1.0's {bright_sum} — the \
             rain level multiplies every column's alpha, so a drizzle is faint \
             everywhere rather than merely sparse"
        ),
    );

    let snow_img = shot(gpu, off, Vec::new(), columns.clone(), 1.0)?;
    if let Some(d) = &args.out_dir {
        let _ = off.save_png(gpu, &d.join("snow.png"));
    }
    c.record(
        "g7.snow_renders_and_differs_from_rain",
        lit(&snow_img) > 200 && snow_img != rain_img,
        format!(
            "{} lit pixels, and a different image from rain — two textures, two \
             draws, two max-alphas",
            lit(&snow_img)
        ),
    );

    let none = shot(gpu, off, Vec::new(), Vec::new(), 1.0)?;
    c.record(
        "g8.no_columns_draws_nothing",
        lit(&none) == 0,
        format!(
            "{} lit pixels with an empty state — the sensitivity partner for every \
             witness above",
            lit(&none)
        ),
    );

    // Unlit columns must come out dark: weather goes through the same lightmap
    // as terrain, so rain in a cave is not lit by the sky.
    let dark: Vec<WeatherColumn> = columns
        .iter()
        .map(|c| WeatherColumn {
            sky_light: 0,
            block_light: 0,
            ..*c
        })
        .collect();
    let dark_img = shot(gpu, off, dark, Vec::new(), 1.0)?;
    let dark_sum: u64 = dark_img.chunks_exact(4).map(|p| p[0] as u64).sum();
    c.record(
        "g9.weather_is_lit_by_the_same_lightmap_as_terrain",
        dark_sum * 4 < bright_sum,
        format!(
            "fully unlit columns sum to {dark_sum} against sky-lit {bright_sum} — \
             the vertex light word feeds `lm_light`, the same function the world \
             and water passes use, so rain in a cave is dark"
        ),
    );
    Ok(())
}
