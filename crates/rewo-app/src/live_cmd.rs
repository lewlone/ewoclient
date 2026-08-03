//! `rewo live` — the M3 capstone: connect, play, and SEE it. A real
//! windowed client — the M1 protocol + M3 physics session feeding the M2
//! renderer, with live re-meshing as the world changes.
//!
//! The main loop drives the 20 Hz tick on a 50 ms accumulator and renders
//! every frame from the player's eye. **Meshing happens off the frame**
//! (REWO_PLAN §4): dirty columns are snapshotted (`Arc`-shared, no copies)
//! and meshed on a rayon worker pool (`rewo_mesh::pool::MeshPool`); the
//! frame only uploads finished meshes, metered by a per-frame budget. The
//! socket reader is its own thread, as before.
//!
//! Headless-verifiable: `--run-seconds N` auto-exits and `--out PNG` writes
//! the final frame from the player's eye — a machine-checkable artifact
//! proving the live client renders the actual server world at the player's
//! position.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ash::vk;
use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use rewo_data::assets::{self, BakedFont};
use rewo_data::entity_types::EntityTypes;
use rewo_data::{DataPaths, GameData};
use rewo_gpu::celestial::{CelestialImage, CelestialState, CelestialTextures};
use rewo_gpu::end_sky::EndSkyImage;
use rewo_gpu::entities::{
    srgb_to_linear, EntityDraw, EntityModelKind, FontData, MobTexEntry, MobTextures,
};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::renderer::{RenderOutcome, Renderer};
use rewo_gpu::world::{SkyMode, WorldLightmapState, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_mesh::pool::{MeshPool, MeshTables};
use rewo_net::play::PlaySession;
use rewo_net::Connection;
use rewo_world::dimension::{DimensionTypeDef, Skybox};
use rewo_world::lightmap::{
    darkness_lightmap, night_vision_intensity, rgb24_to_vec3, sample, BlockLightFlicker,
    LightmapState,
};
use rewo_world::physics::{TickInput, EYE_HEIGHT};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::stats::{OverlayRing, StatsAccum};

const CLEAR_SKY: [f32; 4] = [0.184, 0.380, 1.0, 1.0];
const TICK_DT: f32 = 0.05; // 20 Hz
/// Max finished meshes uploaded to the GPU per frame — the rest stay queued
/// in the pool's result channel. (Meshing itself is unmetered: it runs on
/// the worker pool, off the frame.)
const UPLOAD_BUDGET: usize = 6;

#[derive(ClapArgs)]
pub struct LiveArgs {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 25599)]
    port: u16,
    /// Player name. Defaults to the launcher's `REWO_USERNAME` env handoff
    /// (REWO_PLAN §9.1), then "RewoLive".
    #[arg(long)]
    username: Option<String>,
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Auto-exit after N seconds (headless soak).
    #[arg(long)]
    run_seconds: Option<f32>,
    /// Write the final frame to this PNG (headless verification artifact).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Frames in flight (M6 latency knob): 1 = lowest latency, 2 = default.
    #[arg(long, default_value_t = 2)]
    fif: usize,
    /// Load an OptiFine CEM resource-pack zip (M9): mobs render with the
    /// pack's custom models. Also read from `REWO_PACK` if unset.
    #[arg(long)]
    pack: Option<PathBuf>,
    /// Brightness gamma (`Options.gamma`, default 0.5) — the notGamma lift
    /// weight in the camera lightmap (M13). Must be in `[0, 1]`.
    #[arg(long, default_value_t = 0.5)]
    gamma: f32,
    /// Darkness mob-effect scale (`Options.darknessEffectScale`, default 1.0)
    /// — scales the Darkness pulse in the camera lightmap (M13). `[0, 1]`.
    #[arg(long = "darkness-effect-scale", default_value_t = 1.0)]
    darkness_effect_scale: f32,
    /// Simulate players' capes as cloth instead of vanilla's rigid slab
    /// (M61). Off by default — vanilla is the default cape. Also read from
    /// `REWO_WAVY_CAPE=1`.
    #[arg(long = "wavy-cape", default_value_t = false)]
    wavy_cape: bool,
    #[arg(long, default_value_t = false)]
    no_validation: bool,
    /// M86's live gate. Runs the windowed client against a server, counts which
    /// bake-gated render paths the frame loop actually reached, proves each
    /// per-frame buffer ring rotates, and asserts the run was
    /// validation-clean — then prints one row per witness and exits non-zero if
    /// any failed. Implies `--run-seconds` if none was given.
    ///
    /// This is a **reachability** gate, not a pixel one. The pixels of every
    /// path it covers are already graded headlessly (`itemshot`, `handshot`,
    /// `weathershot`, `particleshot`, `bordershot`, `breakshot`); what none of
    /// them could see is that the windowed client never called into any of it.
    #[arg(long = "render-check", default_value_t = false)]
    render_check: bool,
}

/// How many seconds `--render-check` runs for when `--run-seconds` is absent.
///
/// Long enough for the server to send the inventory and for the weather /
/// particle paths to have been reached many times over, short enough to be a
/// gate rather than a soak.
const RENDER_CHECK_SECONDS: f32 = 8.0;

/// What `live --render-check` observed (M86).
///
/// Every field is a **count of frames on which something happened**, never a
/// snapshot, because the failure this gate exists for is "the branch never ran
/// at all" and a snapshot taken on the wrong frame cannot tell that from "it
/// ran and had nothing to do".
#[derive(Default)]
struct RenderCheck {
    frames: u64,
    /// `self.baked.is_some()` observed at the top of a frame. The witness whose
    /// absence let the bug live from M3 to M86.
    baked_frames: u64,
    /// Frames on which the rain-fog band came from the bake rather than from
    /// the `None` sentinel `[1e9, 1e9 + 1]`. A *value* witness, not a call
    /// count: the sentinel is what the dead branch produced, so a finite band
    /// cannot be faked by merely entering the arm.
    fog_band_frames: u64,
    gui_item_frames: u64,
    hand_frames: u64,
    weather_frames: u64,
    /// Frames on which the inventory screen was drawn. The gate opens it
    /// halfway through, so this is roughly half the run — it exists to prove
    /// the screen path (and with it `VelvetTextPass::sync_atlas`) was reached
    /// at all, not to pin a count.
    screen_frames: u64,
    /// Frames on which a CONTAINER's screen was drawn — a menu other than the
    /// player's own (M88).
    ///
    /// M87 shipped the container render and could not prove the windowed
    /// client reached it: `--render-check` opens the *inventory*, which is
    /// `menu_layout::PLAYER` and takes the pass's own `inventory.png` rect, so
    /// every container-specific path — the panel blits, the container-sized
    /// origin, the layout's own slot rects — stayed unexercised here. That is
    /// the exact shape of the blind spot M86 was: a path nothing drives.
    container_frames: u64,
    /// The panel height the RENDERER was holding while a container was open.
    ///
    /// Read back from `WorldRenderer::container_panel_height`, not from the
    /// open menu's layout — the first cut asked the layout, which answers 168
    /// for a chest whether or not the panel builder returned one, so it could
    /// not tell a working container from one that had silently fallen back to
    /// the player's 166-tall panel. A value witness is only a value witness if
    /// it reads the value the draw used.
    container_panel_h: Option<f32>,
    /// Frames on which a container was drawn **before** the gate force-opened
    /// the inventory (M89).
    ///
    /// The witness for `open_screen` opening the client's screen. M87 decoded
    /// the packet and drew whatever menu was open, but nothing turned the
    /// screen on, so a chest recorded its menu and showed nothing unless the
    /// player independently pressed E. These frames can only exist if the
    /// packet did it.
    container_self_opened_frames: u64,
    /// Passes actually constructed by the end of the run.
    gui_items_ready: bool,
    hand_ready: bool,
    clouds_ready: bool,
    weather_ready: bool,
    particles_ready: bool,
    border_ready: bool,
    crumbling_ready: bool,
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
    gui_item_last: u64,
    hand_last: u64,
    gui_item_orphans: u64,
    hand_orphans: u64,
    gui_item_max_live: usize,
    hand_max_live: usize,
    /// Last-seen rebuild counters, so a legitimate ring reset is not scored as
    /// a use-after-free.
    gui_item_generation: u64,
    hand_generation: u64,
    /// How many rebuilds happened, reported so a run where the exemption fired
    /// suspiciously often is visible rather than silently forgiven.
    gui_item_rebuilds: u64,
    hand_rebuilds: u64,
    /// Whether validation was actually on. Asserting "0 errors" is worthless
    /// without it — a run with the layer off reports 0 for free.
    validation: bool,
}

impl RenderCheck {
    /// Sample the ringed passes once per frame, after this frame's `set_*`
    /// calls and before the next one.
    fn sample_rings(&mut self, wr: &WorldRenderer) {
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
    fn report(&self) -> bool {
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
            "r17 validation was enabled",
            self.validation,
            format!("{}", self.validation),
        );
        row(
            "r18 the session was validation-clean",
            vuids == 0,
            format!("{vuids} errors"),
        );

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
        pass == rows.len()
    }
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
fn validate_unit(name: &str, v: f32) -> Result<(), String> {
    if !v.is_finite() || !(0.0..=1.0).contains(&v) {
        return Err(format!(
            "--{name} must be a finite value in [0, 1], got {v}"
        ));
    }
    Ok(())
}

pub fn run(args: LiveArgs) -> Result<(), String> {
    // M13 camera-lightmap options — validate BEFORE loading/baking/connecting
    // so a bad value fails fast without any side effects.
    validate_unit("gamma", args.gamma)?;
    validate_unit("darkness-effect-scale", args.darkness_effect_scale)?;

    let data = GameData::load_for_version(&args.version)?;
    let jar = client_jar_path(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    // `ItemTags.SPEARS`, from the data pack the client jar ships — the tag
    // `AvatarRenderer.getArmPose` tests to pose a *held* (not swinging) spear.
    let spears = rewo_data::item_tags::ItemTag::load_spears(&jar, &data.items)?;
    // M25b: chest facing + material, per block state.
    let chest_states = rewo_data::chest_states::ChestStates::load(&paths.blocks_json())?;
    // M25e: the text transform per sign state.
    let sign_states = rewo_data::sign_states::SignStates::load(&paths.blocks_json())?;
    // M20: item identities the mob arm rigs test against.
    let bow_item = data.items.id("minecraft:bow");
    // Shared with the entity collector for held-item id → name (M22).
    let items = std::sync::Arc::new(data.items.clone());
    let _ = CROSSBOW_ITEM.set(data.items.id("minecraft:crossbow"));
    // M73: the crosshair entity pick's two version tables. Both fail loud
    // here rather than at the first frame — a drifted table would otherwise
    // show up as "nothing is ever under the crosshair".
    let _ = PICK_SHAPES.set(rewo_data::entity_pick::EntityPickTable::resolve(
        &data.entity_types,
    )?);
    let _ = REDIRECTABLE.set(
        rewo_data::entity_pick::EntityTypeTag::load_redirectable_projectile(
            &jar,
            &data.entity_types,
        )?,
    );
    // Per-state collision shapes (slabs/stairs/fences, not just full cubes).
    let collide: Vec<Vec<[f32; 6]>> = baked.collide.clone();
    let global_bits = data.blocks.global_palette_bits;
    // Launcher account handoff — online-mode servers need it, offline
    // servers ignore it. The explicit --username wins for the name.
    let auth = rewo_net::crypt::OnlineAuth::from_env();
    let username = args
        .username
        .clone()
        .or_else(|| auth.as_ref().map(|a| a.username.clone()))
        // Offline launcher handoff sets the name without a token.
        .or_else(|| std::env::var("REWO_USERNAME").ok())
        .unwrap_or_else(|| "RewoLive".into());

    let dirt_item = data.items.id("dirt");
    let colormaps = rewo_world::biome::Colormaps::from_pixels(
        baked.grass_colormap.clone(),
        baked.foliage_colormap.clone(),
        baked.dry_foliage_colormap.clone(),
    );
    let conn = Connection::connect(&args.host, args.port, &data)?;
    let mut session = conn.into_play(
        &args.host,
        args.port,
        &username,
        auth.as_ref(),
        collide,
        global_bits,
        colormaps,
    )?;
    // Entity collision: per-type footprint + whether it shoves (living only).
    session.entity_push = entity_push_table(&data.entity_types);
    // Resolve the kinds whose entity events drive model rigs — a
    // `ClientboundEntityEventPacket` byte is polymorphic by entity class, so
    // the id alone can't name the animation.
    session.warden_type_id = data.entity_types.id_of("minecraft:warden");
    session.armadillo_type_id = data.entity_types.id_of("minecraft:armadillo");
    // The Allay's type id disambiguates its index-16 `DATA_DANCING` from the
    // modeled baby path at the same slot (both index 16, both BOOLEAN).
    session.allay_type_id = data.entity_types.id_of("minecraft:allay");
    // M20: the index-17 BOOLEAN is `Pillager.IS_CHARGING_CROSSBOW`.
    session.pillager_type_id = data.entity_types.id_of("minecraft:pillager");
    // M52: the two kinds that disambiguate an otherwise-shared metadata slot —
    // the sheep's wool byte at 18 and the creaking's `IS_ACTIVE` at 17.
    session.sheep_type_id = data.entity_types.id_of("minecraft:sheep");
    session.creaking_type_id = data.entity_types.id_of("minecraft:creaking");
    // M60: the player, for the index-16 skin-customisation byte (cape bit).
    session.player_type_id = Some(data.entity_types.player_id);
    // M81: `handleTakeItemEntity` branches three ways on the collected
    // entity's class — an item's stack is shrunk and only then removed, an
    // experience orb is never removed here at all, and anything else goes
    // immediately. The two ids are what tell those apart.
    session.take_item_kinds = rewo_net::TakeItemKinds {
        item: data.entity_types.id_of("minecraft:item"),
        orb: data.entity_types.id_of("minecraft:experience_orb"),
        local_player: None,
    };
    // M64: the six mobs whose texture a metadata field chooses. An id that
    // does not resolve leaves that mob on its baked texture.
    session.variant_type_ids = rewo_net::VariantKinds {
        cat: data.entity_types.id_of("minecraft:cat"),
        wolf: data.entity_types.id_of("minecraft:wolf"),
        frog: data.entity_types.id_of("minecraft:frog"),
        axolotl: data.entity_types.id_of("minecraft:axolotl"),
        horse: data.entity_types.id_of("minecraft:horse"),
        llama: data.entity_types.id_of("minecraft:llama"),
        // M68: the index-17 INT. Not a texture id — the packed
        // (shape, pattern, body colour, pattern colour) — but it rides the
        // same setter, and the gate matters because index 17 already carries
        // a spellcaster BYTE and a pillager/creaking BOOLEAN.
        tropical_fish: data.entity_types.id_of("minecraft:tropical_fish"),
    };
    // M61: opt-in cloth capes. Off leaves `EntityTable` allocating and
    // ticking nothing, so the vanilla cape path is exactly M60's.
    if wavy_cape_requested(args.wavy_cape) {
        log::info!("live: wavy capes on ({} segments)", rewo_world::wavy_cape::SEGMENTS);
        session.world.entities.set_wavy_capes(true);
    }
    // M26: `block_event`'s `b0 == 1` means a different thing to each block
    // entity, so the type is what selects the body. Resolving through the
    // classification table rather than looking three names up directly is what
    // makes this a boundary: `resolve` errors on a registered type nobody has
    // classified, so a version that adds one stops the client here instead of
    // dropping it silently. That check used to run only in the gate.
    let be_registry = rewo_world::block_entities::BlockEntityRegistry::resolve(
        &rewo_data::block_entity_types::load(&paths.registries_json())?,
    )?;
    session.block_event_types = be_registry.block_event_types();
    // Which skull states are POWERED, so their animation counters run (M29).
    session.powered_skull_states = chest_states.powered_skull_states().clone();
    // A conduit scans for its own frame (M30), so it needs the block states
    // that count as water and as prismarine, resolved once from the bake.
    session.conduit_states = chest_states.conduit_states().clone();
    session.water_states = baked.water.clone();
    session.conduit_frame_states = {
        let mut v = vec![false; baked.water.len()];
        for id in 0..v.len() as u32 {
            if let Some(name) = data.blocks.block_name(id) {
                if rewo_world::conduit::FRAME_BLOCKS.contains(&name) {
                    v[id as usize] = true;
                }
            }
        }
        v
    };
    // M19 combat swings: the machine-extracted living / swing-ticking sets gate
    // every swing input and decide whose clock runs (`updateSwingTime` is not
    // universal), and the equipment tables decide how long each swing lasts and
    // which arm animation it plays.
    session.entity_classes = Some(std::sync::Arc::new(data.entity_classes));
    // M72 passenger positioning: with the attachment table installed,
    // `EntityTable::tick_lerp` re-derives every rider's position from its
    // vehicle at the end of each tick, exactly as `tickPassenger` →
    // `rideTick` → `positionRider` does. Without it a rider renders at its own
    // stale synced position and floats beside the mount.
    session
        .world
        .entities
        .set_attachments(std::sync::Arc::new(data.entity_attachments));
    // The component walker is keyed by name and the wire by id, so the table
    // is installed once the registry is known. Without this every component is
    // unwalkable and the first enchanted sword in a packet costs every stack
    // after it — so the count is logged rather than assumed.
    {
        let n = rewo_net::component_wire::install_shapes(data.component_registry.ids());
        log::info!(
            "rewo-net: {n}/{} data component codec(s) transcribed of {} registered",
            rewo_net::component_wire::CODECS.len(),
            data.component_registry.len()
        );
    }
    // …and kept, because M66's advanced tooltip has to walk the other way: a
    // patch's raw ids back to names, so the item's prototype table can say
    // whether each one is an addition or an override.
    session.component_names = Some(std::sync::Arc::new(data.component_registry));
    session.swing_data = Some(rewo_net::item_stack::SwingWireData {
        prototypes: data.swing_animations,
        components: data.components,
        use_profiles: data.use_profiles,
    });
    // Client-side relighting of our own edits — the server only sends light
    // on chunk load, never for a placed torch or a broken roof.
    session.set_light_tables(
        baked.emission.clone(),
        baked.dampening.clone(),
        baked.face_occludes.clone(),
    );
    log::info!("live: session up, opening window…");
    let etypes = data.entity_types;
    // M84: the three registries the statistics screen resolves against.
    let stat_registries = data.stat_registries;
    // M52 attributes: the type registry turns a spawned entity's type id into
    // the name `DefaultAttributes.SUPPLIERS` is keyed by, and the attribute
    // registry supplies both the clamp and the supplier filter. Without both,
    // `route_update_attributes` recognises the packet and stores nothing.
    session.entity_types = Some(std::sync::Arc::new(etypes.clone()));
    session.attribute_registry = Some(data.attributes.clone());

    let want_validation = cfg!(debug_assertions) && !args.no_validation;
    match &args.out {
        // Headless: pump the session until spawn + a settle window, render
        // one frame from the eye, save. No window at all.
        Some(out) if args.run_seconds.is_none() => {
            let settle = std::env::var("REWO_SETTLE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(6.0);
            run_headless(
                session,
                baked,
                etypes,
                spears,
                chest_states,
                sign_states,
                bow_item,
                items,
                want_validation,
                out,
                settle,
                dirt_item,
                args.pack.clone(),
                args.gamma,
                args.darkness_effect_scale,
            )
        }
        _ => run_windowed(
            session,
            baked,
            etypes,
            stat_registries,
            spears,
            chest_states,
            sign_states,
            bow_item,
            items,
            args,
            want_validation,
            dirt_item,
        ),
    }
}

/// Velvet-tinted linear colors for the capsule set.
fn linear_rgb(r: u8, g: u8, b: u8) -> [f32; 3] {
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
    uv: [f32; 2],
    slim: bool,
    /// Atlas origin of this player's cape slot, if one was uploaded.
    cape: Option<(u32, u32)>,
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
    req_tx: std::sync::mpsc::Sender<(u128, TexKind, String, bool)>,
    res_rx: std::sync::mpsc::Receiver<(u128, TexKind, bool, Vec<u8>)>,
    requested: std::collections::HashSet<(u128, TexKind)>,
    registry: SkinRegistry,
}

impl SkinLoader {
    fn new() -> Self {
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
    fn request(&mut self, uuid: u128, info: &rewo_net::skins::SkinInfo) {
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
    fn poll_uploads(&mut self, gpu: &mut Gpu, wr: &mut WorldRenderer) {
        while let Ok((uuid, kind, slim, rgba)) = self.res_rx.try_recv() {
            match kind {
                TexKind::Skin => {
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
struct GestureEntry {
    gesture: rewo_gpu::mobs::Gesture,
    start_seconds: f32,
    start_tick: i64,
}

#[derive(Default)]
pub(crate) struct GestureTracker {
    map: std::collections::HashMap<i32, GestureEntry>,
}

impl GestureTracker {
    /// Record `wanted` for this entity at `now` seconds / `tick`; returns the
    /// active gesture, its age in seconds, and the network tick it started on.
    /// `head_start` pre-advances a *newly entered* gesture's clock (vanilla's
    /// SCARED `fastForward`). A repeated *same* gesture keeps its original
    /// transition tick — so metadata that re-arrives without an actual pose
    /// change is not a new transition.
    fn update(
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
fn event_age_seconds(start_tick: Option<i64>, tick: i64, alpha: f32) -> Option<f32> {
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
fn wanted_gesture(kind: EntityModelKind, pose: u8, state: u8) -> Option<rewo_gpu::mobs::Gesture> {
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
fn resolve_wavy_cape(
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
fn root_vehicle_of(ents: &rewo_world::entities::EntityTable, id: i32) -> i32 {
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
fn crossbow_item_id() -> Option<i32> {
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
fn emissive_state(
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
const WARDEN_TENDRIL_TICKS: f32 = 10.0;

/// Choose one entity's ETF texture variant. The properties are keyed by the
/// mob-texture key, so a mob whose model uses several textures varies on its
/// *first* — the one a pack's `<entity>.properties` names.
fn etf_variant(
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

fn collect_entities<'a>(
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
    let force_limb: Option<(f32, f32)> = std::env::var("REWO_FORCE_LIMB").ok().and_then(|s| {
        let mut it = s.split(',');
        Some((
            it.next()?.trim().parse().ok()?,
            it.next()?.trim().parse().ok()?,
        ))
    });
    // Headless-only knob: `REWO_FORCE_HEAD=<degrees>` cranks every mob's head
    // yaw to body-yaw + this offset, so a PNG can prove head-look turns the
    // head independently of the body without depending on live server AI.
    let force_head: Option<f32> = std::env::var("REWO_FORCE_HEAD")
        .ok()
        .and_then(|s| s.trim().parse().ok());
    // Headless-only knob: `REWO_FORCE_GESTURE=<name>[,<age_s>]` pins every
    // gesture-rigged mob into that state (mobshot names, e.g.
    // "warden_roar,1.5") — deterministic gesture PNGs without server AI.
    let force_gesture: Option<(rewo_gpu::mobs::Gesture, f32)> =
        std::env::var("REWO_FORCE_GESTURE").ok().and_then(|s| {
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
            let cem = crate::mobshot_cmd::load_cem_overrides(&path)?;
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

pub(crate) fn entity_textures(baked: &assets::BakedAssets) -> MobTextures<'_> {
    MobTextures {
        // M64: vanilla's own metadata-driven alternates are always present —
        // they are jar textures, not a pack's, so they do not wait for one.
        // `entity_textures_with` appends a pack's ETF alternates after these;
        // the two live in disjoint id bands.
        variants: vanilla_variants(baked),
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
fn vanilla_variants(baked: &assets::BakedAssets) -> Vec<rewo_gpu::entities::VariantTexEntry<'_>> {
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

/// Borrow the baked HUD sprites into the renderer's view type.
fn hud_sprite(sp: &assets::HudSprite) -> rewo_gpu::hud::HudSpriteData<'_> {
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
        food_full: hud_sprite(&h.food_full),
        food_half: hud_sprite(&h.food_half),
        food_empty: hud_sprite(&h.food_empty),
        experience_bar_background: hud_sprite(&h.experience_bar_background),
        experience_bar_progress: hud_sprite(&h.experience_bar_progress),
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
fn eye_view_proj(
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
fn eye_view(eye: Vec3, yaw_deg: f32, pitch_deg: f32) -> Mat4 {
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
fn local_hurt_tilt(session: &PlaySession, alpha: f32, strength: f32) -> HurtTilt {
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

fn player_eye(session: &PlaySession) -> Vec3 {
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
fn mesh_output_is_stale(out_generation: u64, current_generation: u64) -> bool {
    out_generation != current_generation
}

/// Per-frame mesh pump. Frees removed columns, uploads up to
/// `upload_budget` finished meshes from the worker pool, then submits
/// fresh snapshots for dirty columns. Returns how many meshes uploaded.
fn pump_meshing(
    session: &mut PlaySession,
    gpu: &mut Gpu,
    world_renderer: &mut WorldRenderer,
    pool: &mut MeshPool,
    upload_budget: usize,
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

fn col_dist((cx, cz): (i32, i32), px: f32, pz: f32) -> f32 {
    let x = cx as f32 * 16.0 + 8.0 - px;
    let z = cz as f32 * 16.0 + 8.0 - pz;
    x * x + z * z
}

// -- headless ---------------------------------------------------------------

fn run_headless(
    mut session: PlaySession,
    baked: assets::BakedAssets,
    etypes: EntityTypes,
    spears: rewo_data::item_tags::ItemTag,
    // Chest block states → facing + material, for the M25b block-entity draws.
    chest_states: rewo_data::chest_states::ChestStates,
    sign_states: rewo_data::sign_states::SignStates,
    bow_item: Option<i32>,
    items: std::sync::Arc<rewo_data::items::Items>,
    want_validation: bool,
    out: &std::path::Path,
    settle_seconds: f32,
    dirt_item: Option<i32>,
    pack: Option<PathBuf>,
    gamma: f32,
    darkness_option: f32,
) -> Result<(), String> {
    let _ = dirt_item;
    let mut gpu = Gpu::new(None, want_validation)?;
    let mut off = Offscreen::new(&mut gpu, 1280, 720)?;
    let mut world_renderer =
        WorldRenderer::new(&mut gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    let etf = init_entities_maybe_cem(&mut world_renderer, &mut gpu, &baked, &pack)?;
    // M22: the baked held-item models, converted across the crate seam.
    world_renderer.set_held_items(to_gpu_held_items(&baked.held_items));
    init_celestial_if_present(&mut world_renderer, &mut gpu, &baked)?;
    init_weather_if_present(&mut world_renderer, &mut gpu, &baked)?;
    init_particles_if_present(&mut world_renderer, &mut gpu, &baked)?;
    init_crumbling_if_present(&mut world_renderer, &mut gpu, &baked)?;
    let mut weather_assets = WeatherAssets::new(&baked);
    let mut gui_items = GuiItemState::new(&baked);
    let mut particle_assets = ParticleAssets::new(&baked);
    world_renderer.set_animations(layer_animations(&baked));
    if let Some(hud) = hud_sprites(&baked) {
        world_renderer.init_hud(&mut gpu, &hud)?;
    }
    let locator_styles = match locator_sprites(&baked) {
        Some(l) => {
            let styles = l.styles.clone();
            world_renderer.init_locator_bar(&mut gpu, &l)?;
            styles
        }
        None => Vec::new(),
    };
    if let Some(c) = container_sprites(&baked) {
        world_renderer.init_container(&mut gpu, &c)?;
    }
    if let Some(font) = font_data(&baked) {
        world_renderer.init_text(&mut gpu, &font)?;
    }

    // Pump the session on a real 20 Hz clock until spawned + settled, so
    // chunks arrive and the player position is real.
    let start = Instant::now();
    let idle = TickInput::default();
    let mut tick = 0u64;
    // M35 headless click state (`REWO_CLICK`).
    let mut clicked = false;
    let mut click_resyncs_at: Option<u32> = None;
    let mut summoned = false;
    // The block-light flicker (M13): a 48-bit LCG advanced exactly once per
    // successful 20 Hz client tick, mirroring `LightmapRenderStateExtractor`.
    let mut flicker = BlockLightFlicker::random();
    while start.elapsed().as_secs_f32() < settle_seconds {
        let deadline = start + Duration::from_millis(50) * (tick as u32 + 1);
        session.tick(&idle)?;
        flicker.tick();
        if let Some(reason) = &session.disconnect {
            return Err(format!("disconnected: {reason}"));
        }
        // REWO_SUMMON=mob: once spawned, /summon a mob ~3 blocks in front
        // (op required) so the model can be verified without a live one.
        if !summoned && session.spawned {
            // REWO_PRECMD: run one op command before the summon (e.g. clear
            // prior test mobs with `kill @e[type=husk]`), so a re-run starts
            // from a clean scene.
            if let Ok(cmd) = std::env::var("REWO_PRECMD") {
                // Semicolon-separated, so a scene that needs several commands
                // (a `clear` then a handful of `give`s) is still one knob.
                for one in cmd.split(';').map(str::trim).filter(|c| !c.is_empty()) {
                    let _ = session.send_command(one);
                    log::info!("REWO_PRECMD: {one}");
                }
                if !cmd.is_empty() {
                    std::env::remove_var("REWO_PRECMD");
                }
            }
            if let Ok(mob) = std::env::var("REWO_SUMMON") {
                let dir = look_dir(session.player.yaw, 0.0);
                let dist = std::env::var("REWO_SUMMON_DIST")
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(3.0);
                // Optional vertical offset — float the mob into empty sky so a
                // verification shot isn't occluded by ground clutter.
                let dy: f64 = std::env::var("REWO_SUMMON_DY")
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0.0);
                let (sx, sy, sz) = (
                    session.player.x + dir[0] * dist,
                    session.player.y + dy,
                    session.player.z + dir[2] * dist,
                );
                // Optional NBT tail (e.g. REWO_SUMMON_NBT={CustomName:'"Bo"'}).
                let nbt = std::env::var("REWO_SUMMON_NBT").unwrap_or_default();
                let cmd = format!("summon minecraft:{mob} {sx:.2} {sy:.2} {sz:.2} {nbt}");
                if let Err(e) = session.send_command(&cmd) {
                    log::warn!("REWO_SUMMON: {e}");
                }
                log::info!("REWO_SUMMON: {cmd}");
                summoned = true;
            }
        }
        // REWO_CHAT: send a chat line once (verifies the chat overlay).
        if summoned || session.spawned {
            if let Ok(msg) = std::env::var("REWO_CHAT") {
                if !msg.is_empty() {
                    let _ = session.send_chat(&msg);
                    std::env::remove_var("REWO_CHAT");
                }
            }
        }
        // M35: `REWO_CLICK=<menu slot>[,<button>]` clicks one inventory slot
        // once the contents have arrived, then keeps ticking so the server's
        // answer lands before the frame is drawn. A rejected prediction comes
        // back as a whole-container update, which `inventory.content_updates`
        // counts — so this is a real end-to-end gate, not a "the packet was
        // written" claim.
        // Not the moment the first stack arrives: `/give` sends one container
        // update per item and each advances the server's state id, so a click
        // fired mid-give would echo a stale one and be resynced. Forty ticks
        // is two seconds of quiet.
        if !clicked && session.spawned && !session.inventory.is_empty() && tick >= 40 {
            // `REWO_DUMP_INVENTORY=1`: print every occupied slot once the
            // container has settled. The components are the point — a stack
            // that decoded at all proves the walk reached the end of its
            // patch, and the values prove it read the right bytes (M41).
            if std::env::var("REWO_DUMP_INVENTORY").is_ok() {
                for i in 0..rewo_world::inventory::MENU_SLOTS {
                    if let Some(s) = session.inventory.menu_slot(i) {
                        println!(
                            "[rewo-m41] slot {i:2}: item {:4} x{:<3} components {:#018x}                              damage {:?} max_damage {:?} enchanted {}",
                            s.item_id, s.count, s.components, s.damage, s.max_damage, s.enchanted
                        );
                    }
                }
                std::env::remove_var("REWO_DUMP_INVENTORY");
            }
            if let Ok(whole) = std::env::var("REWO_CLICK") {
              let before_all = session.inventory.content_updates();
              // Semicolon-separated, so a run can pick a stack up and then do
              // something with it — a drag needs a stack on the cursor, and no
              // single click can leave one there and use it.
              for spec in whole.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                // `d:<slot>,<slot>,…[,one]` is a quick-craft drag over those
                // slots; the trailing `one` selects type 1 (one per slot).
                if let Some(rest) = spec.strip_prefix("d:") {
                    let one = rest.trim_end().ends_with("one");
                    let kind = if one {
                        rewo_world::inventory::QUICK_CRAFT_ONE
                    } else {
                        rewo_world::inventory::QUICK_CRAFT_SPLIT
                    };
                    let touched: Vec<usize> = rest
                        .split(',')
                        .filter_map(|v| v.trim().parse::<usize>().ok())
                        .collect();
                    let props = |id: i32| item_props(&items, id);
                    let accepted = session.inventory.quick_craft_accepts(&touched, kind, &props);
                    match session.shown_menu_mut().click_quick_craft(&accepted, kind, &props) {
                        Some(end) => {
                            use rewo_world::inventory::Inventory as Inv;
                            let input = rewo_world::inventory::CONTAINER_INPUT_QUICK_CRAFT;
                            let carried = session.inventory.carried();
                            let phase = |slot: i16, header: i32| {
                                rewo_world::inventory::ClickPrediction {
                                    slot,
                                    button: Inv::quick_craft_button(kind, header),
                                    changed: Vec::new(),
                                    carried,
                                }
                            };
                            let no_slot = rewo_world::inventory::QUICK_CRAFT_NO_SLOT;
                            let mut ok = session
                                .container_click_input(&phase(no_slot, 0), input)
                                .is_ok();
                            for &sl in &accepted {
                                ok = ok
                                    && session
                                        .container_click_input(&phase(sl as i16, 1), input)
                                        .is_ok();
                            }
                            if ok && session.container_click_input(&end, input).is_ok() {
                                session.shown_menu_mut().apply_prediction(&end);
                                println!(
                                    "[rewo-m35] DRAG over {accepted:?} type {kind}: \
                                     3 + {} packet(s), {} changed slot(s), carried {:?}",
                                    accepted.len(),
                                    end.changed.len(),
                                    session.inventory.carried()
                                );
                            } else {
                                println!("[rewo-m35] DRAG send failed");
                            }
                        }
                        None => println!("[rewo-m35] DRAG over {accepted:?}: not predictable"),
                    }
                    continue;
                }
                let mut parts = spec.split(',');
                let slot: i32 = parts.next().and_then(|v| v.trim().parse().ok()).unwrap_or(-1);
                let button: i8 = parts.next().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                let props = |id: i32| item_props(&items, id);
                // `REWO_CLICK=<slot>,<button>[,<kind>]`, where `kind` selects
                // the `ContainerInput`: `q` quick-move, `s` swap (the button
                // is then an inventory index), `t` throw, `a` pickup-all.
                let kind = spec
                    .split(',')
                    .nth(2)
                    .map(|f| f.trim().to_string())
                    .unwrap_or_default();
                let (input, predicted) = match kind.as_str() {
                    "q" => (
                        rewo_world::inventory::CONTAINER_INPUT_QUICK_MOVE,
                        session.shown_menu_mut().click_quick_move(slot, &props),
                    ),
                    "s" => (
                        rewo_world::inventory::CONTAINER_INPUT_SWAP,
                        session.shown_menu_mut().click_swap(slot, button as i32, &props),
                    ),
                    "t" => (
                        rewo_world::inventory::CONTAINER_INPUT_THROW,
                        session.shown_menu_mut().click_throw(slot, button, &props),
                    ),
                    "a" => (
                        rewo_world::inventory::CONTAINER_INPUT_PICKUP_ALL,
                        session.shown_menu_mut().click_pickup_all(slot, button, &props),
                    ),
                    _ => (0, session.shown_menu_mut().click_pickup(slot, button, &props)),
                };
                match predicted {
                    Some(prediction) => {
                        match session.container_click_input(&prediction, input) {
                            Ok(()) => {
                                session.shown_menu_mut().apply_prediction(&prediction);
                                println!(
                                    "[rewo-m35] CLICK slot {slot} button {button} input \
                                     {input}: predicted {} changed slot(s), carried {:?}",
                                    prediction.changed.len(),
                                    session.inventory.carried()
                                );
                            }
                            Err(e) => println!("[rewo-m35] CLICK send failed: {e}"),
                        }
                    }
                    None => println!("[rewo-m35] CLICK slot {slot}: not predictable"),
                }
              }
              // One window over the whole sequence, so a drag's three packets
              // are graded together with the click that set it up.
              click_resyncs_at = Some(before_all);
              clicked = true;
              std::env::remove_var("REWO_CLICK");
            }
        }
        tick += 1;
        let now = Instant::now();
        if now < deadline {
            std::thread::sleep(deadline - now);
        }
    }
    if let Some(before) = click_resyncs_at {
        let after = session.inventory.content_updates();
        println!(
            "[rewo-m35] CLICK result: {} container resync(s) after the click — {}",
            after - before,
            if after == before {
                "the server accepted the prediction"
            } else {
                "REJECTED, the server re-sent the whole container"
            }
        );
    }
    if !session.spawned {
        return Err("never spawned".into());
    }
    // Mesh everything loaded — one shot, parallel across the rayon pool
    // (order-preserving; nothing mutates the world while this runs).
    for (cx, cz) in session.take_removed() {
        world_renderer.remove_column(&mut gpu, cx, cz);
    }
    let _ = session.take_dirty(); // superseded — we mesh every column below
    let mut coords = session.world.column_coords();
    coords.sort_unstable();
    let t0 = Instant::now();
    let outputs = rewo_mesh::pool::mesh_all(
        session.dimension_generation,
        &session.world,
        &baked.render,
        &baked.models,
        &coords,
    );
    let mut meshed = 0usize;
    for out in outputs {
        if let Some(mesh) = out.mesh {
            world_renderer.upload_column(
                &mut gpu,
                out.cx,
                out.cz,
                bytemuck::cast_slice(&mesh.vertices),
                &mesh.indices,
                bytemuck::cast_slice(&mesh.tvertices),
                &mesh.tindices,
                mesh.y_min,
                mesh.y_max,
            )?;
            meshed += 1;
        }
    }
    log::info!(
        "live: meshed {} of {} columns in {:.1} ms (parallel one-shot)",
        meshed,
        coords.len(),
        t0.elapsed().as_secs_f32() * 1000.0
    );
    gpu.wait_idle();

    // Look slightly down from the eye toward the horizon (or a debug
    // pitch) — unless REWO_LOOK_ENTITY=1, which aims at the nearest entity
    // so the verification PNG is guaranteed to frame it.
    let eye = player_eye(&session);
    // Single-shot render: a fresh tracker sees every gesture at age 0 (use
    // REWO_FORCE_GESTURE for a specific rig time).
    let mut gestures = GestureTracker::default();
    // Headless single-frame: skins can't finish fetching in time (use
    // `mobshot --skin` for a deterministic real-skin PNG). Empty registry.
    let skins = SkinRegistry::new();
    // Resolve ONE camera lightmap for this frame at a FIXED partial of 1.0,
    // matching vanilla `GameRenderer` (lines 376-386):
    // `lightmapRenderStateExtractor.extract(lightmapRenderState, 1.0F)`. Feed
    // the identical value to both the entity-light sampler and the renderer
    // uniform, so mobs and terrain share a lightmap.
    let partial = 1.0;
    let snapshot = session.visual_effect_snapshot(partial);
    let lightmap = resolve_lightmap(
        session.day_ticks,
        session.active_dimension_type.as_ref(),
        flicker.block_factor(),
        snapshot,
        gamma,
        darkness_option,
        partial,
    );
    // Drained before `collect_entities` takes its long-lived borrow of the
    // session; the particle spawn happens further down, once the renderer is
    // ready for it.
    let particle_events = std::mem::take(&mut session.particle_events);
    // M48: permute + upload any trim sprites this frame needs, before the
    // draws take their borrow of the session.
    let trim_slots = ensure_trims(&session, &items, &baked.trims, &mut gpu, &mut world_renderer);
    let draws = collect_entities(
        &session,
        &etypes,
        1.0,
        &mut gestures,
        0.0,
        &skins,
        &lightmap,
        &spears,
        bow_item,
        &items,
        &baked.equipment,
        &trim_slots,
        &etf,
        // The headless one-shot has no key handling, so the HUD is never
        // hidden. `REWO_HUD_HIDDEN=1` is the knob that lets a gate or a
        // scripted shot exercise F1's suppression without a keyboard.
        std::env::var("REWO_HUD_HIDDEN").is_ok_and(|v| v.trim() == "1"),
        frame_crosshair_pick(&session, &etypes, 1.0),
    );
    for (id, e) in session.world.entities.iter() {
        let p = e.render_pos(1.0);
        println!(
            "[rewo-entities] #{id} {} at ({:.2},{:.2},{:.2}) yaw {:.0}{}",
            etypes.name(e.type_id).unwrap_or("?"),
            p[0],
            p[1],
            p[2],
            e.yaw,
            session
                .world
                .entities
                .name_of(e.uuid)
                .map(|n| format!(" name \"{n}\""))
                .unwrap_or_default(),
        );
    }
    println!("[rewo-entities] {} tracked", draws.len());
    let mut yaw = session.player.yaw;
    let mut pitch = std::env::var("REWO_PITCH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    // REWO_LOOK_AT="x,y,z": aim the camera at a fixed world point, bypassing
    // the entity search — deterministic framing for a summoned target even in
    // a scene full of other entities of the same kind.
    if let Some(pt) = std::env::var("REWO_LOOK_AT").ok().and_then(|s| {
        let mut it = s.split(',');
        Some(Vec3::new(
            it.next()?.trim().parse().ok()?,
            it.next()?.trim().parse().ok()?,
            it.next()?.trim().parse().ok()?,
        ))
    }) {
        let d = pt - Vec3::new(eye.x, eye.y, eye.z);
        let len = d.length().max(1e-4);
        yaw = (-d.x).atan2(d.z).to_degrees();
        pitch = (-d.y / len).asin().to_degrees();
        log::info!(
            "live: LOOK_AT {pt:?} from eye ({:.1},{:.1},{:.1}) -> yaw {yaw:.1} pitch {pitch:.1}",
            eye.x,
            eye.y,
            eye.z
        );
    } else if std::env::var("REWO_LOOK_ENTITY").is_ok() {
        // Aim at the nearest interesting model: a player, or (REWO_LOOK=slime)
        // the nearest slime; else the nearest anything.
        let d = |e: &EntityDraw| {
            (e.pos[0] - eye.x).powi(2) + (e.pos[1] - eye.y).powi(2) + (e.pos[2] - eye.z).powi(2)
        };
        let look = std::env::var("REWO_LOOK").ok();
        let look_kind = look
            .as_deref()
            .map(|s| rewo_gpu::mobs::kind_for_entity_name(&format!("minecraft:{s}")));
        let pref = |e: &&EntityDraw| match look_kind {
            Some(k) if k != EntityModelKind::Capsule => e.kind == k,
            _ => e.name.is_some(),
        };
        // REWO_LOOK_HIGH: among the preferred kind, take the highest one
        // (a floated summon sits above all ground clutter — deterministic).
        let high = std::env::var("REWO_LOOK_HIGH").is_ok();
        let nearest = if high {
            draws
                .iter()
                .filter(pref)
                .max_by(|a, b| a.pos[1].partial_cmp(&b.pos[1]).unwrap())
        } else {
            draws
                .iter()
                .filter(pref)
                .min_by(|a, b| d(a).partial_cmp(&d(b)).unwrap())
        }
        .or_else(|| draws.iter().min_by(|a, b| d(a).partial_cmp(&d(b)).unwrap()));
        if let Some(t) = nearest {
            let d = Vec3::new(
                t.pos[0] - eye.x,
                t.pos[1] + t.height * 0.5 - eye.y,
                t.pos[2] - eye.z,
            );
            let len = d.length().max(1e-4);
            yaw = (-d.x).atan2(d.z).to_degrees();
            pitch = (-d.y / len).asin().to_degrees();
            log::info!(
                "live: aiming at entity ({:.1},{:.1},{:.1})",
                t.pos[0],
                t.pos[1],
                t.pos[2]
            );
        }
    }
    // Block targeting: aim the raycast the same way the camera looks, so the
    // selection outline lands on whatever block the eye view frames.
    let hit = session.target_block(eye_f64(&session), look_dir(yaw, pitch), REACH);
    if let Some(h) = hit {
        log::info!("live: targeting block {:?} face {:?}", h.block, h.face);
    }
    world_renderer.set_selection(hit.map(|h| h.block));
    let (cr, cu) = camera_basis(yaw, pitch);
    // Before `set_entities`: the entity pass reads the eye as the CEM
    // `player_pos_*`, which FA aims mob eyes/heads with.
    world_renderer.set_camera(eye.to_array());
    // Day/night + effects: push the one resolved lightmap plus the sky/fog tint.
    apply_lightmap(
        &mut world_renderer,
        &lightmap,
        session.day_ticks,
        session.active_dimension_type.as_ref(),
        {
            let w = effective_weather(&session);
            (w.rain_level(), w.thunder_level())
        },
        rain_fog_band(&session, &mut weather_assets, None),
    );
    apply_biome_sky_fog(&mut world_renderer, &session);
    // M33: the sun and moon fade out as rain comes in
    // (`SkyRenderState.rainBrightness`).
    let mut cel = celestial_state_of(session.day_ticks);
    apply_weather_to_celestial(&mut cel, &session);
    world_renderer.set_celestial(cel);
    apply_weather(
        &mut world_renderer,
        &mut gpu,
        &session,
        &mut weather_assets,
        1.0,
        None,
    );
    apply_border(&mut world_renderer, &mut gpu, &session, 1.0);
    // M35: `REWO_OPEN_INVENTORY=1` opens the screen for the headless shot, so
    // a PNG can show it without a windowed session and a keypress. The cursor
    // is parked at the window centre, which is where `set_screen_open` puts it.
    let mut headless_screen_labels: Vec<rewo_gpu::world::OwnedTextLine> = Vec::new();
    let (sw, sh) = (off.extent.width as f32, off.extent.height as f32);
    let screen_open = std::env::var("REWO_OPEN_INVENTORY")
        .map(|v| v != "0")
        .unwrap_or(false);
    if screen_open {
        // `REWO_MOUSE=x,y` moves the cursor for the shot, which is the only way
        // to photograph the preview turning to follow it.
        let mouse = std::env::var("REWO_MOUSE")
            .ok()
            .and_then(|v| {
                let (a, b) = v.split_once(',')?;
                Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
            })
            .unwrap_or((sw as f64 / 2.0, sh as f64 / 2.0));
        // `REWO_PREVIEW_SKIN=<username|url>` fetches one profile's textures
        // for the shot. Offline test servers carry no textures property, so
        // this is the only way to photograph the preview wearing a real skin
        // — or, since M64, a real cape.
        let mut skin = std::env::var("REWO_PREVIEW_SKIN").ok().and_then(|spec| {
            let info = match crate::skin_fetch::resolve(&spec) {
                Ok(i) => i,
                Err(e) => {
                    log::warn!("preview: {spec}: {e}");
                    return None;
                }
            };
            // The two are independent: a profile may carry a cape and no
            // skin, exactly as `SkinLoader::request` treats them.
            let mut t = PreviewTextures::default();
            if let Some(url) = info.url.as_ref() {
                match crate::skin_fetch::fetch_rgba64(url) {
                    Ok(rgba) => {
                        log::info!(
                            "preview: skin {spec} ({} model)",
                            if info.slim { "slim" } else { "wide" }
                        );
                        t.skin = Some((rgba, info.slim));
                    }
                    Err(e) => log::warn!("preview: skin {spec}: {e}"),
                }
            }
            if let Some(url) = info.cape.as_ref() {
                match crate::skin_fetch::fetch_cape_rgba(url) {
                    Ok(rgba) => {
                        log::info!("preview: cape {spec}");
                        t.cape = Some(rgba);
                    }
                    Err(e) => log::warn!("preview: cape {spec}: {e}"),
                }
            }
            (t.skin.is_some() || t.cape.is_some()).then_some(t)
        });
        // The headless path takes no glyph cache: the gates' golden images
        // are graded against the bitmap tooltip, and swapping the typeface
        // under them would move every one of those pixels for a reason that
        // has nothing to do with what they test.
        let (labels, _velvet) = apply_screen(
            &mut world_renderer,
            &mut gpu,
            &session,
            &items,
            &mut gui_items,
            &baked,
            skin.as_mut(),
            None,
            // The headless screen renders the normal tooltip: the gates' golden
            // images grade what a default session shows, and F3+H is not it.
            rewo_gpu::tooltip::TooltipFlag::NORMAL,
            mouse,
            (sw, sh),
        );
        headless_screen_labels = labels;
    } else {
        apply_hotbar_icons(
            &mut world_renderer,
            &mut gpu,
            &session,
            &items,
            &mut gui_items,
            (sw, sh),
        );
    }
    if let Some(p) = particle_assets.as_mut() {
        let view = eye_view(eye, session.player.yaw, session.player.pitch).to_cols_array_2d();
        apply_particles(
            &mut world_renderer,
            &mut gpu,
            &session,
            particle_events,
            p,
            &baked,
            1.0,
            view,
        );
    }
    apply_crumbling(&mut world_renderer, &mut gpu, &session, &baked, eye);
    // M38: the first-person hand. `REWO_HAND_SWING=<0..1>` freezes the swing
    // partway for a shot — it is a tick clock, so a headless frame would
    // otherwise always catch it at rest.
    {
        let mut hand = HandState::new(&baked);
        // Settle the equip clock, or the item is caught mid-dip on tick one.
        hand.settle(&session, &items);
        hand.forced_attack = std::env::var("REWO_HAND_SWING")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .map(|v| v.clamp(0.0, 1.0));
        apply_hand(
            &mut world_renderer,
            &mut gpu,
            &session,
            &items,
            &mut hand,
            1.0,
            sw / sh,
        );
    }
    let bes = collect_block_entities(&session.world, &chest_states, &lightmap, 1.0, session.game_time(), (cr, cu));
    // A spawner's caged mob rides the ENTITY pass, mounted inside its block
    // (M31), so it joins the entity draws rather than the block-entity ones.
    let caged = collect_spawner_mobs(
        &session.world,
        &etypes,
        chest_states.spawner_states(),
        &lightmap,
        1.0,
    );
    let portals = collect_end_portals(
        &session.world,
        chest_states.end_portal_states(),
        chest_states.end_gateway_states(),
    );
    world_renderer.set_end_portals(&mut gpu, &portals, session.game_time())?;
    let mut draws = draws;
    draws.extend(caged.iter().map(spawner_mob_draw));
    // M81: the stacks in flight to whoever picked them up. Appended to the
    // same list so they go through `prepare_held_items` below — a pickup's
    // item needs an atlas slot exactly as a dropped one does, and the entity
    // it came from has already left the table.
    draws.extend(collect_pickups(
        &session,
        &items,
        &lightmap,
        1.0,
        start.elapsed().as_secs_f32(),
    ));
    // Every texture the frame samples from the entity atlas — items in hands,
    // dropped stacks, and now block-entity models, which share the pool.
    let mut held: Vec<&str> = draws.iter().flat_map(|d| d.held).flatten().collect();
    held.extend(draws.iter().filter_map(|d| d.ground_item));
    held.extend(bes.iter().map(|b| b.model.as_str()));
    world_renderer.prepare_held_items(&mut gpu, &held)?;
    let be_draws: Vec<_> = bes.iter().map(|b| b.as_draw()).collect();
    let sign_lines = match world_renderer.font_advance() {
        Some(a) => collect_sign_text(&session.world, &sign_states, &lightmap, a),
        None => Vec::new(),
    };
    let sign_draws: Vec<_> = sign_lines
        .iter()
        .map(|l| rewo_gpu::entities::WorldTextDraw {
            transform: l.transform,
            text: &l.text,
            x: l.x,
            y: l.y,
            z: l.z,
            color: l.color,
            light: l.light,
        })
        .collect();
    world_renderer.set_entities_and_block_entities(
        &draws,
        &be_draws,
        &sign_draws,
        cr,
        cu,
        start.elapsed().as_secs_f32(),
    );
    world_renderer.set_hud(
        session.health,
        session.food,
        0,
        resolve_hud_gauges(
            &session.hud,
            &session.inventory,
            &items,
            has_experience(&session),
            0.0,
        ),
    );
    {
        let scale = rewo_gpu::hud::gui_scale(1280.0, 720.0);
        world_renderer.set_locator_bar(resolve_locator_bar(
            &session,
            &session.world.entities,
            &locator_styles,
            crate::modules::VANILLA_FOV,
            (1280.0 / scale) as i32,
            (720.0 / scale) as i32,
            0.0,
        ));
    }
    let mut headless_text = build_text(&session, gui_px(1280, 720), 720.0, None, true);
    headless_text.extend(headless_screen_labels);
    world_renderer.set_text(headless_text);
    world_renderer.anim_tick(&mut gpu, session.ticks)?;
    let vp = eye_view_proj(eye, yaw, pitch, 1280.0 / 720.0, crate::modules::VANILLA_FOV);
    let ring = OverlayRing::default();
    let draw = OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [16.0, 16.0],
        size: [560.0, 140.0],
    };
    for _ in 0..3 {
        off.render(&gpu, Some((&mut world_renderer, vp)), &draw, CLEAR_SKY)?;
    }
    off.save_png(&gpu, out)?;
    // M16 diagnostic: which dimension the frame was actually resolved from, and
    // the three values that drive it. Printed once, not per frame — enough to
    // tell a Nether frame from an Overworld one in a headless log, without
    // asking anyone to look at the PNG.
    println!(
        "[rewo-m16] dimension: {} skybox {:?} (end_sky asset {}) ambient {:?} sky_light {:?} x{:.3}",
        session
            .active_dimension_type
            .as_ref()
            .map(|d| d.name.as_str())
            .unwrap_or("<unresolved>"),
        world_renderer.sky_mode(),
        if world_renderer.end_sky_ready() { "present" } else { "MISSING" },
        lightmap.ambient_color,
        lightmap.sky_light_color,
        lightmap.sky_factor,
    );
    let total = world_renderer.column_count();
    let gpu_drawn = world_renderer.read_draw_count(&mut gpu);
    println!(
        "[rewo-m3-live] headless: spawned at ({:.1},{:.1},{:.1}), {} columns loaded, GPU cull drew {} of {} ({} culled on GPU)",
        session.player.x,
        session.player.y,
        session.player.z,
        session.world.loaded_columns(),
        gpu_drawn,
        total,
        total as i64 - gpu_drawn as i64,
    );
    println!("[rewo-m3-live] wrote {}", out.display());
    world_renderer.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}

// -- windowed ---------------------------------------------------------------

#[derive(Default)]
struct Keys {
    w: bool,
    a: bool,
    s: bool,
    d: bool,
    jump: bool,
    sneak: bool,
    sprint: bool,
}

impl Keys {
    fn input(&self) -> TickInput {
        TickInput {
            forward: (self.w as i32 - self.s as i32) as f32,
            strafe: (self.a as i32 - self.d as i32) as f32,
            jump: self.jump,
            sneak: self.sneak,
            sprint: self.sprint && self.w,
        }
    }
}

struct LiveState {
    window: Arc<Window>,
    gpu: Gpu,
    renderer: Renderer,
    world_renderer: WorldRenderer,
}

struct LiveApp {
    session: Option<PlaySession>,
    baked: Option<assets::BakedAssets>,
    etypes: EntityTypes,
    /// `minecraft:spears` membership — decides the `SPEAR` arm pose for a
    /// spear that is merely *held* (the swinging-STAB case needs no tag).
    spears: rewo_data::item_tags::ItemTag,
    /// Chest block states → facing + material, for the M25b block-entity draws.
    chest_states: rewo_data::chest_states::ChestStates,
    sign_states: rewo_data::sign_states::SignStates,
    /// `Items.BOW` protocol id — a bow suppresses the skeleton attack rig.
    bow_item: Option<i32>,
    /// Item registry, for id → name when resolving held models (M22).
    items: std::sync::Arc<rewo_data::items::Items>,
    /// Armour layer definitions (M46). Cloned out of the bake because
    /// `self.baked` is *taken* when the window opens, and the entity draws
    /// need this every frame after that.
    equipment: std::sync::Arc<rewo_data::equipment::EquipmentAssets>,
    /// Trim sources + palettes (M48), cloned for the same reason.
    trims: std::sync::Arc<rewo_data::equipment::TrimAssets>,
    /// The language map (M82), cloned for the same reason as the two above.
    ///
    /// **`self.baked` is `None` for the whole windowed session** — the init
    /// closure `take()`s it and drops it, which has been true since M3
    /// (`47da8a0`) and is why `equipment` and `trims` are cloned here at all.
    /// The death screen's four labels are resolved from this instead, so the
    /// screen does not join the list of things that silently never happen in
    /// the windowed client. See the M82 entry in `REWO_PLAN.md` §15 for the
    /// full finding.
    lang: std::sync::Arc<rewo_data::lang::Language>,
    pool: MeshPool,
    /// M33: the cloud map, the climate noises and the cached cloud mesh. Built
    /// on the first frame that has a bake, since `baked` arrives with the
    /// session rather than at construction.
    weather: Option<WeatherAssets>,
    /// M34: the hotbar icons' atlas residency + the baked items. Built on the
    /// first frame that has a bake, like `weather`.
    gui_items: Option<GuiItemState>,
    /// The inventory screen (M35).
    screen: ScreenState,
    /// The first-person hand (M38).
    hand: Option<HandState>,
    /// A quick-craft drag in progress (M40).
    drag: DragState,
    /// Whether left control is held — Ctrl+Q drops a whole stack (M40).
    ctrl: bool,
    /// The slot the last left click landed on and when, for the double click
    /// that becomes `PICKUP_ALL` (M40).
    last_click: Option<usize>,
    last_click_at: std::time::Instant,
    /// Whether either shift is held — a shift-click in the inventory is a
    /// quick-move rather than a pickup.
    shift: bool,
    /// The local player's textures as they sit in the **preview** pass's
    /// atlas (M36 skin, M64 cape). Held separately from `skins` because the
    /// two passes have separate atlases and an address from one is
    /// meaningless in the other.
    preview_skin: Option<PreviewTextures>,
    /// This frame's stack-count labels. Built with the icons, consumed by the
    /// text pass a few lines later — the two are separated only because the
    /// icons need `&mut gpu` and the text does not.
    screen_labels: Vec<rewo_gpu::world::OwnedTextLine>,
    /// M37 particles — `None` until the bake arrives, and stays `None` if the
    /// jar has no particle sprites.
    particles: Option<ParticleAssets>,
    keys: Keys,
    want_validation: bool,
    run_seconds: Option<f32>,
    /// M86's live reachability + validation gate. `None` unless
    /// `--render-check`.
    check: Option<RenderCheck>,
    fif: usize,
    state: Option<LiveState>,
    ring: OverlayRing,
    cpu: StatsAccum,
    started: Instant,
    last_frame: Option<Instant>,
    tick_accum: f32,
    logged_spawn: bool,
    uploaded_total: usize,
    flood_logged: bool,
    /// Selected hotbar slot 0..8 (number keys), for the HUD selection frame.
    hotbar_slot: u8,
    /// Dirt item id (for right-click place), resolved from the item table.
    dirt_item: Option<i32>,
    /// F3 debug overlay visible. Default on.
    ///
    /// Toggled on **release**, not press: `keyDebugModifier` and
    /// `keyDebugOverlay` are the same key, so vanilla's `KeyboardHandler` waits
    /// to see whether F3 was used as a chord modifier before treating it as a
    /// toggle (`if (this.usedDebugKeyAsModifier) { clear it } else { toggle }`).
    debug: bool,
    /// `Hud.isHidden()` — F1 (M70). Vanilla's is a plain `toggle()` on press,
    /// with none of F3's modifier dance, and it starts `false`.
    ///
    /// Rewo consumes it only where vanilla's *label* path does: it suppresses
    /// floating nametags and health bars on un-teamed entities. Vanilla also
    /// hides the whole GUI layer from it (`guiRenderState.isHudHidden`);
    /// hiding Rewo's hotbar/hearts/F3 block is a separate concern this
    /// milestone deliberately leaves alone, and is recorded as open.
    hud_hidden: bool,
    /// F3 is held — the `keyDebugModifier` half (M66).
    f3_down: bool,
    /// `usedDebugKeyAsModifier` — a chord fired while F3 was down, so its
    /// release must not also toggle the overlay.
    f3_used_as_modifier: bool,
    /// `Options.advancedItemTooltips` — F3+H (M66). Vanilla persists it to
    /// `options.txt`; Rewo has no options file, so it resets each session.
    advanced_tooltips: bool,
    /// M88 — whether `--render-check` has injected its container open yet.
    /// Latched so the inject happens once rather than every frame past the
    /// threshold, which would re-open the menu and reset its state each frame.
    container_injected: bool,
    /// Whether `--render-check` has force-opened the inventory yet (M89).
    /// Frames before this are the ones that prove `open_screen` opens the
    /// screen on its own.
    screen_forced_open: bool,
    /// `Hud.lastToolHighlight` + `toolHighlightTimer` (M66) — the held-item
    /// name that fades in over the hotbar.
    tool_highlight: rewo_gpu::hud::ToolHighlight,
    /// M83's `waypoint_style` table, resolved once at init. Held rather than
    /// rebuilt per frame because `markers` needs it every frame and its
    /// sprite lists are `Vec`s.
    locator_styles: Vec<rewo_gpu::locator_bar::WaypointStyle>,
    /// M52 module port: the legit module set, loaded from the active client
    /// profile's `modules.toml` -- the same file the launcher's Settings →
    /// Modules tab writes, so a Native instance needs no new config contract.
    modules: crate::modules::Modules,
    /// M52b Velvet type stack: the glyph cache behind tooltip text. `None`
    /// when `assets/fonts` is missing -- the tooltip then falls back to the
    /// vanilla bitmap pass rather than drawing nothing, because a client that
    /// loses its tooltips over a missing font file is worse than one that
    /// draws them plainly.
    glyphs: Option<rewo_gpu::velvet_glyph::GlyphCache>,
    /// F2 was pressed and a capture is owed (M51). Serviced after the frame
    /// rather than inside the key handler, because a capture needs the same
    /// `gpu`/`world_renderer` the render loop owns.
    capture_pending: bool,
    /// Per-entity gesture state-change clocks (pose-driven rigs).
    gestures: GestureTracker,
    /// Block-light flicker (M13): ticked once per successful 20 Hz tick,
    /// mirroring vanilla's `LightmapRenderStateExtractor`.
    flicker: BlockLightFlicker,
    /// `Options.gamma` — the camera-lightmap notGamma lift weight (M13).
    gamma: f32,
    /// `Options.darknessEffectScale` — the Darkness-effect pulse scale (M13).
    darkness_option: f32,
    /// Async player-skin fetch + upload (online-mode real skins).
    skins: SkinLoader,
    /// OptiFine CEM resource pack (M9) — mob-model overrides, applied at
    /// entity-pass init.
    pack: Option<PathBuf>,
    /// The same pack's ETF random-entity rules (M52), consulted per entity per
    /// frame. Empty without a pack.
    etf: rewo_data::etf::EtfPack,
    /// `minecraft:stat_type` / `custom_stat` / `block` (M84). Cloned out of the
    /// report because `self.baked` is a `LiveState` concern.
    stat_registries: std::sync::Arc<rewo_data::stats::StatRegistries>,
    /// The statistics screen's own state (M84) — `None` when it is shut.
    ///
    /// Opened by F6, because vanilla's only route to it is the pause menu's
    /// `Statistics` button and M85's pause screen does not carry one. Recorded
    /// as a Rewo-specific opener rather than smuggled in as if it were
    /// vanilla's.
    stats: Option<crate::stats_view::StatsView>,
    /// The death screen's own state (M82) — `None` while alive.
    death: Option<DeathView>,
    /// Whichever of M85's three screens is up, and the state it needs to be
    /// rebuilt on a resize.
    view: ScreenView,
    /// **The durable copy of the server's links (M85).**
    ///
    /// `SessionState` owns them while the session lives, exactly as
    /// `ClientCommonPacketListenerImpl.serverLinks` does — but the disconnect
    /// screen exists *after* the session is dropped, and it is the one screen
    /// that needs them. So they are mirrored here every frame the session is
    /// alive. Reading them off the session at disconnect time would be
    /// `REWO_PLAN.md` §0.0 gotcha 13 in its other shape: state consulted after
    /// the event that destroys it, with every gate that builds the state
    /// blind to the difference.
    server_links: rewo_net::server_links::ServerLinks,
    /// A screen asked to leave the server (M82). Serviced in the frame loop,
    /// because a widget press has no `ActiveEventLoop` to exit with.
    exit_requested: bool,
    init_error: Option<String>,
}

impl ApplicationHandler for LiveApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Rewo · live")
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.init_error = Some(format!("create window: {e}"));
                event_loop.exit();
                return;
            }
        };
        // Returns the bake **alongside** the state, and the `Ok` arm below puts
        // it back in `self.baked`.
        //
        // This closure used to return `LiveState` alone, which meant the
        // `self.baked.take()` below dropped the bake at the closing brace and
        // left `self.baked` as `None` for the entire windowed session. Every
        // `if let Some(baked) = self.baked.as_ref()` in `frame` was therefore
        // dead code in `rewo live` — the item icons, the inventory screen, the
        // first-person hand, the cloud deck, the precipitation, the rain-fog
        // band, the particles, the world border and the block-breaking decals,
        // none of which had ever rendered in the windowed client since M3. See
        // the M86 entry in `REWO_PLAN.md` §15.
        let init = (|| -> Result<(LiveState, assets::BakedAssets), String> {
            let rdh = window
                .display_handle()
                .map_err(|e| format!("dh: {e}"))?
                .as_raw();
            let rwh = window
                .window_handle()
                .map_err(|e| format!("wh: {e}"))?
                .as_raw();
            let mut gpu = Gpu::new(Some((rdh, rwh)), self.want_validation)?;
            let size = window.inner_size();
            let mut renderer = Renderer::with_frames_in_flight(
                &mut gpu,
                size.width.max(1),
                size.height.max(1),
                vk::PresentModeKHR::MAILBOX,
                self.fif,
            )?;
            renderer.ensure_depth(&mut gpu)?;
            let baked = self.baked.take().ok_or("assets consumed")?;
            let mut world_renderer = WorldRenderer::new(
                &mut gpu,
                renderer.swapchain.format,
                assets::TEX_SIZE,
                &baked.layers,
            )?;
            self.etf = init_entities_maybe_cem(&mut world_renderer, &mut gpu, &baked, &self.pack)?;
            world_renderer.set_held_items(to_gpu_held_items(&baked.held_items));
            init_celestial_if_present(&mut world_renderer, &mut gpu, &baked)?;
            init_weather_if_present(&mut world_renderer, &mut gpu, &baked)?;
            init_particles_if_present(&mut world_renderer, &mut gpu, &baked)?;
    init_crumbling_if_present(&mut world_renderer, &mut gpu, &baked)?;
            world_renderer.set_animations(layer_animations(&baked));
            if let Some(l) = locator_sprites(&baked) {
                self.locator_styles = l.styles.clone();
                world_renderer.init_locator_bar(&mut gpu, &l)?;
            }
            if let Some(w) = widget_sprites(&baked) {
                world_renderer.init_screen(&mut gpu, &w)?;
            }
            if let Some(hud) = hud_sprites(&baked) {
                world_renderer.init_hud(&mut gpu, &hud)?;
                // M52b: the Velvet type stack, windowed only. A build with no
                // fonts on disk gets `None` and the tooltip falls back to the
                // bitmap pass -- losing tooltips over a missing font file
                // would be worse than drawing them plainly.
                if let Some(cache) = self.glyphs.as_ref() {
                    if let Err(e) = world_renderer.init_velvet_text(&mut gpu, cache) {
                        log::warn!("velvet text unavailable: {e}");
                        self.glyphs = None;
                    }
                }
            }
            Ok((
                LiveState {
                    window: window.clone(),
                    gpu,
                    renderer,
                    world_renderer,
                },
                baked,
            ))
        })();
        match init {
            Ok((state, baked)) => {
                let _ = state.window.set_cursor_grab(CursorGrabMode::Confined);
                state.window.set_cursor_visible(false);
                self.started = Instant::now();
                self.state = Some(state);
                // The half M3 forgot. Without it every baked-gated branch in
                // `frame` is unreachable — see the closure's doc above.
                self.baked = Some(baked);
            }
            Err(e) => {
                self.init_error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(LiveState { gpu, renderer, .. }) = self.state.as_mut() {
                    if size.width > 0 && size.height > 0 {
                        let _ = renderer.resize(gpu, size.width, size.height);
                        let _ = renderer.ensure_depth(gpu);
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let p = event.state == ElementState::Pressed;
                // While the screen is open the player stands still: only the
                // two keys that close it are read, and every movement key is
                // released so a key held at the moment of opening does not
                // stick down behind it.
                // With the screen open the keys that reach the world are
                // swallowed, but the screen has keys of its own: a number key
                // or F swaps the hovered slot with a hotbar slot, Q drops from
                // it. Ctrl is tracked either way, because Ctrl+Q drops the
                // whole stack.
                if matches!(event.physical_key, PhysicalKey::Code(KeyCode::ControlLeft)) {
                    self.ctrl = p;
                }
                if self.screen.inventory_open() {
                    if p {
                        if let Some(action) = screen_key_action(event.physical_key, self.ctrl) {
                            let ext = self.state.as_ref().map(|s| s.window.inner_size());
                            let items = self.items.clone();
                            if let (Some(session), Some(ext)) = (self.session.as_mut(), ext) {
                                click_screen(
                                    session,
                                    &items,
                                    &self.screen,
                                    action,
                                    ext.width as f32,
                                    ext.height as f32,
                                );
                            }
                            return;
                        }
                    }
                    if !matches!(
                        event.physical_key,
                        PhysicalKey::Code(KeyCode::Escape)
                            | PhysicalKey::Code(KeyCode::KeyE)
                            | PhysicalKey::Code(KeyCode::ShiftLeft)
                            | PhysicalKey::Code(KeyCode::ShiftRight)
                    ) {
                        return;
                    }
                }
                // M82: a non-inventory screen owns the keyboard.
                //
                // `Screen.keyPressed`'s order is Esc → the focused widget →
                // Tab; the inventory is exempt above only because its own
                // `keyPressed` override runs first and it has no widgets to
                // focus. The three debug keys still get through, which is
                // vanilla's arrangement too — `KeyboardHandler.keyPress`
                // handles the screenshot and debug keys before it hands the
                // event to `minecraft.gui.screen()`. **Esc is not on that
                // list**, so a death screen (`shouldCloseOnEsc() == false`)
                // swallows it and `rewo live` does not quit.
                if self.screen.any_open() && !self.screen.inventory_open() {
                    if p {
                        let shift = self.shift;
                        let result = glfw_key(event.physical_key).and_then(|k| {
                            self.screen
                                .screens
                                .current_mut()
                                .map(|s| (s.kind, s.key_pressed(k, shift)))
                        });
                        match result {
                            Some((kind, rewo_world::screen::KeyResult::Pressed(id))) => {
                                self.press_widget(kind, id);
                                return;
                            }
                            Some((kind, rewo_world::screen::KeyResult::Close)) => {
                                // `onClose()`. For a dialog that is
                                // `DialogAction.CLOSE` → the previous screen,
                                // which is the pause screen it was opened
                                // from; for everything else it is
                                // `setScreen(null)`.
                                if kind == rewo_world::screen::ScreenKind::ServerLinks {
                                    self.open_pause_screen();
                                } else if kind == rewo_world::screen::ScreenKind::Stats {
                                    // M84: a screen with per-screen state must
                                    // drop it here too, or `pump_stats_screen`
                                    // re-opens the screen Esc just closed.
                                    self.close_stats();
                                } else {
                                    self.close_view_screen();
                                }
                                return;
                            }
                            Some((_, rewo_world::screen::KeyResult::Handled)) => return,
                            _ => {}
                        }
                    }
                    if !matches!(
                        event.physical_key,
                        PhysicalKey::Code(KeyCode::F1)
                            | PhysicalKey::Code(KeyCode::F2)
                            | PhysicalKey::Code(KeyCode::F3)
                    ) {
                        return;
                    }
                }
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::KeyW) => self.keys.w = p,
                    PhysicalKey::Code(KeyCode::KeyA) => self.keys.a = p,
                    PhysicalKey::Code(KeyCode::KeyS) => self.keys.s = p,
                    PhysicalKey::Code(KeyCode::KeyD) => self.keys.d = p,
                    PhysicalKey::Code(KeyCode::Space) => self.keys.jump = p,
                    // M52 Toggle Sneak: with the module on, Shift *flips*
                    // sneak instead of holding it. `!event.repeat` is load
                    // bearing -- the OS auto-repeats a held key, and without
                    // the guard holding Shift would flip sneak dozens of times
                    // a second rather than once.
                    PhysicalKey::Code(KeyCode::ShiftLeft) => {
                        if self.modules.is_on("toggle_sneak") {
                            if p && !event.repeat {
                                self.keys.sneak = !self.keys.sneak;
                            }
                        } else {
                            self.keys.sneak = p;
                        }
                        self.shift = p;
                    }
                    PhysicalKey::Code(KeyCode::ShiftRight) => self.shift = p,
                    // M52 Toggle Sprint, same shape as Toggle Sneak.
                    PhysicalKey::Code(KeyCode::ControlLeft) => {
                        if self.modules.is_on("toggle_sprint") {
                            if p && !event.repeat {
                                self.keys.sprint = !self.keys.sprint;
                            }
                        } else {
                            self.keys.sprint = p;
                        }
                    }
                    // M52 Zoom: a HELD key, not a toggle -- every one of the
                    // 54 zoom mods in the survey binds it that way. C is
                    // Zoomify's default. The divide happens in
                    // `Modules::render` so it composes with FOV Control.
                    PhysicalKey::Code(KeyCode::KeyC) => self.modules.set_zoom_held(p),
                    // Esc closes the inventory if it is open, opens the
                    // **pause screen** if a session is running, and only quits
                    // otherwise (M85 — before it, Esc quit outright).
                    //
                    // A non-inventory screen never reaches this arm: the block
                    // above hands Esc to `Screen.keyPressed`, whose
                    // `shouldCloseOnEsc()` decides. That is why the pause
                    // screen closes on Esc and the death and disconnect
                    // screens do not.
                    PhysicalKey::Code(KeyCode::Escape) if p => {
                        if self.screen.inventory_open() {
                            self.set_screen_open(false);
                        } else if self.session.is_some() {
                            self.open_pause_screen();
                        } else {
                            event_loop.exit();
                        }
                    }
                    PhysicalKey::Code(KeyCode::Escape) => {}
                    // F6 opens and closes the statistics screen (M84).
                    //
                    // **A Rewo-specific binding.** Vanilla reaches this screen
                    // from the pause menu's `Statistics` button; M85's pause
                    // screen transcribes `PauseScreen`'s own grid, which does
                    // not carry one (`StatsScreen` is reached from the
                    // *singleplayer* pause menu's second row, which M85's
                    // multiplayer transcription omits). A key is the interim
                    // route rather than a claim about vanilla's input.
                    PhysicalKey::Code(KeyCode::F6) if p && !event.repeat => {
                        if self.stats.is_some() {
                            self.close_stats();
                        } else {
                            self.open_stats();
                        }
                    }
                    // E opens and closes the inventory (M35).
                    PhysicalKey::Code(KeyCode::KeyE) if p => {
                        let open = !self.screen.inventory_open();
                        self.set_screen_open(open);
                    }
                    // F3 is a **modifier** whose release toggles the overlay
                    // (M66). `keyDebugModifier` and `keyDebugOverlay` are the
                    // same key, so `KeyboardHandler` defers the toggle to
                    // `action == 0` and skips it when a chord already fired:
                    //
                    //   if (usedDebugKeyAsModifier) usedDebugKeyAsModifier = false;
                    //   else                        toggleDebugOverlay();
                    //
                    // Toggling on press instead would flip the overlay every
                    // time you pressed F3+H.
                    // F1 — `Hud.toggle()` (M70). On **press**, and with no
                    // modifier dance: F1 is not also a chord prefix, so unlike
                    // F3 there is nothing to disambiguate. `!event.repeat`
                    // guards the OS auto-repeat, which would otherwise flip it
                    // dozens of times a second while held.
                    PhysicalKey::Code(KeyCode::F1) if p && !event.repeat => {
                        self.hud_hidden = !self.hud_hidden;
                        log::info!(
                            "hud.{}",
                            if self.hud_hidden { "hidden" } else { "shown" }
                        );
                    }
                    PhysicalKey::Code(KeyCode::F3) => {
                        self.f3_down = p;
                        if !p {
                            if self.f3_used_as_modifier {
                                self.f3_used_as_modifier = false;
                            } else {
                                self.debug = !self.debug;
                            }
                        }
                    }
                    // F3+H — `keyDebugShowAdvancedTooltips`.
                    PhysicalKey::Code(KeyCode::KeyH) if p && self.f3_down => {
                        self.advanced_tooltips = !self.advanced_tooltips;
                        self.f3_used_as_modifier = true;
                        log::info!(
                            "debug.advanced_tooltips.{}",
                            if self.advanced_tooltips { "on" } else { "off" }
                        );
                    }
                    // F2 captures a screenshot — vanilla's `keyScreenshot`,
                    // GLFW key 291. Only the request is recorded here; the
                    // capture itself happens after the frame.
                    PhysicalKey::Code(KeyCode::F2) if p => self.capture_pending = true,
                    // Number keys 1..9 select the hotbar slot (HUD frame +
                    // sent to the server so the held item matches).
                    PhysicalKey::Code(code) if p => {
                        if let Some(n) = digit_key(code) {
                            self.hotbar_slot = n;
                            if let Some(s) = self.session.as_mut() {
                                let _ = s.select_hotbar(n);
                            }
                        }
                    }
                    _ => {}
                }
            }
            // M82: a click on a widget-bearing screen. Before the inventory's
            // arm and before the world's, because
            // `ContainerEventHandler.mouseClicked` returns **true whenever
            // `getChildAt` found something** — the child's own answer only
            // decides whether it was *pressed*. So a right-click on a button
            // is eaten by the screen and never digs.
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } if self.screen.any_open() && !self.screen.inventory_open() => {
                let (mx, my) = self.mouse_gui();
                let b = match button {
                    MouseButton::Left => 0u8,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return,
                };
                let pressed = self
                    .screen
                    .screens
                    .current_mut()
                    .map(|s| (s.kind, s.mouse_clicked(mx, my, b)));
                if let Some((kind, rewo_world::screen::MouseResult::Pressed(id))) = pressed {
                    self.press_widget(kind, id);
                }
            }
            WindowEvent::MouseInput {
                state: btn, button, ..
            } if btn == ElementState::Pressed && self.screen.inventory_open() => {
                // A click on the screen moves items; it never digs or places.
                let ext = self.state.as_ref().map(|s| s.window.inner_size());
                let items = self.items.clone();
                if let (Some(session), Some(ext)) = (self.session.as_mut(), ext) {
                    let b = match button {
                        MouseButton::Left => 0,
                        MouseButton::Right => 1,
                        _ => return,
                    };
                    // `AbstractContainerScreen.mouseClicked`'s double click:
                    // the **same slot**, the **left** button, and under 250 ms
                    // since the last one. Not "two clicks anywhere in
                    // 250 ms" — moving to a neighbouring slot resets it.
                    let layout = session.shown_menu().layout();
                    let slot =
                        self.screen
                            .hovered(layout, ext.width as f32, ext.height as f32);
                    let now = std::time::Instant::now();
                    let doubled = b == 0
                        && slot.is_some()
                        && self.last_click == slot
                        && now.duration_since(self.last_click_at).as_millis() < 250;
                    self.last_click = slot;
                    self.last_click_at = now;
                    // With a stack already on the cursor a press starts a
                    // drag rather than a click. The two are told apart at
                    // *release*: a drag that never left its slot collapses
                    // back into the click it looks like, which is exactly what
                    // vanilla's one-slot special case does.
                    if session.inventory.carried().is_some() && !self.shift && !doubled {
                        self.drag.begin(b);
                        if let Some(slot) = slot {
                            self.drag.add(slot);
                        }
                        return;
                    }
                    let action = if doubled {
                        SlotAction::PickupAll
                    } else if self.shift {
                        SlotAction::QuickMove
                    } else {
                        SlotAction::Pickup(b)
                    };
                    click_screen(
                        session,
                        &items,
                        &self.screen,
                        action,
                        ext.width as f32,
                        ext.height as f32,
                    );
                }
            }
            // Releasing a button over the open screen ends any drag (M40).
            WindowEvent::MouseInput {
                state: ElementState::Released,
                ..
            } if self.screen.inventory_open() => {
                let items = self.items.clone();
                if let Some(session) = self.session.as_mut() {
                    finish_drag(session, &items, &mut self.drag);
                }
            }
            // Releasing the right button ends a use — eating stops, a bow
            // fires, a shield drops. Vanilla sends `RELEASE_USE_ITEM`, and the
            // pose ends locally at the same moment.
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Right,
                ..
            } => {
                if let Some(session) = self.session.as_mut() {
                    let _ = session.stop_use();
                }
            }
            WindowEvent::MouseInput {
                state: btn, button, ..
            } if btn == ElementState::Pressed => {
                // Left-click digs the targeted block; right-click places
                // against its hit face, or — with nothing to place against —
                // starts *using* the held item (M38).
                if let Some(session) = self.session.as_mut() {
                    // The pick ray starts from the f64 eye (`eye_f64`), not the
                    // f32 render eye — at large coordinates the two disagree by
                    // more than a block.
                    let hit = session.target_block(
                        eye_f64(session),
                        look_dir(session.player.yaw, session.player.pitch),
                        REACH,
                    );
                    // Right-clicking thin air uses the item. Vanilla also uses
                    // it when the *block* interaction is declined, which needs
                    // the server's answer; this is the half that needs no round
                    // trip, and it covers eating, drawing and blocking.
                    if hit.is_none() && button == MouseButton::Right {
                        let _ = session.start_use(rewo_world::entities::InteractionHand::MainHand);
                    }
                    if let Some(h) = hit {
                        let [x, y, z] = h.block;
                        let face = face_index(h.face);
                        match button {
                            MouseButton::Left => {
                                let _ = session.start_dig(x, y, z, face);
                            }
                            MouseButton::Right => {
                                let _ = session.use_item_on(x, y, z, face);
                            }
                            _ => {}
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.screen.mouse = (position.x, position.y);
                // Crossing a slot with the button down extends a drag. The
                // slot only *joins* it if the server would accept it, and that
                // is decided at release — this records the path.
                if self.screen.inventory_open() {
                    if let Some(ext) = self.state.as_ref().map(|s| s.window.inner_size()) {
                        let layout = self.shown_layout();
                        if let Some(slot) =
                            self.screen
                                .hovered(layout, ext.width as f32, ext.height as f32)
                        {
                            self.drag.add(slot);
                        }
                    }
                }
            }
            // M84: the scroll wheel drives whichever list is up.
            //
            // `AbstractScrollArea.mouseScrolled` is
            // `setScrollAmount(scrollAmount() - scrollY * scrollRate())`, and
            // the **minus** is the whole of it: a positive `scrollY` (wheel
            // away from you) moves the list toward row 0. winit reports a notch
            // as `LineDelta(_, 1.0)`, which is GLFW's own unit, so the two need
            // no conversion; a trackpad's `PixelDelta` is divided by a line's
            // height first.
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y as f64,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y / 16.0,
                };
                if let Some(view) = self.stats.as_mut() {
                    view.model.list_mut().mouse_scrolled(dy);
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if self.screen.any_open() {
            // The cursor is free and driving the screen; the camera holds
            // still. `CursorMoved` supplies the position, so this raw delta is
            // simply dropped rather than accumulated.
            return;
        }
        if let (DeviceEvent::MouseMotion { delta }, Some(session)) = (&event, self.session.as_mut())
        {
            // Mouse drives the player's look (what we render AND send).
            session.player.yaw += delta.0 as f32 * 0.15;
            session.player.pitch =
                (session.player.pitch - delta.1 as f32 * 0.15).clamp(-89.0, 89.0);
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

impl LiveApp {
    /// Open or close the inventory screen (M35).
    ///
    /// Opening frees the cursor and parks it in the middle of the window;
    /// closing grabs it again and hides it. The park matters — winit reports
    /// no position until the mouse moves, so without it the first frame would
    /// hover whatever slot happens to sit at the stale coordinate.
    /// The layout on screen — the open container's, or the player's.
    ///
    /// Falls back to `PLAYER` with no session, which is what the screen shows
    /// before a connection anyway.
    fn shown_layout(&self) -> &'static rewo_world::menu_layout::MenuLayout {
        self.session
            .as_ref()
            .map(|s| s.shown_menu().layout())
            .unwrap_or(&rewo_world::menu_layout::PLAYER)
    }

    fn set_screen_open(&mut self, open: bool) {
        if open {
            let (gw, gh) = self.gui_size();
            self.screen
                .screens
                .open(rewo_world::screen::Screen::new(
                    rewo_world::screen::ScreenKind::Inventory,
                    gw,
                    gh,
                ));
        } else {
            self.screen.screens.close();
        }
        self.grab_for_screen(open);
    }

    /// The cursor in **GUI** pixels — the space every widget's rect is in.
    ///
    /// **M82 passed `self.screen.mouse` straight through, and it is in screen
    /// pixels.** `deathshot` divides by the GUI scale before calling the same
    /// builders, so its hover and click witnesses passed while the live path
    /// tested a point up to four times too far right and down: at any scale
    /// above 1 the death screen's buttons could not be hovered, and could only
    /// be *clicked* where the mis-scaled point happened to land on one. M85
    /// then built three more screens against the same call, so by the time M84
    /// found it — through its own hover witness measuring a zero-pixel
    /// difference — it was in five places. Every screen goes through here now.
    ///
    /// The inventory does **not**: `rewo_gpu::container::screen_to_gui` is
    /// panel-relative, which is what `slot_at` wants, and it takes screen
    /// pixels by design.
    fn mouse_gui(&self) -> (f64, f64) {
        let Some(ext) = self.state.as_ref().map(|s| s.window.inner_size()) else {
            return (0.0, 0.0);
        };
        let scale = rewo_gpu::hud::gui_scale(ext.width as f32, ext.height as f32) as f64;
        (self.screen.mouse.0 / scale, self.screen.mouse.1 / scale)
    }

    /// The window in GUI pixels — the space every screen lays its widgets out
    /// in. `(0, 0)` before the window exists, which no screen is ever built
    /// against.
    fn gui_size(&self) -> (i32, i32) {
        let Some(state) = self.state.as_ref() else {
            return (0, 0);
        };
        let size = state.window.inner_size();
        let px = gui_px(size.width, size.height);
        ((size.width as f32 / px) as i32, (size.height as f32 / px) as i32)
    }

    /// `Gui.setScreen`'s cursor half — `mouseHandler.releaseMouse()` +
    /// `KeyMapping.releaseAll()` on the way in, `grabMouse()` on the way out.
    ///
    /// Split out of [`Self::set_screen_open`] so a screen that is *not* the
    /// inventory gets exactly the same treatment: the death screen frees the
    /// cursor for the same reason and by the same code.
    fn grab_for_screen(&mut self, open: bool) {
        // Every movement key is released, so one held while opening does not
        // stay pressed behind the screen.
        self.keys = Keys::default();
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if open {
            let size = state.window.inner_size();
            self.screen.mouse = (size.width as f64 / 2.0, size.height as f64 / 2.0);
            let _ = state.window.set_cursor_grab(CursorGrabMode::None);
            state.window.set_cursor_visible(true);
        } else {
            let _ = state.window.set_cursor_grab(CursorGrabMode::Confined);
            state.window.set_cursor_visible(false);
        }
    }

    /// The vanilla bitmap font's advance table, when the renderer has one.
    ///
    /// M85's three screens are *laid out* from text widths (a title's
    /// `StringWidget` width, a disconnect reason's wrap), so the builder needs
    /// this before anything is drawn. Windowed, it lives on the entity pass —
    /// `self.baked` was `take()`n in `resumed` and has been `None` ever since
    /// (see the `lang` field's docs).
    fn advance(&self) -> Option<[u8; 256]> {
        self.state
            .as_ref()
            .and_then(|s| s.world_renderer.font_advance())
            .copied()
    }

    fn text_width(&self, text: &str) -> i32 {
        self.advance()
            .map(|a| rewo_gpu::text::width(text, &a))
            .unwrap_or(0)
    }

    /// `PauseScreen(true)` — Esc with a session behind it (M85).
    fn open_pause_screen(&mut self) {
        let labels = rewo_world::pause_screen::PauseLabels::resolve(&self.lang);
        // `!connection.serverLinks().isEmpty()` — the packet's whole effect on
        // this screen. Read from the durable mirror, which is the same value
        // the session holds while it is alive.
        let has_links = !self.server_links.is_empty();
        self.view = ScreenView::Pause(labels, has_links);
        self.rebuild_view_screen();
        self.grab_for_screen(true);
        log::info!("live: pause screen (server links: {has_links})");
    }

    /// The button on the pause screen — `showDialog(Dialogs.SERVER_LINKS)`.
    fn open_links_screen(&mut self) {
        let labels = rewo_world::server_links_screen::ServerLinksLabels {
            title: self
                .lang
                .or_key(rewo_world::server_links_screen::KEY_TITLE)
                .to_string(),
            back: self
                .lang
                .or_key(rewo_world::server_links_screen::KEY_BACK)
                .to_string(),
            // `ServerLinks.Entry.displayName()` —
            // `type.map(KnownLinkType::displayName, r -> r)`: the lang map for
            // a known type, the server's own component for a custom one.
            links: self
                .server_links
                .entries()
                .iter()
                .map(|e| match &e.label {
                    rewo_net::server_links::ServerLinkLabel::Known(t) => {
                        self.lang.or_key(&t.lang_key()).to_string()
                    }
                    rewo_net::server_links::ServerLinkLabel::Custom(text) => text.clone(),
                })
                .collect(),
        };
        log::info!("live: server-links dialog ({} link(s))", labels.links.len());
        self.view = ScreenView::Links(labels);
        self.rebuild_view_screen();
        self.grab_for_screen(true);
    }

    /// `createDisconnectScreen` — **the screen with no session behind it.**
    fn open_disconnect_screen(
        &mut self,
        cause: rewo_world::disconnect_screen::DisconnectCause,
        reason: String,
    ) {
        use rewo_net::server_links::KnownLinkType;
        // `serverLinks.findKnownType(BUG_REPORT).map(Entry::link)`, off the
        // durable mirror — the session is about to be dropped, and on the
        // `ClientError` path it may already be unusable.
        let candidate = self
            .server_links
            .find_known_type(KnownLinkType::BugReport)
            .map(|e| e.link.clone());
        let details = rewo_world::disconnect_screen::DisconnectDetails::new(
            cause,
            reason,
            candidate.as_deref(),
        );
        let labels = rewo_world::disconnect_screen::DisconnectLabels::resolve(&self.lang);
        log::warn!(
            "live: disconnected ({cause:?}): {} — bug report link: {:?}",
            details.reason,
            details.bug_report_link
        );
        self.view = ScreenView::Disconnected(labels, details);
        self.session = None;
        self.rebuild_view_screen();
        self.grab_for_screen(true);
    }

    /// `Screen.resize` → `repositionElements` → `rebuildWidgets` → `init()`,
    /// for whichever of M85's screens is up. Also the opener: building a
    /// screen and rebuilding it are the same call, which is exactly vanilla's
    /// arrangement and the reason `ScreenView` carries what it does.
    fn rebuild_view_screen(&mut self) {
        let (gw, gh) = self.gui_size();
        if gw <= 0 || gh <= 0 {
            return;
        }
        let advance = self.advance();
        let screen = match &self.view {
            ScreenView::None => return,
            ScreenView::Pause(labels, has_links) => {
                let tw = self.text_width(&labels.title);
                rewo_world::pause_screen::build(labels, *has_links, tw, gw, gh)
            }
            ScreenView::Links(labels) => {
                let tw = self.text_width(&labels.title);
                rewo_world::server_links_screen::build(labels, tw, gw, gh)
            }
            ScreenView::Disconnected(labels, details) => {
                let width_of = move |t: &str| match &advance {
                    Some(a) => rewo_gpu::text::width(t, a),
                    None => 0,
                };
                rewo_world::disconnect_screen::build(labels, details, gw, gh, &width_of)
            }
        };
        self.screen.screens.open(screen);
    }

    /// Close whichever of M85's screens is up and hand the cursor back.
    fn close_view_screen(&mut self) {
        self.view = ScreenView::None;
        self.screen.screens.close();
        self.grab_for_screen(false);
    }

    /// One widget press on whatever screen is up (M82).
    ///
    /// This is the dispatch arm a new screen adds: the framework hands back a
    /// `(ScreenKind, WidgetId)` and the app decides what it means.
    fn press_widget(&mut self, kind: rewo_world::screen::ScreenKind, id: rewo_world::screen::WidgetId) {
        use rewo_world::death_screen as ds;
        use rewo_world::screen::ScreenKind;
        match (kind, id) {
            // M84: the statistics screen. `Done` is `onClose()`, a tab press is
            // `selectTab`, and a sort button is `sortByColumn` — the last two
            // rebuild, because a tab change moves the six sort widgets and
            // reselects every tab's sheet.
            (ScreenKind::Stats, rewo_world::stats_screen::DONE) => {
                self.close_stats();
            }
            (ScreenKind::Stats, id) => {
                if self.stats.as_mut().is_some_and(|v| v.press(id)) {
                    self.rebuild_stats_screen();
                }
            }
            (ScreenKind::Death, ds::RESPAWN) => {
                if let Some(session) = self.session.as_mut() {
                    if let Err(e) = session.perform_respawn() {
                        log::warn!("live: respawn: {e}");
                    }
                }
                // `button.active = false` — the second half of vanilla's
                // `onPress`. The screen stays up until the server's respawn
                // arrives; this is what stops a double press.
                if let Some(s) = self.screen.screens.current_mut() {
                    if let Some(w) = s.widget_mut(ds::RESPAWN) {
                        w.active = false;
                    }
                }
                // `KeyMapping.resetToggleKeys()`.
                self.keys = Keys::default();
            }
            // M85's three screens.
            (ScreenKind::Pause, rewo_world::pause_screen::RETURN_TO_GAME) => {
                // `this.minecraft.gui.setScreen(null); mouseHandler.grabMouse();`
                self.close_view_screen();
            }
            (ScreenKind::Pause, rewo_world::pause_screen::SERVER_LINKS) => {
                // `minecraft.player.connection.showDialog(dialog, this)`.
                self.open_links_screen();
            }
            (ScreenKind::Pause, rewo_world::pause_screen::DISCONNECT) => {
                // `minecraft.disconnectFromWorld(DEFAULT_QUIT_MESSAGE)`. Rewo
                // has no title screen to land on, so leaving the session is
                // the whole of it — the same call the death screen's second
                // button makes.
                log::info!("live: pause screen — leaving the server");
                self.exit_requested = true;
            }
            (ScreenKind::Pause, id) => {
                // Advancements, Statistics and Options. Drawn as vanilla draws
                // them and inert on press: `award_stats` is a sibling
                // milestone's screen and the other two do not exist. Logged
                // rather than silently swallowed.
                log::info!("live: pause screen — widget {id} is not implemented");
            }
            (ScreenKind::ServerLinks, rewo_world::server_links_screen::BACK) => {
                // `DialogAction.CLOSE` → `previousScreen`, which is the pause
                // screen this dialog was opened from.
                self.open_pause_screen();
            }
            (ScreenKind::ServerLinks, id) => {
                // **Rewo does not open a URL.** Vanilla's path is
                // `StaticAction(ClickEvent.OpenUrl)` → `Screen.clickUrlAction`,
                // which itself shows a `ConfirmLinkScreen` unless the player
                // turned the prompt off. Launching a browser from a string a
                // remote server chose is a decision, not a transcription — see
                // `rewo_net::server_links`.
                let i = rewo_world::server_links_screen::link_index(id).unwrap_or(0);
                match self.server_links.entries().get(i) {
                    Some(e) => log::info!(
                        "live: server link {i} selected: {} (Rewo does not open URLs)",
                        e.link
                    ),
                    None => log::warn!("live: server link {i} has no entry"),
                }
            }
            (ScreenKind::Disconnected, _) => {
                // `gui.toMenu` / `gui.toTitle` — a server list and a title
                // screen, neither of which Rewo has. Leaving is the whole of
                // it.
                log::info!("live: disconnect screen — exiting");
                self.exit_requested = true;
            }
            (ScreenKind::Death, ds::TITLE_SCREEN) => {
                // `exitToTitleScreen`: `level.disconnect(...)` then
                // `disconnectWithSavingScreen()` then a `TitleScreen`. Rewo
                // has no title screen, so leaving the session is the whole of
                // it. The `ConfirmScreen` vanilla interposes for a non-hardcore
                // world is not reproduced — see `death_screen`'s docs.
                log::info!("live: death screen — leaving the server");
                self.exit_requested = true;
            }
            _ => {}
        }
    }

    /// Open, reposition and close the death screen (M82).
    ///
    /// Three separate rules, and only the first is the packet's:
    ///
    /// 1. `handlePlayerCombatKill` opens it. The session has already taken
    ///    vanilla's other branch (`!shouldShowDeathScreen()` → respawn
    ///    immediately), so a drained death always means "show the screen".
    /// 2. `Screen.resize` rebuilds it — which is `init()`, so the one-second
    ///    button guard restarts. That is vanilla's behaviour and not a bug.
    /// 3. **`handleRespawn` closes it**, not the button press. Watched through
    ///    the session's respawn watermark.
    fn pump_death_screen(&mut self) {
        use rewo_world::screen::ScreenKind;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let (respawns, kill) = (session.respawn_epoch(), session.take_death());
        let (hardcore, score) = (session.hardcore, session.score);
        if let Some(kill) = kill {
            let (gw, gh) = self.gui_size();
            let lang = self.lang.clone();
            let (view, screen) =
                DeathView::open(&kill, hardcore, score, &lang, respawns, gw, gh);
            log::info!(
                "live: death screen — \"{}\", hardcore={hardcore}, score={score}",
                view.model.cause_of_death.as_deref().unwrap_or("")
            );
            self.death = Some(view);
            self.screen.screens.open(screen);
            self.grab_for_screen(true);
            return;
        }
        let Some(view) = self.death.as_ref() else {
            return;
        };
        if respawns != view.respawn_epoch {
            log::info!("live: respawned — closing the death screen");
            self.death = None;
            self.screen.screens.close();
            self.grab_for_screen(false);
            return;
        }
        // A resize while dead. Compared against the screen's recorded size so
        // a frame that changes nothing rebuilds nothing — a rebuild resets the
        // guard, and doing it every frame would leave the buttons dead forever.
        let (gw, gh) = self.gui_size();
        let stale = self
            .screen
            .screens
            .current()
            .is_some_and(|s| s.kind == ScreenKind::Death && (s.width != gw || s.height != gh));
        if stale {
            if let (Some(view), Some(s)) = (self.death.as_ref(), self.screen.screens.current_mut()) {
                view.reposition(s, gw, gh);
            }
        }
    }

    /// One frame with **no session** — the disconnect screen (M85).
    ///
    /// Deliberately not a cut-down copy of [`Self::frame`]: it renders the
    /// screen pass and the text pass and nothing else, because with no world
    /// there is nothing else to render. The view-projection is the identity,
    /// which is what the offscreen gates have passed since M82 and which the
    /// (empty) world pass does not read anyway.
    fn render_screen_only(&mut self, event_loop: &ActiveEventLoop) {
        if self.exit_requested {
            event_loop.exit();
            return;
        }
        let mouse_gui = self.mouse_gui();
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let extent = state.renderer.swapchain.extent;
        let px = gui_px(extent.width, extent.height);
        let mut chrome = rewo_gpu::screen::ScreenDraw::default();
        let mut text = Vec::new();
        if let Some(screen) = self.screen.screens.current() {
            chrome = screen_chrome(screen, Some(mouse_gui));
            if let Some(advance) = state.world_renderer.font_advance() {
                text = screen_text_lines(screen, &advance, px);
            }
        }
        state.world_renderer.set_screen(chrome);
        state.world_renderer.set_text(text);
        let draw = OverlayDraw {
            samples_ms: &self.ring.data,
            head: self.ring.head(),
            scale_ms: 20.0,
            // Off-screen: the strip chart measures a frame loop that is no
            // longer running.
            origin: [-4000.0, -4000.0],
            size: [8.0, 8.0],
        };
        let LiveState {
            window,
            gpu,
            renderer,
            world_renderer,
        } = state;
        let vp = glam::Mat4::IDENTITY.to_cols_array_2d();
        match renderer.render(gpu, Some((world_renderer, vp)), &draw, CLEAR_SKY) {
            Ok(RenderOutcome::Rendered) | Ok(RenderOutcome::Skipped) => {}
            Ok(RenderOutcome::NeedsRecreate) => {
                let size = window.inner_size();
                let _ = renderer.recreate(gpu, size.width, size.height);
                let _ = renderer.ensure_depth(gpu);
            }
            Err(e) => {
                log::error!("live: render failed: {e}");
                event_loop.exit();
            }
        }
        window.request_redraw();
        // **The soak deadline lives here too, and its absence is the one thing
        // the live run caught that the gate could not.**
        //
        // `--run-seconds` was checked at the bottom of `frame`, past the point
        // where the session is borrowed — so a client that lost its connection
        // ran forever. That is the shape this milestone is about: everything
        // the frame loop does after acquiring a session was *implicitly*
        // session-gated, and a screen with no session behind it is what makes
        // the implicit gate observable.
        if let Some(limit) = self.run_seconds {
            if self.started.elapsed().as_secs_f32() >= limit {
                event_loop.exit();
            }
        }
    }

    /// Open the statistics screen and ask the server for the numbers (M84).
    ///
    /// `StatsScreen.init()`'s last line is the `REQUEST_STATS` client command,
    /// so the request is part of *opening*, not of rendering — a screen opened
    /// with no request sits on "Retrieving statistics…" forever.
    fn open_stats(&mut self) {
        let (gw, gh) = self.gui_size();
        let labels = rewo_world::stats_screen::StatsLabels::resolve(&self.lang);
        let counter = self
            .session
            .as_ref()
            .map(|s| s.stats.clone())
            .unwrap_or_default();
        let (view, screen) = crate::stats_view::StatsView::build(
            &counter,
            &self.stat_registries,
            &self.items,
            &self.etypes,
            &self.lang,
            labels,
            Default::default(),
            (None, 0),
            gw,
            gh,
        );
        log::info!(
            "live: statistics screen — {} stats held, loading={}",
            counter.len(),
            view.model.loading
        );
        self.stats = Some(view);
        self.screen.screens.open(screen);
        self.grab_for_screen(true);
        if let Some(session) = self.session.as_mut() {
            if let Err(e) = session.request_stats() {
                log::warn!("live: request_stats: {e}");
            }
        }
    }

    fn close_stats(&mut self) {
        self.stats = None;
        self.screen.screens.close();
        self.grab_for_screen(false);
    }

    /// `repositionElements` — rebuild from the current counter, keeping the
    /// tab, the sort and the scroll.
    fn rebuild_stats_screen(&mut self) {
        let (gw, gh) = self.gui_size();
        let Some(view) = self.stats.as_ref() else {
            return;
        };
        let (tab, sort) = (
            view.model.tab,
            (view.model.sort_column, view.model.sort_order),
        );
        let scrolls: Vec<f64> = view.model.lists.iter().map(|l| l.scroll()).collect();
        let labels = view.labels.clone();
        let counter = self
            .session
            .as_ref()
            .map(|s| s.stats.clone())
            .unwrap_or_default();
        let (mut view, screen) = crate::stats_view::StatsView::build(
            &counter,
            &self.stat_registries,
            &self.items,
            &self.etypes,
            &self.lang,
            labels,
            tab,
            sort,
            gw,
            gh,
        );
        // The scroll survives a rebuild, re-clamped against the new content —
        // `updateSizeAndPosition` ends in `refreshScrollAmount()`, which is
        // `setScrollAmount(scrollAmount)` and therefore exactly this clamp.
        for (l, v) in view.model.lists.iter_mut().zip(scrolls) {
            l.set_scroll(v);
        }
        self.stats = Some(view);
        self.screen.screens.open(screen);
    }

    /// Rebuild when the numbers or the window changed (M84).
    fn pump_stats_screen(&mut self) {
        use rewo_world::screen::ScreenKind;
        if self.stats.is_none() {
            return;
        }
        let updates = self.session.as_ref().map(|s| s.stats.updates).unwrap_or(0);
        let (gw, gh) = self.gui_size();
        let stale = self
            .screen
            .screens
            .current()
            .is_some_and(|s| s.kind == ScreenKind::Stats && (s.width != gw || s.height != gh));
        let fresh = self.stats.as_ref().is_some_and(|v| v.built_from != updates);
        if stale || fresh {
            self.rebuild_stats_screen();
        }
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|l| now.duration_since(l).as_secs_f32())
            .unwrap_or(0.0)
            .min(0.1);
        self.last_frame = Some(now);
        if dt > 0.0 {
            self.cpu.push(dt * 1000.0);
            self.ring.push(dt * 1000.0);
        }

        // M74: `container_close` — the server closing whatever screen is
        // open. Drained before the session borrow below, because acting on it
        // calls `set_screen_open`, which needs all of `self`.
        //
        // Compared as a watermark, not consumed as a flag: the session owns
        // the counter and this is the only reader, so two closes in one frame
        // collapse to one screen close without either being lost.
        let close_requested = match self.session.as_ref() {
            Some(s) => {
                let n = s.client_state.close_container_requests();
                let changed = n != self.screen.close_requests_seen;
                self.screen.close_requests_seen = n;
                changed
            }
            None => false,
        };
        if close_requested && self.screen.inventory_open() {
            self.set_screen_open(false);
        }

        // M89 — a container the server opened opens the client's screen.
        //
        // M87 decoded `open_screen` into `Menus` and rendered whatever menu
        // was open, but nothing turned the screen ON, so right-clicking a
        // chest recorded the menu and showed nothing: the render only engaged
        // if the player separately pressed E while a container happened to be
        // open. `handleOpenScreen` is `MenuScreens.create`, which *is* the
        // screen opening — the two are one action in vanilla.
        //
        // Watermarked like `close_requested` above rather than compared to the
        // screen's state, so a server that re-opens the same container id (a
        // fresh menu, per `reopening_replaces_the_slots_rather_than_keeping_them`)
        // is not mistaken for the one already showing.
        let opened = self
            .session
            .as_ref()
            .and_then(|s| s.menus.open().map(|m| m.container_id));
        if opened != self.screen.container_shown {
            self.screen.container_shown = opened;
            match opened {
                Some(_) => self.set_screen_open(true),
                // The menu closing closes the screen with it — but only if a
                // container was what put it up. Pressing E with no container
                // open must not be closed by this.
                None if self.screen.inventory_open() => self.set_screen_open(false),
                None => {}
            }
        }

        // M86's gate drives the inventory open for the second half of its run.
        //
        // The screen is one of the paths the bake fix re-enables, and it is the
        // one carrying the ninth instance of this milestone's bug —
        // `VelvetTextPass::sync_atlas` destroying its glyph image and rewriting
        // its descriptor set in place, whose own comment said "the caller is
        // expected to have idled" of a caller that does not. Nothing reaches it
        // except an open inventory, so a check that never opens one would grade
        // that fix not at all. The mouse is parked over the panel so a tooltip
        // lays out glyphs and the cache actually goes dirty.
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

        // M85: mirror the server's links out of the session while it is
        // alive. The disconnect screen reads them *after* it is gone — see the
        // field's docs.
        if let Some(s) = self.session.as_ref() {
            if s.session.server_links != self.server_links {
                self.server_links = s.session.server_links.clone();
                log::info!("live: {} server link(s)", self.server_links.len());
            }
        }

        // M85: the connection ended. Before the session borrow below, because
        // it drops the session — and vanilla's `onDisconnect` is likewise the
        // thing that tears the level down rather than something the level does.
        let ended = self.session.as_ref().and_then(|s| {
            s.disconnect.clone().map(|reason| {
                (
                    s.disconnect_cause
                        .unwrap_or(rewo_world::disconnect_screen::DisconnectCause::EndOfStream),
                    reason,
                )
            })
        });
        if let Some((cause, reason)) = ended {
            self.death = None;
            self.open_disconnect_screen(cause, reason);
        }

        // M85: a window resize while one of M85's screens is up. Same
        // watermark shape as `pump_death_screen`'s — compare the screen's
        // recorded size so a frame that changes nothing rebuilds nothing.
        {
            let (gw, gh) = self.gui_size();
            let stale = !matches!(self.view, ScreenView::None)
                && self
                    .screen
                    .screens
                    .current()
                    .is_some_and(|s| s.width != gw || s.height != gh);
            if stale {
                self.rebuild_view_screen();
            }
        }

        // M82: you died, or you respawned. Both before the session borrow
        // below, for the same reason `container_close` is.
        self.pump_death_screen();
        self.pump_stats_screen();
        if self.exit_requested {
            event_loop.exit();
            return;
        }

        // **M85: the frame with no session.**
        //
        // Every frame before this one needed a world: the loop below borrows
        // `self.session` and returns if there is none, which is why the client
        // used to `event_loop.exit()` on a disconnect rather than showing a
        // screen. The disconnect screen exists *because* there is no session,
        // so it gets its own arm — appended, not woven into the path below,
        // which stays exactly as it was.
        //
        // What it draws is the screen and nothing else. The world pass runs
        // with an identity view-projection over an empty world (no columns, no
        // entities), which is what `deathshot` has been doing offscreen since
        // M82; the menu background is opaque, so nothing behind it is visible
        // anyway.
        if self.session.is_none() {
            self.render_screen_only(event_loop);
            return;
        }

        // Fixed 20 Hz tick on an accumulator.
        let Some(session) = self.session.as_mut() else {
            return;
        };
        self.tick_accum += dt;
        let input = self.keys.input();
        let mut ran_tick = false;
        while self.tick_accum >= TICK_DT {
            self.tick_accum -= TICK_DT;
            if let Err(e) = session.tick(&input) {
                // `onPacketError` — a handler threw. Vanilla's is the path that
                // fills `bugReportLink`, so this is a `ClientError` and the
                // server's own bug-report link (if it sent one) appears on the
                // disconnect screen (M85).
                //
                // Recorded onto the session rather than acted on here: `self`
                // is mutably borrowed through `session` for this whole block,
                // and routing it through the one field the top of the frame
                // already reads keeps every disconnect on one path.
                log::error!("live: tick failed: {e}");
                session.disconnect = Some(e);
                session.disconnect_cause =
                    Some(rewo_world::disconnect_screen::DisconnectCause::ClientError);
                return;
            }
            // Advance the block-light flicker exactly once per successful tick.
            self.flicker.tick();
            // M71 — a client-generated system message (currently only
            // `NO_RESPAWN_BLOCK_AVAILABLE`) is queued as a *translation key*,
            // because vanilla builds a `Component.translatable` and resolves
            // it against the loaded language at render time. This is that
            // resolution; the key itself is the fallback, which is what
            // vanilla shows for a key the language file lacks.
            for key in session.game_state.take_system_messages() {
                let text = self
                    .baked
                    .as_ref()
                    .and_then(|b| b.lang.get(key))
                    .unwrap_or(key)
                    .to_string();
                session.chat_log.push(text);
            }
            // `Hud.tick`'s held-item label clock (M66) — once per client tick,
            // and it reads the selected stack *after* the tick that may have
            // changed it, exactly as vanilla's `Gui.tick` does.
            let label = self
                .baked
                .as_ref()
                .and_then(|b| selected_item_label(session, &self.items, &b.item_names));
            self.tool_highlight.tick(
                label.as_ref().map(|(id, n)| (*id, n.as_str())),
                NOTIFICATION_DISPLAY_TIME,
            );
            // M82: `Screen.tick()` — once per client tick, for whatever screen
            // is up. The death screen's is `delayTicker++`, and its buttons
            // arm at exactly 20.
            if self.death.is_some() {
                if let Some(s) = self.screen.screens.current_mut() {
                    rewo_world::death_screen::DeathScreen::tick(s);
                }
            }
            ran_tick = true;
        }
        if session.disconnect.is_some() {
            // Serviced at the top of the *next* frame, by the block that opens
            // the disconnect screen. Returning here rather than acting is what
            // keeps that decision in one place — and this frame has already
            // borrowed the session it is about to drop.
            return;
        }
        if ran_tick && session.spawned && !self.logged_spawn {
            self.logged_spawn = true;
            // `REWO_PRECMD`: the same semicolon-separated op-command knob the
            // headless path has had since M64, wired into the windowed one
            // (M82) — without it a windowed run cannot stage anything that
            // needs a command, and the death screen needs `/kill`.
            if let Ok(cmd) = std::env::var("REWO_PRECMD") {
                for one in cmd.split(';').map(str::trim).filter(|c| !c.is_empty()) {
                    let _ = session.send_command(one);
                    log::info!("REWO_PRECMD: {one}");
                }
            }
            log::info!(
                "live: spawned at ({:.1},{:.1},{:.1})",
                session.player.x,
                session.player.y,
                session.player.z
            );
            // Put a stack of dirt in slot 0 so right-click can place (the
            // test server is creative — a no-op elsewhere).
            if let Some(dirt) = self.dirt_item {
                let _ = session.creative_set_hotbar(0, dirt, 64);
                let _ = session.select_hotbar(0);
            }
        }

        let Some(state) = self.state.as_mut() else {
            return;
        };
        // Upload finished meshes + feed the worker pool. (Uploads are
        // async slot-ring submissions — the CPU never waits on the copy;
        // same-queue FIFO ordering keeps this frame's draws safe.)
        match pump_meshing(
            session,
            &mut state.gpu,
            &mut state.world_renderer,
            &mut self.pool,
            UPLOAD_BUDGET,
        ) {
            Ok(n) => {
                self.uploaded_total += n;
                // Log once when the pool first idles after spawn + a settle
                // margin (the chunk stream arrives over the first seconds —
                // firing on the tiny pre-stream batch would be misleading).
                if !self.flood_logged
                    && session.spawned
                    && self.uploaded_total > 0
                    && self.pool.in_flight() == 0
                    && session.dirty_len() == 0
                    && self.started.elapsed().as_secs_f32() >= 2.0
                {
                    self.flood_logged = true;
                    log::info!(
                        "live: initial mesh flood done — {} uploads for {} columns in {:.1}s",
                        self.uploaded_total,
                        session.world.loaded_columns(),
                        self.started.elapsed().as_secs_f32()
                    );
                }
            }
            Err(e) => {
                log::error!("live: remesh failed: {e}");
                event_loop.exit();
                return;
            }
        }

        // Player skins: request any newly-announced ones, upload any that
        // finished fetching (real skins on online-mode servers).
        for (uuid, info) in session.take_pending_skins() {
            self.skins.request(uuid, &info);
        }
        self.skins
            .poll_uploads(&mut state.gpu, &mut state.world_renderer);

        // Entities: frame-interpolated snapshot + camera-billboarded tags.
        let alpha = (self.tick_accum / TICK_DT).clamp(0.0, 1.0);
        let anim_time = self.started.elapsed().as_secs_f32();
        // Resolve ONE camera lightmap for this frame at a FIXED partial of 1.0
        // — not the entity-interpolation `alpha`. Vanilla `GameRenderer` (lines
        // 376-386) calls `lightmapRenderStateExtractor.extract(lightmapRenderState,
        // 1.0F)` with a hard-coded 1.0F, so the camera lightmap is resolved at
        // the tick boundary regardless of the frame's partial tick. Entity
        // position interpolation still uses `alpha` (below); only the lightmap /
        // effect snapshot is pinned to 1.0. This matches the headless path.
        let lightmap_partial = 1.0;
        let snapshot = session.visual_effect_snapshot(lightmap_partial);
        let lightmap = resolve_lightmap(
            session.day_ticks,
            session.active_dimension_type.as_ref(),
            self.flicker.block_factor(),
            snapshot,
            // M52 Full Bright: pin vanilla's MAXIMUM gamma rather than
            // bypassing the lightmap. The value still goes through
            // `darkness_lightmap` -> `brightness_factor` -> the exact mix M13
            // transcribed, so night vision and the darkness effect keep
            // composing with it correctly. A bypass would have made Full
            // Bright silently defeat both.
            if self.modules.is_on("fullbright") {
                crate::modules::MAX_GAMMA
            } else {
                self.gamma
            },
            self.darkness_option,
            lightmap_partial,
        );
        let trim_slots = ensure_trims(
            session,
            &self.items,
            &self.trims,
            &mut state.gpu,
            &mut state.world_renderer,
        );
        let draws = collect_entities(
            session,
            &self.etypes,
            alpha,
            &mut self.gestures,
            anim_time,
            &self.skins.registry,
            &lightmap,
            &self.spears,
            self.bow_item,
            &self.items,
            &self.equipment,
            &trim_slots,
            &self.etf,
            self.hud_hidden,
            frame_crosshair_pick(session, &self.etypes, alpha),
        );
        let (cr, cu) = camera_basis(session.player.yaw, session.player.pitch);
        let eye = player_eye(session);
        // Before `set_entities`: the entity pass reads the eye as the CEM
        // `player_pos_*`, which FA aims mob eyes/heads with.
        state.world_renderer.set_camera(eye.to_array());
        apply_lightmap(
            &mut state.world_renderer,
            &lightmap,
            session.day_ticks,
            session.active_dimension_type.as_ref(),
            {
                let w = effective_weather(session);
                (w.rain_level(), w.thunder_level())
            },
            {
                let band = match self.baked.as_ref() {
                    Some(baked) => {
                        let w = self.weather.get_or_insert_with(|| WeatherAssets::new(baked));
                        rain_fog_band(session, w, Some(dt * 20.0))
                    }
                    None => [1.0e9, 1.0e9 + 1.0],
                };
                // r3: the sentinel the dead branch produced is 1e9 blocks out.
                // Counting a *finite* band rather than "we took the Some arm"
                // is what makes this a value witness — the arm could be taken
                // and still hand the renderer nonsense.
                if let Some(c) = self.check.as_mut() {
                    if band[0] < 1.0e8 {
                        c.fog_band_frames += 1;
                    }
                }
                band
            },
        );
        apply_biome_sky_fog(&mut state.world_renderer, session);
        let bes = collect_block_entities(&session.world, &self.chest_states, &lightmap, alpha, session.game_time(), (cr, cu));
        // A spawner's caged mob rides the ENTITY pass, mounted inside its
        // block (M31), so it joins the entity draws rather than these.
        let caged = collect_spawner_mobs(
            &session.world,
            &self.etypes,
            self.chest_states.spawner_states(),
            &lightmap,
            alpha,
        );
        let portals = collect_end_portals(
            &session.world,
            self.chest_states.end_portal_states(),
            self.chest_states.end_gateway_states(),
        );
        if let Err(e) = state.world_renderer.set_end_portals(
            &mut state.gpu,
            &portals,
            session.game_time(),
        ) {
            log::warn!("live: end portal upload failed: {e}");
        }
        let mut draws = draws;
        draws.extend(caged.iter().map(spawner_mob_draw));
        draws.extend(collect_pickups(
            session,
            &self.items,
            &lightmap,
            alpha,
            anim_time,
        ));
        let mut held: Vec<&str> = draws.iter().flat_map(|d| d.held).flatten().collect();
        held.extend(draws.iter().filter_map(|d| d.ground_item));
        held.extend(bes.iter().map(|b| b.model.as_str()));
        // A failed upload leaves the item simply absent (no resident slot →
        // no quads), which is preferable to killing the frame loop.
        if let Err(e) = state.world_renderer.prepare_held_items(&mut state.gpu, &held) {
            log::warn!("live: held-item texture upload: {e}");
        }
        let be_draws: Vec<_> = bes.iter().map(|b| b.as_draw()).collect();
        let sign_lines = match state.world_renderer.font_advance() {
            Some(a) => collect_sign_text(&session.world, &self.sign_states, &lightmap, a),
            None => Vec::new(),
        };
        let sign_draws: Vec<_> = sign_lines
            .iter()
            .map(|l| rewo_gpu::entities::WorldTextDraw {
                transform: l.transform,
                text: &l.text,
                x: l.x,
                y: l.y,
                z: l.z,
                color: l.color,
                light: l.light,
            })
            .collect();
        state.world_renderer.set_entities_and_block_entities(
            &draws,
            &be_draws,
            &sign_draws,
            cr,
            cu,
            anim_time,
        );
        drop(draws);

        let extent = state.renderer.swapchain.extent;
        let aspect = extent.width.max(1) as f32 / extent.height.max(1) as f32;
        // Targeted block for the selection outline.
        let hit = session.target_block(
            eye_f64(session),
            look_dir(session.player.yaw, session.player.pitch),
            REACH,
        );
        state.world_renderer.set_selection(hit.map(|h| h.block));
        // Camera + lightmap were already set above from the same eye/state this
        // frame — don't duplicate them. Celestial still tracks the world clock.
        state
            .world_renderer
            .set_celestial({
                let mut cel = celestial_state_of(session.day_ticks);
                apply_weather_to_celestial(&mut cel, session);
                cel
            });
        // M34/M35: the item icons, before the weather so the borrow of
        // `baked` is over by the time `apply_weather` takes its own. One pass
        // serves both the hotbar and the open screen — only the rectangles
        // differ.
        let ext = state.window.inner_size();
        let (sw, sh) = (ext.width as f32, ext.height as f32);
        let hovered = self
            .screen
            .inventory_open()
            .then(|| self.screen.hovered(session.shown_menu().layout(), sw, sh))
            .flatten();
        if let Some(baked) = self.baked.as_ref() {
            if let Some(c) = self.check.as_mut() {
                c.gui_item_frames += 1;
            }
            let items = self.items.clone();
            let gi = self.gui_items.get_or_insert_with(|| GuiItemState::new(baked));
            if self.screen.inventory_open() {
                if let Some(c) = self.check.as_mut() {
                    c.screen_frames += 1;
                }
                let (labels, velvet) = apply_screen(
                    &mut state.world_renderer,
                    &mut state.gpu,
                    session,
                    &items,
                    gi,
                    baked,
                    self.preview_skin.as_mut(),
                    self.glyphs.as_mut(),
                    rewo_gpu::tooltip::TooltipFlag::of(self.advanced_tooltips),
                    self.screen.mouse,
                    (sw, sh),
                );
                self.screen_labels = labels;
                // M88 — read the panel back OUT of the renderer, after the
                // draw path set it. Asking the open menu's layout instead
                // would answer 168 for a chest whether or not the panel
                // builder returned one.
                if let Some(h) = state.world_renderer.container_panel_height() {
                    let forced = self.screen_forced_open;
                    if let Some(c) = self.check.as_mut() {
                        c.container_frames += 1;
                        c.container_panel_h = Some(h);
                        if !forced {
                            c.container_self_opened_frames += 1;
                        }
                    }
                }
                // Any glyph laid out above may be new to the atlas. Sync
                // BEFORE the runs are drawn and outside the rendering scope --
                // it records a transfer, and a run referencing a rect that has
                // not reached the GPU samples whatever was there before.
                if let Some(cache) = self.glyphs.as_mut() {
                    if let Err(e) = state.world_renderer.sync_velvet_atlas(&mut state.gpu, cache) {
                        log::warn!("velvet atlas sync: {e}");
                    }
                }
                state.world_renderer.set_velvet_runs(velvet);
            } else {
                self.screen_labels.clear();
                state.world_renderer.set_velvet_runs(Vec::new());
                state.world_renderer.set_preview(None);
                apply_hotbar_icons(
                    &mut state.world_renderer,
                    &mut state.gpu,
                    session,
                    &items,
                    gi,
                    (sw, sh),
                );
            }
        }
        if !self.screen.inventory_open() {
            state.world_renderer.set_container(false, None);
        }
        let _ = hovered;
        // M38: the first-person hand. Suppressed while the inventory screen is
        // open — the screen owns the view, and vanilla does the same.
        if let Some(baked) = self.baked.as_ref() {
            if let Some(c) = self.check.as_mut() {
                c.hand_frames += 1;
            }
            let items = self.items.clone();
            let h = self.hand.get_or_insert_with(|| HandState::new(baked));
            h.tick(session, &items);
            if self.screen.inventory_open() {
                let _ = state
                    .world_renderer
                    .set_hand(&mut state.gpu, &[], [[0.0; 4]; 4]);
            } else {
                apply_hand(
                    &mut state.world_renderer,
                    &mut state.gpu,
                    session,
                    &items,
                    h,
                    alpha,
                    sw / sh,
                );
            }
        }
        // M33: the cloud deck and this frame's precipitation. The assets are
        // built lazily because `baked` arrives with the session.
        if let Some(baked) = self.baked.as_ref() {
            if let Some(c) = self.check.as_mut() {
                c.weather_frames += 1;
            }
            let w = self.weather.get_or_insert_with(|| WeatherAssets::new(baked));
            apply_weather(
                &mut state.world_renderer,
                &mut state.gpu,
                session,
                w,
                alpha,
                // 20 ticks per second — .
                Some(dt * 20.0),
            );
            apply_border(&mut state.world_renderer, &mut state.gpu, session, alpha);
            if self.particles.is_none() {
                self.particles = ParticleAssets::new(baked);
            }
            if let Some(p) = self.particles.as_mut() {
                let eye = player_eye(session);
                let view =
                    eye_view(eye, session.player.yaw, session.player.pitch).to_cols_array_2d();
                let events = std::mem::take(&mut session.particle_events);
                apply_particles(
                    &mut state.world_renderer,
                    &mut state.gpu,
                    session,
                    events,
                    p,
                    baked,
                    alpha,
                    view,
                );
            }
            apply_crumbling(
                &mut state.world_renderer,
                &mut state.gpu,
                session,
                baked,
                player_eye(session),
            );
        }
        state
            .world_renderer
            .set_hud(
                session.health,
                session.food,
                self.hotbar_slot,
                resolve_hud_gauges(
                    &session.hud,
                    &session.inventory,
                    &self.items,
                    has_experience(session),
                    alpha,
                ),
            );
        let px = gui_px(extent.width, extent.height);
        let fps = (!self.cpu.is_empty()).then(|| 1000.0 / self.cpu.average().max(0.001));
        let mut text = build_text(session, px, extent.height as f32, fps, self.debug);
        // M66: the held-item name over the hotbar. Needs the font's advances
        // to centre itself, so it is built here rather than in `build_text`.
        if let Some(advance) = state.world_renderer.font_advance() {
            text.extend(selected_item_name_line(
                session,
                &self.items,
                &self.tool_highlight,
                &advance,
                px,
                (extent.width as f32, extent.height as f32),
            ));
            // M79: the XP level number, then the title / subtitle / action
            // bar. The titles go last so they sit over everything the HUD
            // draws, which is where `nextStratum()` puts them in vanilla.
            text.extend(experience_level_lines(
                &session.hud.experience,
                has_experience(session),
                self.baked.as_ref().map(|b| &b.lang),
                &advance,
                px,
                (extent.width as f32, extent.height as f32),
            ));
            text.extend(title_lines(
                &session.hud.titles,
                &advance,
                px,
                (extent.width as f32, extent.height as f32),
                // `deltaTracker.getGameTimeDeltaPartialTick(false)` — this
                // frame's fraction of the way into the current tick, the same
                // `alpha` the entity lerps use.
                alpha,
            ));
        }
        // The stack counts are text like any other line, drawn after the icons
        // because the text pass runs last.
        text.extend(self.screen_labels.drain(..));
        // M82: the death screen — its chrome into the screen pass, its four
        // text runs onto the end of this frame's lines. Last, so the title,
        // the cause and the button labels sit over the HUD, which is where
        // `extractRenderStateWithTooltipAndSubtitles`'s stratum order puts
        // everything a screen draws.
        {
            let mut chrome = rewo_gpu::screen::ScreenDraw::default();
            // Every hover test wants the cursor in GUI space — see
            // `LiveApp::mouse_gui` for the M82 bug this fixes, and for why all
            // five screens share the one conversion.
            let mouse_gui = (
                self.screen.mouse.0 / px as f64,
                self.screen.mouse.1 / px as f64,
            );
            // M84: the statistics screen, on the same seam. Only one screen is
            // ever up, so the three arms are exclusive by construction rather
            // than by an ordering rule.
            if let (Some(view), Some(screen)) = (self.stats.as_ref(), self.screen.screens.current())
            {
                let advance = state.world_renderer.font_advance();
                chrome = crate::stats_view::chrome(
                    view,
                    screen,
                    Some(mouse_gui),
                    advance.as_ref().map(|a| &**a),
                );
                if let Some(advance) = advance {
                    text.extend(crate::stats_view::lines(view, screen, &advance, px));
                }
            } else if self.death.is_some() {
                if let (Some(view), Some(screen)) =
                    (self.death.as_ref(), self.screen.screens.current())
                {
                    // The pass itself is built in `resumed`, beside
                    // `init_hud` and `init_container` — the one place the bake
                    // is still alive.
                    chrome = screen_chrome(screen, Some(mouse_gui));
                    if let Some(advance) = state.world_renderer.font_advance() {
                        text.extend(death_screen_lines(
                            view,
                            screen,
                            &advance,
                            px,
                            (extent.width as f32, extent.height as f32),
                        ));
                    }
                }
            } else if !matches!(self.view, ScreenView::None) {
                // M85's three screens. All of their text is on their widgets,
                // so one generic builder serves all three — the death screen
                // keeps its own only because its title, cause and score are
                // *not* widgets in vanilla either.
                if let Some(screen) = self.screen.screens.current() {
                    chrome = screen_chrome(screen, Some(mouse_gui));
                    if let Some(advance) = state.world_renderer.font_advance() {
                        text.extend(screen_text_lines(screen, &advance, px));
                    }
                }
            }
            state.world_renderer.set_screen(chrome);
        }
        state.world_renderer.set_text(text);
        if let Err(e) = state
            .world_renderer
            .anim_tick(&mut state.gpu, session.ticks)
        {
            log::error!("live: texture animation: {e}");
        }
        // M52: resolve the module state once per frame. Every legit module
        // defaults off, so an unconfigured client produces exactly the
        // constants this path used before -- which is what keeps the golden
        // PNGs byte-identical.
        let render_modules = self.modules.render();
        // M83's locator bar. After `render_modules` because its bearing window
        // is measured against the same FOV the frame is projected through --
        // the Zoom module divides it, and a dot must not drift out of a strip
        // that is still 60 degrees wide in the shader's terms.
        {
            let scale = rewo_gpu::hud::gui_scale(extent.width as f32, extent.height as f32);
            let bar = resolve_locator_bar(
                session,
                &session.world.entities,
                &self.locator_styles,
                render_modules.fov_degrees,
                (extent.width as f32 / scale) as i32,
                (extent.height as f32 / scale) as i32,
                alpha,
            );
            state.world_renderer.set_locator_bar(bar);
        }
        let vp = eye_view_proj_hurt(
            eye,
            session.player.yaw,
            session.player.pitch,
            aspect,
            render_modules.fov_degrees,
            local_hurt_tilt(session, alpha, render_modules.damage_tilt_strength),
        );
        let draw = OverlayDraw {
            samples_ms: &self.ring.data,
            head: self.ring.head(),
            scale_ms: 20.0,
            origin: [16.0, 16.0],
            size: [560.0, 140.0],
        };
        let LiveState {
            window,
            gpu,
            renderer,
            world_renderer,
        } = state;
        if std::mem::take(&mut self.capture_pending) {
            // Vanilla copies the framebuffer it just presented. Rewo's
            // swapchain images carry no `TRANSFER_SRC`, so the equivalent is to
            // render the same state again into an offscreen target — which is
            // also what makes a supersampled capture possible later.
            //
            // **At the swapchain's format**, not the gates' default: a
            // `WorldRenderer` bakes its colour format into every pipeline, so
            // the live renderer can only draw into a matching attachment.
            match crate::capture::grab(
                gpu,
                world_renderer,
                vp,
                &draw,
                renderer.swapchain.format,
                renderer.swapchain.extent,
            ) {
                Ok(path) => log::info!("saved screenshot as {}", path.display()),
                Err(e) => log::warn!("screenshot failed: {e}"),
            }
        }
        // M86's gate samples here: after every `set_*` this frame made and
        // before `render` consumes them, which is the only window in which the
        // rings' current slots are the ones about to be bound.
        let baked_live = self.baked.is_some();
        if let Some(c) = self.check.as_mut() {
            c.frames += 1;
            c.baked_frames += u64::from(baked_live);
            c.sample_rings(world_renderer);
            c.gui_items_ready |= world_renderer.gui_items_ready();
            c.hand_ready |= world_renderer.hand_ready();
            c.clouds_ready |= world_renderer.clouds_ready();
            c.weather_ready |= world_renderer.weather_ready();
            c.particles_ready |= world_renderer.particles_ready();
            c.border_ready |= world_renderer.border_ready();
            c.crumbling_ready |= world_renderer.crumbling_ready();
        }
        match renderer.render(gpu, Some((world_renderer, vp)), &draw, CLEAR_SKY) {
            Ok(RenderOutcome::Rendered) | Ok(RenderOutcome::Skipped) => {}
            Ok(RenderOutcome::NeedsRecreate) => {
                let size = window.inner_size();
                let _ = renderer.recreate(gpu, size.width, size.height);
                let _ = renderer.ensure_depth(gpu);
            }
            Err(e) => {
                log::error!("live: render failed: {e}");
                event_loop.exit();
            }
        }

        if let Some(limit) = self.run_seconds {
            if self.started.elapsed().as_secs_f32() >= limit {
                event_loop.exit();
            }
        }
    }
}

fn run_windowed(
    session: PlaySession,
    baked: assets::BakedAssets,
    etypes: EntityTypes,
    stat_registries: rewo_data::stats::StatRegistries,
    spears: rewo_data::item_tags::ItemTag,
    chest_states: rewo_data::chest_states::ChestStates,
    sign_states: rewo_data::sign_states::SignStates,
    bow_item: Option<i32>,
    items: std::sync::Arc<rewo_data::items::Items>,
    args: LiveArgs,
    want_validation: bool,
    dirt_item: Option<i32>,
) -> Result<(), String> {
    // M86's gate runs in the rain, unless the caller asked for something else.
    //
    // Not a convenience. Without it the precipitation pass is built and then
    // fed nothing, and — the part that actually bit — `rain_fog_band` returns
    // the very same `[1e9, 1e9 + 1]` the dead branch produced, because that is
    // the *correct* answer in clear weather. The `r3` witness failed on its
    // first run for exactly that reason: this project's recurring detector
    // error, a signal measured against a background that already contains it.
    // Forcing rain gives the band a finite value the sentinel cannot be
    // mistaken for, and makes `r8`/`r10` about drawn precipitation rather than
    // about a pass merely existing.
    if args.render_check && std::env::var_os("REWO_FORCE_WEATHER").is_none() {
        std::env::set_var("REWO_FORCE_WEATHER", "1.0");
    }
    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let pool = MeshPool::new(MeshTables {
        render: baked.render.clone(),
        models: baked.models.clone(),
    })?;
    let mut app = LiveApp {
        capture_pending: false,
        particles: None,
        session: Some(session),
        // Cloned before the bake is stored, because it is `take`n when the
        // window opens and the entity draws need this every frame after.
        equipment: std::sync::Arc::new(baked.equipment.clone()),
        trims: std::sync::Arc::new(baked.trims.clone()),
        lang: std::sync::Arc::new(baked.lang.clone()),
        baked: Some(baked),
        etypes,
        spears,
        chest_states,
        sign_states,
        bow_item,
        items,
        pool,
        weather: None,
        gui_items: None,
        screen: ScreenState::default(),
        stat_registries: std::sync::Arc::new(stat_registries),
        stats: None,
        hand: None,
        shift: false,
        ctrl: false,
        drag: DragState::default(),
        last_click: None,
        last_click_at: std::time::Instant::now(),
        screen_labels: Vec::new(),
        preview_skin: None,
        keys: Keys::default(),
        want_validation,
        run_seconds: match (args.run_seconds, args.render_check) {
            (Some(s), _) => Some(s),
            (None, true) => Some(RENDER_CHECK_SECONDS),
            (None, false) => None,
        },
        check: args.render_check.then(|| RenderCheck {
            validation: want_validation,
            ..RenderCheck::default()
        }),
        fif: args.fif,
        state: None,
        ring: OverlayRing::default(),
        cpu: StatsAccum::default(),
        started: Instant::now(),
        last_frame: None,
        tick_accum: 0.0,
        logged_spawn: false,
        uploaded_total: 0,
        flood_logged: false,
        hotbar_slot: 0,
        dirt_item,
        debug: true,
        // `Hud.isHidden` starts false — the HUD is showing (M70).
        hud_hidden: false,
        f3_down: false,
        f3_used_as_modifier: false,
        advanced_tooltips: false,
        container_injected: false,
        screen_forced_open: false,
        tool_highlight: rewo_gpu::hud::ToolHighlight::default(),
        locator_styles: Vec::new(),
        modules: crate::modules::Modules::load(),
        glyphs: load_velvet_fonts(),
        gestures: GestureTracker::default(),
        flicker: BlockLightFlicker::random(),
        gamma: args.gamma,
        darkness_option: args.darkness_effect_scale,
        skins: SkinLoader::new(),
        pack: args.pack.clone(),
        etf: rewo_data::etf::EtfPack::default(),
        death: None,
        view: ScreenView::None,
        server_links: rewo_net::server_links::ServerLinks::default(),
        exit_requested: false,
        init_error: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop run: {e}"))?;
    if let Some(e) = app.init_error.take() {
        return Err(e);
    }
    let elapsed = app.started.elapsed().as_secs_f32();
    // **The teardown is not session-gated and the summary is.**
    //
    // It used to be one `if let (Some(state), Some(session))`, which made
    // `Some(session)` a proxy for "the client is alive" — so a run that ended
    // on the disconnect screen never called `world_renderer.destroy`, and
    // `gpu_allocator` reported every one of its allocations as leaked on exit.
    // That is the second place M85 found the same implicit assumption (the
    // first was the `--run-seconds` deadline, which lived past the session
    // borrow in `frame`), and it is the answer to "does a screen with no
    // session break any framework assumption": twice, and both times the
    // assumption was the same one written two different ways.
    let mut state = app.state.take();
    if let (Some(state), Some(session)) = (state.as_mut(), app.session.take()) {
        println!(
            "[rewo-m3-live] windowed: {:.1}s, {} frames, avg fps {:.0}, frames-in-flight {}",
            elapsed,
            app.cpu.len(),
            app.cpu.len() as f32 / elapsed.max(0.001),
            state.renderer.frames_in_flight(),
        );
        println!(
            "[rewo-m3-live] frame time: avg {:.2}  p99 {:.2}  1% low {:.2}  0.1% low {:.2}  max {:.2} ms",
            app.cpu.average(),
            app.cpu.percentile(0.99),
            app.cpu.low_mean(0.01),
            app.cpu.low_mean(0.001),
            app.cpu.percentile(1.0),
        );
        println!(
            "[rewo-m3-live] final pos ({:.1},{:.1},{:.1}), corrections {}, columns {}",
            session.player.x,
            session.player.y,
            session.player.z,
            session.corrections,
            session.world.loaded_columns(),
        );
        println!(
            "[rewo-m3-live] mesh pool: {} column uploads over the session ({} still in flight)",
            app.uploaded_total,
            app.pool.in_flight(),
        );
        println!(
            "[rewo-m3-live] entities tracked at exit: {}",
            session.world.entities.len(),
        );
        let _ = EYE_HEIGHT;
    }
    // M85's gotcha 14: this block used to be joined to the session summary
    // above by one `if let (Some(state), Some(session))`, so a client that
    // outlived its session tore nothing down and `gpu_allocator` reported
    // everything leaked. The renderer's lifetime is not the session's.
    if let Some(mut state) = state {
        // Idle before tearing anything down. The last frames submitted are
        // still in flight when the loop exits, and several `destroy`s
        // (`text`, `hud`, `locator_bar`, `entities`, `velvet_*`, `overlay`)
        // do not idle for themselves. The headless path fences on its single
        // frame and so has never needed this; the windowed path had no
        // equivalent (M86).
        //
        // Recorded honestly: this did **not** move the VUID count on its own.
        // The ~40,000 destroy-while-in-use errors M86 fixed were all per-frame,
        // not teardown — this closes a real hole that simply was not the one
        // producing the noise.
        state.gpu.wait_idle();
        state.world_renderer.destroy(&mut state.gpu);
        state.renderer.destroy(&mut state.gpu);
    }
    // M86's gate. Reported after teardown so `r17` also covers the destroys —
    // the windowed path had no `device_wait_idle` there until this milestone.
    if let Some(c) = app.check.as_ref() {
        if !c.report() {
            return Err("render-check: one or more witnesses failed".into());
        }
    }
    Ok(())
}

fn client_jar_path(version: &str) -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

/// Build this frame's overlay text: an F3-style debug block (top-left, when
/// `debug`) and the last few chat messages (above the hotbar). GUI scale
/// `px`; `fps` is shown in the header when known (windowed only).
fn build_text(
    session: &PlaySession,
    px: f32,
    screen_h: f32,
    fps: Option<f32>,
    debug: bool,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_gpu::world::OwnedTextLine;
    let white = [0.93, 0.93, 0.93];
    let mut lines = Vec::new();
    // F3 debug block (top-left). Toggled by F3 in the windowed client;
    // always on in headless so a verification PNG shows the state.
    if debug {
        let p = &session.player;
        let (bx, by, bz) = (p.x.floor() as i64, p.y.floor() as i64, p.z.floor() as i64);
        // Chunk-relative block coords (0..15) — Rust's rem_euclid keeps them
        // non-negative in the negative hemisphere, matching vanilla's F3.
        let (rbx, rbz) = (bx.rem_euclid(16), bz.rem_euclid(16));
        let (cx, cz) = (bx.div_euclid(16), bz.div_euclid(16));
        let toward = facing_axis(p.yaw, p.pitch);
        let header = match fps {
            Some(f) => format!("Rewo 26.2   {f:.0} fps"),
            None => "Rewo 26.2".to_string(),
        };
        let f3 = [
            header,
            format!("XYZ: {:.3} / {:.3} / {:.3}", p.x, p.y, p.z),
            format!(
                "Block: {bx} {by} {bz}   [{rbx} {} {rbz}]",
                by.rem_euclid(16)
            ),
            format!("Chunk: {cx} {cz}"),
            {
                // Vanilla F3's "Client Light" — sampled at the eye, the same
                // cell entity lighting uses.
                let (bl, sl) =
                    session
                        .world
                        .light_at(bx as i32, p.eye_y().floor() as i32, bz as i32);
                format!("Light: {} ({sl} sky, {bl} block)", bl.max(sl))
            },
            format!(
                "Facing: {} {}  ({:.1} / {:.1})",
                compass(p.yaw),
                toward,
                p.yaw,
                p.pitch
            ),
            format!(
                "Loaded: {} chunks   Entities: {}   {}",
                session.world.loaded_columns(),
                session.world.entities.len(),
                if p.on_ground { "grounded" } else { "airborne" },
            ),
        ];
        for (i, text) in f3.into_iter().enumerate() {
            lines.push(OwnedTextLine {
                x: 3.0 * px,
                y: (3.0 + i as f32 * 10.0) * px,
                px,
                color: white,
                alpha: 1.0,
                shadow: true,
                text,
            });
        }
    }
    // Recent chat (bottom-left, above the hotbar; oldest higher, newest low).
    let chat = &session.chat_log;
    let show = 8.min(chat.len());
    let line_h = 10.0 * px; // 9px glyph + 1px gap
    let base_y = screen_h - 40.0 * px - line_h; // above the hotbar
    for (i, msg) in chat[chat.len() - show..].iter().enumerate() {
        // Trim overlong lines; a real client wraps.
        let text: String = msg.chars().take(80).collect();
        lines.push(OwnedTextLine {
            x: 3.0 * px,
            y: base_y - (show as f32 - 1.0 - i as f32) * line_h,
            px,
            color: white,
            alpha: 1.0,
            shadow: true,
            text,
        });
    }
    lines
}

/// `options.notificationDisplayTime` — the multiplier on the 40-tick timer.
///
/// Rewo has no options file, so this is vanilla's default (`1.0`, the middle
/// of an `IntRange(5, 100)` mapped through `v / 10.0`), which makes the label
/// hold for two seconds.
pub const NOTIFICATION_DISPLAY_TIME: f64 = 1.0;

/// `ItemStack.getHoverName()` for the selected hotbar stack, as the two fields
/// `Hud.tick`'s re-trigger compares (M66).
///
/// `None` is an empty hand, which zeroes the timer.
fn selected_item_label(
    session: &PlaySession,
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
) -> Option<(i32, String)> {
    let stack = session.inventory.held()?;
    let translated = items.name(stack.item_id).and_then(|n| names.get(n))?;
    let name = session
        .inventory
        .text_of(stack)
        .and_then(|t| t.name.clone())
        .unwrap_or_else(|| translated.clone());
    Some((stack.item_id, name))
}

/// `Hud.extractSelectedItemName` (M66) — the held-item label over the hotbar.
///
/// Vanilla's call site skips it entirely in spectator mode:
///
/// ```java
/// if (this.minecraft.gameMode.getPlayerMode() != GameType.SPECTATOR) {
///    this.extractSelectedItemName(graphics);
/// }
/// ```
///
/// **The ITALIC that `CUSTOM_NAME` adds is not rendered.** The HUD's text pass
/// carries one colour per line and has no italic face, so a renamed stack's
/// label shows its name and its rarity colour but stands upright. The colour
/// and the fade do carry, which is the visible bulk of it — and the same gap
/// M42 records for the bitmap tooltip fallback.
///
/// The backdrop (`textWithBackdrop`'s `fill`) is likewise absent, and there it
/// is not a divergence: `getBackgroundColor(0.0F)` is zero at vanilla's
/// defaults, so vanilla draws no fill either. See
/// [`rewo_gpu::hud::text_backdrop_rect`].
fn selected_item_name_line(
    session: &PlaySession,
    items: &rewo_data::items::Items,
    highlight: &rewo_gpu::hud::ToolHighlight,
    advance: &[u8; 256],
    px: f32,
    (screen_w, screen_h): (f32, f32),
) -> Option<rewo_gpu::world::OwnedTextLine> {
    if session
        .own_game_mode()
        .is_some_and(rewo_net::play::GameMode::is_spectator)
    {
        return None;
    }
    let (item_id, name) = highlight.showing()?;
    let alpha = rewo_gpu::hud::tool_highlight_alpha(highlight.timer);
    // `if (alpha > 0)` — the draw's own guard, separate from the timer's.
    if alpha <= 0 {
        return None;
    }
    let width = rewo_gpu::text::width(name, advance);
    // `canHurtPlayer()` is `localPlayerMode.isSurvival()`, and **that is
    // SURVIVAL || ADVENTURE**. A server that never sent the local player an
    // `UPDATE_GAME_MODE` leaves it unknown, and survival is the assumption the
    // rest of Rewo's HUD already makes — it draws hearts unconditionally.
    let can_hurt = session
        .own_game_mode()
        .map(rewo_net::play::GameMode::is_survival)
        .unwrap_or(true);
    let (gw, gh) = ((screen_w / px) as i32, (screen_h / px) as i32);
    let (x, y) = rewo_gpu::hud::selected_item_name_pos(gw, gh, width, can_hurt);
    // The rarity colour is read off the stack the label names. A stack that
    // changed since the last tick is a different label anyway, so the mismatch
    // is not reachable in practice; white is the fallback rather than the
    // wrong rarity's colour.
    let color = session
        .inventory
        .held()
        .filter(|s| s.item_id == item_id)
        .map(|s| {
            let text = session.inventory.text_of(s);
            rarity_color(stack_rarity(
                items.name(s.item_id),
                text.and_then(|t| t.rarity),
                text.is_some_and(|t| t.is_enchanted),
            ))
        })
        .unwrap_or([1.0, 1.0, 1.0]);
    Some(rewo_gpu::world::OwnedTextLine {
        x: x as f32 * px,
        y: y as f32 * px,
        px,
        color,
        alpha: alpha as f32 / 255.0,
        shadow: true,
        text: name.to_string(),
    })
}

/// M79's two HUD gauges, resolved from the session once per frame.
///
/// **The XP half is gated on `gameMode.hasExperience()`**, which is
/// `localPlayerMode.isSurvival()` — i.e. SURVIVAL *or* ADVENTURE, the same
/// two-value predicate M66's held-item label uses. Vanilla applies it in two
/// places with different consequences: `nextContextualInfoState` picks
/// `ContextualInfo.EMPTY` over `EXPERIENCE`, which removes the *bar*, and
/// `extractCommonHud` guards the level *number* separately (and additionally
/// on `experienceLevel > 0`, so level 0 shows no number even in survival).
/// An unknown mode falls back to survival for the same reason the hearts do.
///
/// **The cooldown half needs a group per slot**, and the group is
/// `getCooldownGroup(stack)`: the stack's `use_cooldown` override when it sets
/// one, the item's registry name otherwise. Both halves of that are here
/// because neither `rewo_net::hud_state` (which never sees a stack) nor
/// `rewo_gpu::hud` (which never sees the item table) can do it alone.
pub(crate) fn resolve_hud_gauges(
    hud: &rewo_net::hud_state::HudState,
    inventory: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    has_experience: bool,
    partial: f32,
) -> rewo_gpu::hud::HudGauges {
    let xp = &hud.experience;
    let mut cooldowns = [0.0f32; 9];
    for (i, slot) in cooldowns.iter_mut().enumerate() {
        let Some(stack) = inventory.hotbar(i) else {
            continue;
        };
        let Some(name) = items.name(stack.item_id) else {
            // An item id the table cannot name has no default group, and
            // guessing one would sweep an unrelated slot. Vanilla cannot
            // reach this: `BuiltInRegistries.ITEM.getKey` always answers.
            continue;
        };
        let group = inventory
            .text_of(stack)
            .and_then(|t| t.cooldown_group.as_deref())
            .unwrap_or(name);
        // `getCooldownPercent(item, getGameTimeDeltaPartialTick(true))`: the
        // frame's fraction into the tick, so the sweep slides rather than
        // stepping at 20 Hz.
        *slot = hud.cooldowns.percent(group, partial);
    }
    rewo_gpu::hud::HudGauges {
        experience: has_experience.then_some(xp.progress),
        xp_needed: xp.xp_needed_for_next_level(),
        cooldowns,
    }
}

pub(crate) fn locator_sprites(
    baked: &assets::BakedAssets,
) -> Option<rewo_gpu::locator_bar::LocatorSpritesData<'_>> {
    let l = baked.locator.as_ref()?;
    Some(rewo_gpu::locator_bar::LocatorSpritesData {
        background: hud_sprite(&l.background),
        arrow_up: hud_sprite(&l.arrow_up),
        arrow_down: hud_sprite(&l.arrow_down),
        dots: l.dots.iter().map(hud_sprite).collect(),
        styles: l
            .styles
            .iter()
            .map(|s| rewo_gpu::locator_bar::WaypointStyle {
                key: s.key.clone(),
                near_distance: s.near_distance,
                far_distance: s.far_distance,
                sprites: s.sprites.clone(),
            })
            .collect(),
    })
}

/// The net → gpu bridge for M83's locator bar, and the three things neither
/// side can do alone.
///
/// * **The identifier.** `rewo_gpu::locator_bar` never sees one, so the
///   `icon.color`-absent fallback (`setBrightness(color(255, hash), 0.9)`) and
///   the camera-entity skip are resolved here, where the store's keys are.
/// * **The style key → index.** The wire carries an `Identifier`; the atlas
///   carries a slot. An unknown key resolves to *no* style, which the pass
///   draws as the synthesised `MissingTextureAtlasSprite` patch — the same
///   answer `WaypointStyleManager.get`'s `getOrDefault(id, MISSING)` gives.
/// * **The entity substitution.** `Vec3iWaypoint.position` prefers the tracked
///   entity's interpolated eye position, which is a `level.getEntity(uuid)`
///   lookup — and `EntityTable` is keyed by entity *id*, so this is the O(n)
///   scan the table's only UUID map (profile names) cannot serve.
///
/// Returns `None` when the locator bar is not the contextual bar this frame.
///
/// **The observer is never in `EntityTable`** (REWO_PLAN §0.0 gotcha 13). Both
/// the camera position and the entity position come from `session.player`; a
/// version of this that reached for `entities.get(session.player_id)` would
/// find nothing and emit an empty bar on every frame, and a gate that built
/// its own table would never see it.
pub(crate) struct LocatorInputs<'a> {
    pub waypoints: &'a rewo_net::waypoints::WaypointStore,
    /// The **camera entity's** UUID. `session.own_uuid`, never a lookup in
    /// `entities` — see the doc above.
    pub own_uuid: Option<u128>,
    pub entities: &'a rewo_world::entities::EntityTable,
    pub styles: &'a [rewo_gpu::locator_bar::WaypointStyle],
    /// `camera.position()`.
    pub eye: [f64; 3],
    /// `cameraEntity.position()` — the feet.
    pub feet: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
    pub has_experience: bool,
    pub xp_prioritised: bool,
    pub ticks: u64,
}

/// The session adapter. Split from [`locator_bar_state`] so the gate drives
/// the emitter the frame drives rather than a copy of it — M45's
/// `install_shapes` failure and M41's rotted `swingshot` fixture were both
/// gates that had reimplemented a slice of the app and stopped testing their
/// subject. Same split M59 made for `resolve_health_bar`.
pub(crate) fn resolve_locator_bar(
    session: &PlaySession,
    entities: &rewo_world::entities::EntityTable,
    styles: &[rewo_gpu::locator_bar::WaypointStyle],
    fov_deg: f32,
    gui_w: i32,
    gui_h: i32,
    alpha: f32,
) -> Option<rewo_gpu::locator_bar::LocatorBarState> {
    let eye = player_eye(session);
    locator_bar_state(
        LocatorInputs {
            waypoints: &session.waypoints,
            own_uuid: session.own_uuid,
            entities,
            styles,
            eye: [eye.x as f64, eye.y as f64, eye.z as f64],
            feet: [session.player.x, session.player.y, session.player.z],
            yaw: session.player.yaw,
            pitch: session.player.pitch,
            fov: fov_deg,
            has_experience: has_experience(session),
            xp_prioritised: session.hud.experience.will_prioritize(),
            ticks: session.ticks,
        },
        gui_w,
        gui_h,
        alpha,
    )
}

pub(crate) fn locator_bar_state(
    input: LocatorInputs<'_>,
    gui_w: i32,
    gui_h: i32,
    alpha: f32,
) -> Option<rewo_gpu::locator_bar::LocatorBarState> {
    use rewo_gpu::locator_bar as lb;
    use rewo_net::waypoints::{WaypointContents, WaypointId};

    let LocatorInputs {
        waypoints,
        own_uuid,
        entities,
        styles,
        eye,
        feet,
        yaw,
        pitch,
        fov,
        has_experience,
        xp_prioritised,
        ticks,
    } = input;

    if !lb::contextual_bar(!waypoints.is_empty(), has_experience, xp_prioritised) {
        return None;
    }

    let cam = lb::LocatorCamera {
        yaw,
        pitch,
        fov,
        camera_pos: eye,
        // `cameraEntity.position()` — the **feet**, which is the wire position.
        entity_pos: feet,
        // Rewo's own projection is infinite-far reversed-Z, so there is no
        // `far` to read; vanilla's is finite. It only scales `z_ndc`, whose
        // sole consumer is a `> 1.0` test that reduces to "closer than the
        // near plane" for any `far >> near` — so a nominal value is exact to
        // the part in `near/far` that the test cannot resolve.
        near: 0.05,
        far: 1024.0,
    };

    let mut out = Vec::new();
    for w in waypoints.iter_sorted() {
        let subject = match w.contents {
            WaypointContents::Empty => lb::WaypointSubject::Empty,
            WaypointContents::Chunk { x, z } => lb::WaypointSubject::Chunk { x, z },
            WaypointContents::Azimuth { radians } => lb::WaypointSubject::Azimuth { radians },
            WaypointContents::Vec3i { x, y, z } => {
                let entity_eye = match w.id {
                    WaypointId::Uuid(uuid) => entities
                        .iter()
                        .find(|(_, e)| e.uuid == uuid)
                        .and_then(|(_, e)| {
                            let p = e.render_pos(alpha);
                            // `e.blockPosition().distManhattan(this.vector) > 3
                            //  ? null : e.getEyePosition(partialTick)` — a
                            // staleness guard, because the waypoint packet and
                            // the entity's own movement packets arrive
                            // independently.
                            let bx = p[0].floor() as i32;
                            let by = p[1].floor() as i32;
                            let bz = p[2].floor() as i32;
                            let manhattan =
                                (bx - x).abs() + (by - y).abs() + (bz - z).abs();
                            if manhattan > 3 {
                                return None;
                            }
                            // `EntityDimensions.scalable`'s default eye height
                            // is `height * 0.85`. A handful of types override
                            // it; the approximation is confined to the pitch
                            // arrow, because `yawAngleToCamera` reads only x
                            // and z — the bearing is a purely horizontal
                            // computation and the y component never enters it.
                            let h = entities
                                .attachments()
                                .and_then(|a| a.points(e.type_id))
                                .map(|p| p.height as f64)
                                .unwrap_or(1.8);
                            Some([p[0], p[1] + h * 0.85, p[2]])
                        }),
                    WaypointId::Name(_) => None,
                };
                lb::WaypointSubject::Vec3i {
                    x,
                    y,
                    z,
                    entity_eye,
                }
            }
        };
        // `icon.color.orElseGet(() -> id.map(uuid -> …, name -> …))` — the two
        // arms differ only in which `hashCode` they call, and both go through
        // `ARGB.color(255, hash)`, the **two-argument** overload that keeps the
        // hash's low 24 bits as RGB rather than treating it as three channels.
        let color = w.icon.color.unwrap_or_else(|| {
            let hash = match &w.id {
                WaypointId::Uuid(u) => lb::java_uuid_hash(*u),
                WaypointId::Name(n) => lb::java_string_hash(n),
            };
            lb::argb_set_brightness(0xFF00_0000 | (hash as u32 & 0x00FF_FFFF), 0.9)
        });
        out.push(lb::LocatorWaypoint {
            subject,
            color,
            // `usize::MAX` is "no style resolved", which the pass draws as the
            // missing patch.
            style: styles
                .iter()
                .position(|s| s.key == w.icon.style)
                .unwrap_or(usize::MAX),
            is_camera_entity: matches!(w.id, WaypointId::Uuid(u) if Some(u) == own_uuid),
        });
    }

    Some(lb::LocatorBarState {
        markers: lb::markers(&out, styles, &cam, gui_w, gui_h),
        tick: ticks as i64,
    })
}

/// `gameMode.hasExperience()` is `localPlayerMode.isSurvival()`, which is
/// **SURVIVAL or ADVENTURE**.
///
/// An unknown mode falls back to survival, the same assumption the hearts
/// already make (M66 records the reasoning at `selected_item_name_line`).
pub(crate) fn has_experience(session: &PlaySession) -> bool {
    session
        .own_game_mode()
        .map(rewo_net::play::GameMode::is_survival)
        .unwrap_or(true)
}

/// The XP level number — `ContextualBar.extractExperienceLevel` (M79).
///
/// Five draws: a black copy at each of ±1 on both axes, then the green one,
/// **all with `shadow = false`**. The outline is what makes the number legible
/// over the bar it straddles; a drop shadow on top of it would thicken the
/// glyphs instead of framing them.
///
/// The string is `Component.translatable("gui.experience.level", level)`,
/// which the vanilla language file renders as the bare number — so the
/// translated form is looked up and the number substituted, with the number
/// alone as the fallback.
pub(crate) fn experience_level_lines(
    xp: &rewo_net::hud_state::ExperienceState,
    has_experience: bool,
    lang: Option<&rewo_data::lang::Language>,
    advance: &[u8; 256],
    px: f32,
    (screen_w, screen_h): (f32, f32),
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    let level = xp.level;
    if !has_experience || level <= 0 {
        return Vec::new();
    }
    let number = level.to_string();
    let text = match lang.and_then(|l| l.get("gui.experience.level")) {
        // `%s` is the one substitution the vanilla key carries.
        Some(pattern) if pattern.contains("%s") => pattern.replacen("%s", &number, 1),
        _ => number,
    };
    let width = rewo_gpu::text::width(&text, advance);
    let (gw, gh) = ((screen_w / px) as i32, (screen_h / px) as i32);
    let (x, y) = rewo_gpu::hud::experience_level_pos(gw, gh, width);
    let mut out = Vec::with_capacity(5);
    let mut push = |dx: i32, dy: i32, color: u32| {
        out.push(rewo_gpu::world::OwnedTextLine {
            x: (x + dx) as f32 * px,
            y: (y + dy) as f32 * px,
            px,
            color: rewo_net::chat_style::rgb_f32(color & 0x00FF_FFFF),
            alpha: 1.0,
            shadow: false,
            text: text.clone(),
        });
    };
    // The four black copies first, in vanilla's own order, then the green.
    push(1, 0, rewo_gpu::hud::EXPERIENCE_LEVEL_OUTLINE);
    push(-1, 0, rewo_gpu::hud::EXPERIENCE_LEVEL_OUTLINE);
    push(0, 1, rewo_gpu::hud::EXPERIENCE_LEVEL_OUTLINE);
    push(0, -1, rewo_gpu::hud::EXPERIENCE_LEVEL_OUTLINE);
    push(0, 0, rewo_gpu::hud::EXPERIENCE_LEVEL_COLOR);
    out
}

/// The title, the subtitle and the action bar — `Hud.extractTitle` and
/// `Hud.extractOverlayMessage` (M79).
///
/// **One text line per styled span**, penned out with the font's advances:
/// vanilla's `graphics.text(font, Component, x, y, color, shadow)` passes the
/// faded colour as a *default* that a span's own `color` replaces, and
/// `Font.StringRenderOutput.getTextColor` keeps the **caller's alpha** when it
/// does:
///
/// ```java
/// if (textColor != null) {
///    int alpha = ARGB.alpha(this.color);
///    return ARGB.color(alpha, textColor.getValue());
/// }
/// ```
///
/// So `{"text":"GO","color":"red"}` is red *and* still fades. Taking the
/// span's colour whole — the natural reading of "the style wins" — would give
/// a title that snaps in and out at full opacity.
pub(crate) fn title_lines(
    t: &rewo_net::hud_state::TitleOverlay,
    advance: &[u8; 256],
    px: f32,
    (screen_w, screen_h): (f32, f32),
    partial: f32,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_net::chat_style::{self, ChatStyle};
    let (gw, gh) = ((screen_w / px) as i32, (screen_h / px) as i32);
    let mut out = Vec::new();
    // A run of spans laid end to end from a top-left in GUI pixels, at a
    // whole-number scale. `scale` multiplies the *font* pixel, which is why
    // the title is 4× and the subtitle 2× rather than being pre-scaled
    // strings.
    let mut run =
        |out: &mut Vec<rewo_gpu::world::OwnedTextLine>,
         line: &chat_style::ChatLine,
         x: i32,
         y: i32,
         scale: i32,
         alpha: f32| {
            let mut pen = x;
            for span in line {
                let w = rewo_gpu::text::width(&span.text, advance);
                if !span.text.is_empty() {
                    out.push(rewo_gpu::world::OwnedTextLine {
                        x: pen as f32 * px,
                        y: y as f32 * px,
                        px: px * scale as f32,
                        color: span.color,
                        alpha,
                        shadow: true,
                        text: span.text.clone(),
                    });
                }
                pen += w * scale;
            }
        };

    // `if (this.title != null && this.titleTime > 0)`.
    if let Some(title) = t.title.as_ref().filter(|_| t.title_time > 0) {
        let alpha = rewo_gpu::hud::title_alpha(t.title_time, t.fade_in, t.stay, t.fade_out, partial);
        // `if (alpha > 0)` — the draw's own guard, so a fully-faded frame
        // emits nothing rather than a transparent quad.
        if alpha > 0 {
            let a = alpha as f32 / 255.0;
            let line = chat_style::parse_component(title, ChatStyle::WHITE);
            let width = rewo_gpu::text::width(&chat_style::plain_text(&line), advance);
            let (x, y) = rewo_gpu::hud::title_pos(gw, gh, width);
            run(&mut out, &line, x, y, rewo_gpu::hud::TITLE_SCALE, a);
            // The subtitle is drawn *inside* the title's block, at the title's
            // alpha — it has no ramp of its own.
            if let Some(subtitle) = &t.subtitle {
                let line = chat_style::parse_component(subtitle, ChatStyle::WHITE);
                let width = rewo_gpu::text::width(&chat_style::plain_text(&line), advance);
                let (x, y) = rewo_gpu::hud::subtitle_pos(gw, gh, width);
                run(&mut out, &line, x, y, rewo_gpu::hud::SUBTITLE_SCALE, a);
            }
        }
    }

    // `if (this.overlayMessageString != null && this.overlayMessageTime > 0)`.
    // A separate block, not an `else` — an action bar and a title show at once.
    if let Some(message) = t
        .overlay_message
        .as_ref()
        .filter(|_| t.overlay_message_time > 0)
    {
        let alpha = rewo_gpu::hud::action_bar_alpha(t.overlay_message_time, partial);
        if alpha > 0 {
            let line = chat_style::parse_component(message, ChatStyle::WHITE);
            let width = rewo_gpu::text::width(&chat_style::plain_text(&line), advance);
            let (x, y) = rewo_gpu::hud::action_bar_pos(gw, gh, width);
            run(&mut out, &line, x, y, 1, alpha as f32 / 255.0);
        }
    }
    out
}

/// Auto GUI scale (vanilla: largest integer fitting a ~320×240 base).
fn gui_px(w: u32, h: u32) -> f32 {
    ((h as f32 / 240.0).min(w as f32 / 320.0))
        .floor()
        .clamp(1.0, 4.0)
}

/// Vanilla F3's "Towards …" axis hint — the dominant world axis of the look
/// direction (used alongside the compass name).
fn facing_axis(yaw_deg: f32, pitch_deg: f32) -> &'static str {
    let d = look_dir(yaw_deg, pitch_deg);
    if d[0].abs() > d[2].abs() {
        if d[0] > 0.0 {
            "(Towards +X)"
        } else {
            "(Towards -X)"
        }
    } else if d[2] > 0.0 {
        "(Towards +Z)"
    } else {
        "(Towards -Z)"
    }
}

/// Cardinal/intercardinal name for a yaw (MC: 0=south/+Z, 90=west/−X).
fn compass(yaw_deg: f32) -> &'static str {
    let a = yaw_deg.rem_euclid(360.0);
    match (a / 45.0).round() as i32 % 8 {
        0 => "S",
        1 => "SW",
        2 => "W",
        3 => "NW",
        4 => "N",
        5 => "NE",
        6 => "E",
        _ => "SE",
    }
}

/// Eye position in f64 (block-precise) — feet + the 1.62 eye height.
fn eye_f64(s: &PlaySession) -> [f64; 3] {
    [s.player.x, s.player.y + 1.62, s.player.z]
}

/// Look direction (unit) from MC-convention yaw/pitch degrees.
fn look_dir(yaw_deg: f32, pitch_deg: f32) -> [f64; 3] {
    let (yaw, pitch) = (yaw_deg.to_radians(), pitch_deg.to_radians());
    [
        (-yaw.sin() * pitch.cos()) as f64,
        (-pitch.sin()) as f64,
        (yaw.cos() * pitch.cos()) as f64,
    ]
}

/// Reach distance (creative). Survival is 3.0; the test server is creative.
const REACH: f64 = 4.5;

/// Face normal → MC face index (0 down, 1 up, 2 north −Z, 3 south +Z,
/// 4 west −X, 5 east +X).
fn face_index(n: [i32; 3]) -> u8 {
    match n {
        [0, -1, 0] => 0,
        [0, 1, 0] => 1,
        [0, 0, -1] => 2,
        [0, 0, 1] => 3,
        [-1, 0, 0] => 4,
        [1, 0, 0] => 5,
        _ => 1,
    }
}

/// winit → GLFW key code, for the keys the screen framework reads (M82).
///
/// `KeyEvent.key()` is a **GLFW** code and `rewo_world::screen` compares
/// against those integers directly, because GLFW is Minecraft's own key
/// namespace — the same reasoning `ewo_core::keybind` records for the
/// launcher's keybind registry. Only the keys `Screen.keyPressed` and
/// `InputWithModifiers.isSelection` look at are mapped; everything else
/// answers `None` and never reaches the screen.
///
/// The four arrow codes are here even though
/// [`rewo_world::screen::Screen::key_pressed`] leaves them inert, so that
/// implementing arrow navigation later is a change in one crate rather than
/// two.
fn glfw_key(key: PhysicalKey) -> Option<i32> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    Some(match code {
        KeyCode::Space => 32,
        KeyCode::Escape => 256,
        KeyCode::Enter => 257,
        KeyCode::Tab => 258,
        KeyCode::ArrowRight => 262,
        KeyCode::ArrowLeft => 263,
        KeyCode::ArrowDown => 264,
        KeyCode::ArrowUp => 265,
        KeyCode::NumpadEnter => 335,
        _ => return None,
    })
}

/// Number keys 1..9 → hotbar slot 0..8.
fn digit_key(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::Digit1 => 0,
        KeyCode::Digit2 => 1,
        KeyCode::Digit3 => 2,
        KeyCode::Digit4 => 3,
        KeyCode::Digit5 => 4,
        KeyCode::Digit6 => 5,
        KeyCode::Digit7 => 6,
        KeyCode::Digit8 => 7,
        KeyCode::Digit9 => 8,
        _ => return None,
    })
}

/// Per entity-type `(width, height, pushable)` for entity collision, indexed
/// by type id. Vanilla only lets living entities shove, so items/projectiles/
/// displays are excluded (see `EntityTypes::pushable`).
pub fn entity_push_table(types: &rewo_data::entity_types::EntityTypes) -> Vec<(f32, f32, bool)> {
    (0..types.len() as i32)
        .map(|id| {
            let (w, h) = types.dimensions(id);
            (w, h, types.pushable(id))
        })
        .collect()
}

/// Per-channel world-light RGB `0..1` for an entity, sampled at `(x, eye_y, z)`
/// through the shared camera `LightmapState` (M13).
///
/// Vanilla samples entity light at `BlockPos.containing(x, eyeY, z)` rather
/// than the feet, and that detail is load-bearing: the feet block is usually
/// the floor the entity stands *in* (light 0), which would render every mob
/// pitch black. Sampling at the eye and running the block/sky levels through
/// the exact same `rewo_world::lightmap::sample` the terrain shader mirrors
/// keeps a mob lit identically to the blocks around it.
/// Every block entity in the world that Rewo can draw, as render draws.
///
/// The block state at the position supplies the facing, the material and the
/// chest half — the block-entity payload carries none of them, because vanilla
/// reads them off `getBlockState()`. A block entity whose block is not a chest,
/// or whose state is not in the table, is simply not drawn: that is M25's
/// fail-closed registry showing through, and it is why an unimplemented type
/// renders nothing rather than a chest in the wrong place.
/// One group's transform slotted into an otherwise-inert part array.
///
/// Group 0 is never read by the emitter, so the array's identity default means
/// "nothing animates" and a model that wants one moving part fills exactly one
/// slot.
fn one_part(
    group: u8,
    xf: rewo_data::be_transform::Affine,
    pivot: [f32; 3],
) -> (
    [rewo_data::be_transform::Affine; rewo_gpu::entities::MAX_PARTS],
    [[f32; 3]; rewo_gpu::entities::MAX_PARTS],
) {
    let mut xs = [rewo_data::be_transform::IDENTITY; rewo_gpu::entities::MAX_PARTS];
    let mut ps = [[0.0f32; 3]; rewo_gpu::entities::MAX_PARTS];
    let g = group as usize;
    if g > 0 && g < rewo_gpu::entities::MAX_PARTS {
        xs[g] = xf;
        ps[g] = pivot;
    }
    (xs, ps)
}

/// The animated groups a skull model carries, if any (M29).
///
/// `SkullModelBase.setupAnim` **always runs**, so a piglin head's ears and a
/// dragon head's jaw are at their formula values even at `animationPos = 0` —
/// they are not "at rest until powered". Rewo drew the mesh's own `PartPose`
/// until M29, which left both piglin ears about 10 degrees off on every head
/// in the world and every dragon jaw shut when vanilla holds it 0.2 rad open.
///
/// The plain `SkullModel` types (skeleton, wither skeleton, zombie, creeper,
/// player) animate **nothing**: their `setupAnim` writes `head.yRot`/`xRot`
/// from state fields that a block skull never sets.
fn skull_parts(
    model: &str,
    animation: f32,
) -> (
    [rewo_data::be_transform::Affine; rewo_gpu::entities::MAX_PARTS],
    [[f32; 3]; rewo_gpu::entities::MAX_PARTS],
) {
    use rewo_data::be_transform as bt;
    use rewo_data::block_entity_models as bem;
    let mut xs = [bt::IDENTITY; rewo_gpu::entities::MAX_PARTS];
    let mut ps = [[0.0f32; 3]; rewo_gpu::entities::MAX_PARTS];
    match model {
        "rewo:be/piglin_head" => {
            let (l, r) = bt::piglin_ear_angles(animation);
            let li = bem::PIGLIN_LEFT_EAR_PART as usize;
            let ri = bem::PIGLIN_RIGHT_EAR_PART as usize;
            xs[li] = bt::mul(&bt::part_at_rest(bem::PIGLIN_LEFT_EAR_PIVOT), &bt::rot_z(l));
            ps[li] = bem::PIGLIN_LEFT_EAR_PIVOT;
            xs[ri] = bt::mul(&bt::part_at_rest(bem::PIGLIN_RIGHT_EAR_PIVOT), &bt::rot_z(r));
            ps[ri] = bem::PIGLIN_RIGHT_EAR_PIVOT;
        }
        "rewo:be/dragon_head" => {
            let i = bem::DRAGON_JAW_PART as usize;
            // The jaw's pose offset rides inside the head's 0.75 scale, so its
            // pivot is the scaled one the bake used.
            let pivot = bem::DRAGON_JAW_PIVOT;
            xs[i] = bt::mul(
                &bt::part_at_rest(pivot),
                &bt::rot_x(bt::dragon_jaw_angle(animation)),
            );
            ps[i] = pivot;
        }
        _ => {}
    }
    (xs, ps)
}

fn collect_block_entities(
    world: &rewo_world::World,
    chests: &rewo_data::chest_states::ChestStates,
    lightmap: &LightmapState,
    alpha: f32,
    game_time: i64,
    // The camera's right and up axes. An active conduit's EYE is a billboard
    // — the one input in this whole path that is a property of the VIEW rather
    // than of the block (M30).
    cam: ([f32; 3], [f32; 3]),
) -> Vec<OwnedBlockEntityDraw> {
    use rewo_data::chest_states::BlockEntityAnim;
    let mut out = Vec::new();
    for (pos, be) in world.block_entities.iter() {
        let state = world.block_state_at(pos.x, pos.y, pos.z);
        let Some(draw) = chests.draw_for(state) else {
            continue;
        };
        // A pot's wobble is a BLOCK-level rotation composed after its facing,
        // not a part animation — the whole pot rocks (M29). It turns about
        // `(0.5, 0, 0.5)`, the block's FLOOR centre, where the facing turns
        // about `(0.5, 0.5, 0.5)`: a pot rocks on its base like a real one.
        let be_transform = match world
            .block_entities
            .pot_wobble(*pos, game_time, alpha)
        {
            Some((style, progress)) if draw.anim == BlockEntityAnim::DecoratedPot => {
                let st = match style {
                    rewo_world::block_entities::PotWobble::Positive => {
                        rewo_data::be_transform::WobbleStyle::Positive
                    }
                    rewo_world::block_entities::PotWobble::Negative => {
                        rewo_data::be_transform::WobbleStyle::Negative
                    }
                };
                rewo_data::be_transform::mul(
                    &rewo_data::be_transform::pot_wobble(st, progress),
                    &draw.transform,
                )
            }
            _ => draw.transform,
        };

        // A skull's animation counter, which only runs while its block state
        // is POWERED, drives a piglin head's ears and a dragon head's jaw.
        let skull_anim = world.block_entities.skull_animation(*pos, alpha);

        // A decorated pot is five draws, not one: the base, plus a side plane
        // per face with its own sherd texture. They share the pot's own
        // transform and differ by the side pose, so each rides the emitter's
        // animated-group slot rather than needing a new draw path.
        // A banner is the woodwork, then the bare cloth, then the base colour
        // as a mask, then one draw per pattern layer — each the SAME flag
        // geometry with a different greyscale sprite and a different dye.
        if let BlockEntityAnim::Banner {
            base_color,
            standing,
        } = draw.anim
        {
            use rewo_data::block_entity_models as bem;
            let light = entity_light(
                world,
                pos.x as f64 + 0.5,
                pos.y as f64 + 0.5,
                pos.z as f64 + 0.5,
                lightmap,
            );
            let flag = if standing {
                bem::BANNER_STANDING_FLAG_MODEL
            } else {
                bem::BANNER_WALL_FLAG_MODEL
            };
            let suffix = if standing { "" } else { "_wall" };
            // The sway (M29). Every cloth draw — the bare flag and each
            // pattern layer — shares ONE phase, or the layers would drift
            // apart and the pattern would slide off the flag it is painted on.
            let phase = rewo_data::be_transform::banner_phase(
                pos.x, pos.y, pos.z, game_time, alpha,
            );
            let flag_pivot = if standing {
                bem::BANNER_STANDING_FLAG_PIVOT
            } else {
                bem::BANNER_WALL_FLAG_PIVOT
            };
            let (xs, ps) = one_part(
                bem::BANNER_FLAG_PART,
                rewo_data::be_transform::banner_flag_part(phase, flag_pivot),
                flag_pivot,
            );
            let mut layer = |model: String, tint: [f32; 3]| OwnedBlockEntityDraw {
                pos: [pos.x as f32, pos.y as f32, pos.z as f32],
                model,
                transform: be_transform,
                light,
                part_transforms: xs,
                part_pivots: ps,
                tint,
            };
            // The bare cloth, untinted — its own texture carries its colour.
            out.push(layer(flag.to_string(), [1.0; 3]));
            // `Sheets.BANNER_PATTERN_BASE` masked with the banner's own dye,
            // drawn before any pattern.
            out.push(layer(
                format!("rewo:be/banner_pattern/base{suffix}"),
                dye_linear(base_color as usize),
            ));
            // Up to sixteen — `submitPatterns` stops there regardless of how
            // many the tag carries.
            for (pattern, colour) in banner_layers(be).into_iter().take(16) {
                let Some(name) = bem::banner_pattern_model(&pattern) else {
                    // An unknown pattern is skipped rather than drawn as some
                    // other one; a wrong banner is worse than a plain one.
                    continue;
                };
                out.push(layer(format!("{name}{suffix}"), dye_linear(colour)));
            }
        }
        // An ACTIVE conduit replaces its dormant shell with four draws: a
        // tumbling cage, the wind shroud twice, and a camera-facing eye whose
        // pupil opens once the frame is complete (M30).
        if draw.model == rewo_data::block_entity_models::CONDUIT.0 {
            let c = world.block_entities.conduit(*pos);
            if c.shape.active() {
                let light = entity_light(
                    world,
                    pos.x as f64 + 0.5,
                    pos.y as f64 + 0.5,
                    pos.z as f64 + 0.5,
                    lightmap,
                );
                let t = c.anim_time(alpha);
                let mut piece = |model: &str, xf: rewo_data::be_transform::Affine| {
                    out.push(OwnedBlockEntityDraw {
                        pos: [pos.x as f32, pos.y as f32, pos.z as f32],
                        model: model.to_string(),
                        transform: xf,
                        light,
                        part_transforms: [rewo_data::be_transform::IDENTITY;
                            rewo_gpu::entities::MAX_PARTS],
                        part_pivots: [[0.0; 3]; rewo_gpu::entities::MAX_PARTS],
                        tint: [1.0; 3],
                    });
                };
                piece(
                    "rewo:be/conduit_cage",
                    rewo_data::be_transform::conduit_cage(c.rotation(alpha), t),
                );
                // Phase 1 uses the vertical texture; the other two the
                // horizontal one. Both copies of the shroud share it.
                let wind = if c.phase() == 1 {
                    "rewo:be/conduit_wind_vertical"
                } else {
                    "rewo:be/conduit_wind"
                };
                piece(wind, rewo_data::be_transform::conduit_wind(c.phase(), false));
                piece(wind, rewo_data::be_transform::conduit_wind(c.phase(), true));
                piece(
                    if c.shape.hunting() {
                        "rewo:be/conduit_eye_open"
                    } else {
                        "rewo:be/conduit_eye_closed"
                    },
                    rewo_data::be_transform::conduit_eye(t, cam.0, cam.1),
                );
                // The dormant shell is NOT drawn alongside — vanilla's two
                // branches are exclusive.
                continue;
            }
        }
        if draw.anim == BlockEntityAnim::DecoratedPot {
            let sherds = pot_sherds(be);
            for (i, item) in sherds.iter().enumerate() {
                out.push(OwnedBlockEntityDraw {
                    pos: [pos.x as f32, pos.y as f32, pos.z as f32],
                    model: rewo_data::block_entity_models::pot_side_model(item.as_deref()),
                    // The wobble rocks the WHOLE pot, sides included — a base
                    // that rocked while its sherds stayed put would come apart.
                    transform: be_transform,
                    light: entity_light(
                        world,
                        pos.x as f64 + 0.5,
                        pos.y as f64 + 0.5,
                        pos.z as f64 + 0.5,
                        lightmap,
                    ),
                    part_transforms: one_part(
                        rewo_data::block_entity_models::POT_SIDE_PART,
                        rewo_data::be_transform::pot_side(i),
                        [0.0; 3],
                    )
                    .0,
                    part_pivots: [[0.0; 3]; rewo_gpu::entities::MAX_PARTS],
                    tint: [1.0; 3],
                });
            }
        }
        let parts = match draw.anim {
            // The pot's BASE has no animated group; its four sides are the
            // separate draws pushed above, each with its own side pose.
            BlockEntityAnim::None
            | BlockEntityAnim::DecoratedPot
            | BlockEntityAnim::Banner { .. } => skull_parts(&draw.model, skull_anim),
            BlockEntityAnim::ChestLid(c) => one_part(
                rewo_data::block_entity_models::CHEST_LID_PART,
                rewo_data::be_transform::chest_lid_part(
                    chest_openness(world, chests, *pos, c, alpha),
                    rewo_data::block_entity_models::CHEST_LID_PIVOT,
                ),
                rewo_data::block_entity_models::CHEST_LID_PIVOT,
            ),
            BlockEntityAnim::ShulkerLid => one_part(
                rewo_data::block_entity_models::SHULKER_LID_PART,
                rewo_data::be_transform::shulker_lid_part(
                    world.block_entities.shulker(*pos).progress(alpha),
                ),
                rewo_data::block_entity_models::SHULKER_LID_PIVOT,
            ),
        };
        out.push(OwnedBlockEntityDraw {
            pos: [pos.x as f32, pos.y as f32, pos.z as f32],
            model: draw.model,
            transform: be_transform,
            // Lit from the block's own cell — a chest fills its block, so
            // there is no neighbour to sample the way a flat model would need.
            light: entity_light(
                world,
                pos.x as f64 + 0.5,
                pos.y as f64 + 0.5,
                pos.z as f64 + 0.5,
                lightmap,
            ),
            // Each animated group builds its own transform. Both clocks are
            // driven by `block_event` and both are animated client-side, but
            // they are not the same clock and not the same motion — see
            // `rewo_world::block_entities::ShulkerAnim`.
            // Each animated group builds its own transform. The clocks are
            // genuinely different — a chest lid converges, a shulker lid runs
            // a four-state machine, a banner hashes the world clock — which is
            // why this is a match rather than one shared animator.
            part_transforms: parts.0,
            part_pivots: parts.1,
            // Only a banner's pattern layers are tinted; every other model's
            // texture already carries its colour.
            tint: [1.0; 3],
        });
    }
    out
}

/// `ChestBlock.opennessCombiner` — the openness a chest renders with.
///
/// ```text
/// acceptDouble(a, b) -> max(a.getOpenNess(t), b.getOpenNess(t))
/// acceptSingle(a)    -> a.getOpenNess(t)
/// ```
///
/// The **max over the pair** is the whole reason `DoubleBlockCombiner` appears
/// in `ChestRenderer` at all — it is not how the half-models are chosen (the
/// block's own `type` does that), it is how both halves of a double chest open
/// together when only one of them received the event. M25 recorded the
/// combiner as the blocker for drawing halves; that was wrong, and this is
/// what it actually does.
///
/// `ChestBlock.getConnectedDirection`:
/// `type == LEFT ? facing.getClockWise() : facing.getCounterClockWise()`.
fn chest_openness(
    world: &rewo_world::World,
    chests: &rewo_data::chest_states::ChestStates,
    pos: rewo_world::block_entities::BlockEntityPos,
    chest: rewo_data::chest_states::ChestState,
    alpha: f32,
) -> f32 {
    use rewo_data::chest_states::ChestType;
    let mine = world.block_entities.lid(pos).openness(alpha);
    let dir = match chest.kind {
        ChestType::Single => return mine,
        ChestType::Left => clockwise(chest.facing),
        ChestType::Right => counter_clockwise(chest.facing),
    };
    let (dx, dz) = step(dir);
    let other = rewo_world::block_entities::BlockEntityPos {
        x: pos.x + dx,
        y: pos.y,
        z: pos.z + dz,
    };
    // Only pair with a block that really is the other half. A LEFT whose
    // neighbour is not a chest is a half-broken pair mid-update, and taking
    // its (absent) lid as 0 is the same answer `acceptNone` gives.
    let paired = chests
        .get(world.block_state_at(other.x, other.y, other.z))
        .is_some_and(|o| o.kind != ChestType::Single);
    if !paired {
        return mine;
    }
    mine.max(world.block_entities.lid(other).openness(alpha))
}

/// `Direction.getClockWise()` for the four horizontals.
fn clockwise(f: rewo_data::chest_states::ChestFacing) -> rewo_data::chest_states::ChestFacing {
    use rewo_data::chest_states::ChestFacing as F;
    match f {
        F::North => F::East,
        F::East => F::South,
        F::South => F::West,
        F::West => F::North,
    }
}

/// `Direction.getCounterClockWise()`.
fn counter_clockwise(
    f: rewo_data::chest_states::ChestFacing,
) -> rewo_data::chest_states::ChestFacing {
    use rewo_data::chest_states::ChestFacing as F;
    match f {
        F::North => F::West,
        F::West => F::South,
        F::South => F::East,
        F::East => F::North,
    }
}

/// `Direction`'s `(stepX, stepZ)` for the four horizontals.
fn step(f: rewo_data::chest_states::ChestFacing) -> (i32, i32) {
    use rewo_data::chest_states::ChestFacing as F;
    match f {
        F::North => (0, -1),
        F::South => (0, 1),
        F::West => (-1, 0),
        F::East => (1, 0),
    }
}

/// One rendered sign line, owning its text (M25e).
pub(crate) struct OwnedSignLine {
    pub transform: rewo_data::be_transform::Affine,
    pub text: String,
    /// Baseline origin in font px. `x` is `-width/2` plus, for an outline
    /// copy, its offset; the caller no longer re-centres.
    pub x: f32,
    pub y: f32,
    /// Depth along the transform's third axis, in font px — negative for the
    /// outline copies so they sit behind the glyphs (M27).
    pub z: f32,
    pub color: [f32; 3],
    pub light: [f32; 3],
}


/// Every end portal and gateway in the world, as geometry for the M32 pass.
///
/// They leave `collect_block_entities` entirely: their shader samples in
/// screen space from two textures, so there is nothing for the block-entity
/// emitter — which wants one texture and a UV — to do with them.
pub(crate) fn collect_end_portals(
    world: &rewo_world::World,
    portal_states: &std::collections::HashSet<u32>,
    gateway_states: &std::collections::HashSet<u32>,
) -> Vec<rewo_gpu::end_portal::PortalDraw> {
    let mut out = Vec::new();
    for (pos, _be) in world.block_entities.iter() {
        let state = world.block_state_at(pos.x, pos.y, pos.z);
        let is_portal = portal_states.contains(&state);
        if !is_portal && !gateway_states.contains(&state) {
            continue;
        }
        // The portal's slab transform is applied here rather than in the
        // shader, because the shader's vertex stage is position-only and
        // vanilla's `TRANSFORMATION` is a poseStack push.
        let xf = if is_portal {
            rewo_data::be_transform::end_portal()
        } else {
            rewo_data::be_transform::end_gateway()
        };
        let verts = rewo_data::block_entity_models::end_portal_positions(is_portal)
            .into_iter()
            .map(|p| {
                let v = [
                    xf[0][0] * p[0] + xf[0][1] * p[1] + xf[0][2] * p[2] + xf[0][3],
                    xf[1][0] * p[0] + xf[1][1] * p[1] + xf[1][2] * p[2] + xf[1][3],
                    xf[2][0] * p[0] + xf[2][1] * p[1] + xf[2][2] * p[2] + xf[2][3],
                ];
                rewo_gpu::end_portal::PortalVertex {
                    pos: [
                        pos.x as f32 + v[0],
                        pos.y as f32 + v[1],
                        pos.z as f32 + v[2],
                    ],
                }
            })
            .collect();
        out.push(rewo_gpu::end_portal::PortalDraw {
            verts,
            layers: if is_portal {
                rewo_gpu::end_portal::PORTAL_LAYERS
            } else {
                rewo_gpu::end_portal::GATEWAY_LAYERS
            },
        });
    }
    out
}

/// A spawner's display-entity id, from `SpawnData` (M31).
///
/// ```text
/// SpawnData.CODEC:  { "entity": CompoundTag, ... }
/// getOrCreateDisplayEntity: if (entityToSpawn.getString("id").isEmpty()) return null;
/// ```
///
/// So the id lives two levels down, and an **empty or absent** one means the
/// spawner has no display entity at all rather than a default — vanilla
/// returns null and draws nothing.
pub(crate) fn spawner_entity_id(be: &rewo_world::block_entities::BlockEntity) -> Option<String> {
    let id = be
        .data
        .get("SpawnData")?
        .get("entity")?
        .get("id")
        .and_then(rewo_proto::nbt::Nbt::as_str)?;
    (!id.is_empty()).then(|| id.to_string())
}

/// Every spawner's caged mob, as mounted entity draws (M31).
///
/// Separate from `collect_block_entities` because the result is an
/// `EntityDraw`, not a block-entity one: the mob rides the **entity** pass, so
/// it gets the same models, rigs and animations every other mob does. The only
/// difference is that its position comes from a mount matrix rather than from
/// the world.
pub(crate) fn collect_spawner_mobs<'a>(
    world: &rewo_world::World,
    etypes: &rewo_data::entity_types::EntityTypes,
    spawner_states: &std::collections::HashSet<u32>,
    lightmap: &LightmapState,
    alpha: f32,
) -> Vec<OwnedSpawnerMob> {
    let mut out = Vec::new();
    for (pos, be) in world.block_entities.iter() {
        if !spawner_states.contains(&world.block_state_at(pos.x, pos.y, pos.z)) {
            continue;
        }
        let Some(name) = spawner_entity_id(be) else {
            continue;
        };
        let Some(type_id) = etypes.id_of(&name) else {
            // An entity type this version does not register: draw nothing
            // rather than substitute a mob that is not in the cage.
            continue;
        };
        let (w, h) = etypes.dimensions(type_id);
        let spin = world.block_entities.spawner(*pos);
        out.push(OwnedSpawnerMob {
            pos: [pos.x as f32, pos.y as f32, pos.z as f32],
            kind: rewo_gpu::mobs::kind_for_entity_name(&name),
            width: w,
            height: h,
            mount: rewo_data::be_transform::spawner_mob(
                rewo_data::be_transform::spawner_spin_degrees(
                    spin.old_spin,
                    spin.spin,
                    alpha,
                ),
                rewo_data::be_transform::spawner_mob_scale(w, h),
            ),
            light: entity_light(
                world,
                pos.x as f64 + 0.5,
                pos.y as f64 + 0.5,
                pos.z as f64 + 0.5,
                lightmap,
            ),
        });
    }
    out.sort_by(|a, b| {
        a.pos[0]
            .total_cmp(&b.pos[0])
            .then(a.pos[1].total_cmp(&b.pos[1]))
            .then(a.pos[2].total_cmp(&b.pos[2]))
    });
    out
}


/// Turn a collected caged mob into an `EntityDraw`.
///
/// Everything except `pos`, `kind` and `mount` is the neutral pose: a spawner's
/// display entity is a *model*, not a simulated mob — vanilla loads it once and
/// never ticks it, so it does not walk, look around, swing or take damage.
pub(crate) fn spawner_mob_draw(m: &OwnedSpawnerMob) -> rewo_gpu::entities::EntityDraw<'_> {
    rewo_gpu::entities::EntityDraw {
        pos: m.pos,
        width: m.width,
        height: m.height,
        color: [1.0; 3],
        name: None,
        // M59: no health bar in a still — the gate renders its own.
        health: None,
        kind: m.kind,
        yaw: 0.0,
        death_time: 0.0,
        head_yaw: 0.0,
        pitch: 0.0,
        limb_swing: 0.0,
        limb_amount: 0.0,
        gesture: None,
        shell: false,
        events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
        allay_dance: None,
        attack: rewo_gpu::mobs::SwingPose::NONE,
        arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
        mob: Default::default(),
        hurt: false,
        held: [None, None],
        ground_item: None,
        armor: [None; 4],
        held_glint: [false; 2],
        ground_glint: false,
        ground_count: 0,
        ground_seed: 0,
        ground_age: None,
        bob_offset: 0.0,
        skin_uv: None,
        scale_mul: 1.0,
        mount: Some(m.mount),
        anim_id: 0.0,
        light: m.light,
        emissive: rewo_gpu::entities::EmissiveState::default(),
        variant: 0,
        dye: None,
        sheared: false,
        undercoat: false,
        fish_dye: None,
        cape: None,
    }
}

/// The in-flight pickup animations, as entity draws (M81).
///
/// Same shape as the spawner's caged mob and for the same reason: this is an
/// *item*, and Rewo's item geometry lives on the entity pass, so the animation
/// reuses the whole `emit_ground_item` path — bob, spin, per-copy jitter and
/// all — rather than growing a second item emitter. Vanilla splits them (a
/// particle group with a captured render state) only because its entity is
/// already deleted; Rewo's problem is the same and its answer is
/// [`rewo_world::pickup`] holding the appearance rather than the entity.
///
/// Everything except position, item and age is neutral: a collected stack does
/// not walk, look around or take damage.
pub(crate) fn collect_pickups<'a>(
    session: &PlaySession,
    item_names: &'a rewo_data::items::Items,
    lightmap: &LightmapState,
    alpha: f32,
    now: f32,
) -> Vec<rewo_gpu::entities::EntityDraw<'a>> {
    let mut out = Vec::new();
    for p in session.world.pickups.iter() {
        let Some((item, count, foil)) = p.stack else {
            // An experience orb or an arrow: vanilla adds the particle
            // regardless and renders that entity's own model, which Rewo does
            // not have. The record exists; nothing is drawn.
            continue;
        };
        let Some(name) = item_names.name(item) else {
            continue;
        };
        let pos = p.render_pos(alpha);
        let pos = [pos[0] as f32, pos[1] as f32, pos[2] as f32];
        let light = entity_light(
            &session.world,
            pos[0] as f64,
            pos[1] as f64,
            pos[2] as f64,
            lightmap,
        );
        out.push(rewo_gpu::entities::EntityDraw {
            pos,
            width: 0.25,
            height: 0.25,
            color: [1.0; 3],
            name: None,
            health: None,
            kind: rewo_gpu::entities::EntityModelKind::Capsule,
            yaw: 0.0,
            death_time: 0.0,
            head_yaw: 0.0,
            pitch: 0.0,
            limb_swing: 0.0,
            limb_amount: 0.0,
            gesture: None,
            shell: false,
            events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
            allay_dance: None,
            attack: rewo_gpu::mobs::SwingPose::NONE,
            arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
            mob: Default::default(),
            hurt: false,
            held: [None, None],
            ground_item: Some(name),
            armor: [None; 4],
            held_glint: [false; 2],
            ground_glint: foil,
            ground_count: count,
            ground_seed: item,
            // The captured `ageInTicks`, reconstructed from the animation's own
            // life counter: the flight is `life + alpha` ticks old, so capture
            // was that many ticks before now. No second clock, and it cannot
            // drift from the one `emit_ground_item` would otherwise use.
            ground_age: Some(now * 20.0 - (p.life as f32 + alpha)),
            bob_offset: bob_offset_for(p.entity_id),
            skin_uv: None,
            scale_mul: 1.0,
            mount: None,
            anim_id: 0.0,
            light,
            emissive: rewo_gpu::entities::EmissiveState::default(),
            variant: 0,
            dye: None,
            sheared: false,
            undercoat: false,
            fish_dye: None,
            cape: None,
        });
    }
    out
}

/// One spawner's caged mob.
pub(crate) struct OwnedSpawnerMob {
    pub pos: [f32; 3],
    pub kind: rewo_gpu::entities::EntityModelKind,
    pub width: f32,
    pub height: f32,
    pub mount: rewo_data::be_transform::Affine,
    pub light: [f32; 3],
}

/// A dye index as a linear-space tint.
///
/// `DyeColor.getTextureDiffuseColor()` is what dyes a banner layer — **not**
/// the `textColor` a sign uses. Two of the sixteen differ enough to be obvious
/// (red is 0xB02E26 here against 0xFF0000 there), so the two tables are kept
/// apart rather than shared.
pub(crate) fn dye_linear(i: usize) -> [f32; 3] {
    let c = rewo_data::block_entity_models::DYE_DIFFUSE_COLORS
        .get(i)
        .copied()
        .unwrap_or(0xFFFFFF);
    linear_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// A banner's pattern layers, as `(pattern id, dye index)`.
///
/// The tag is `patterns`, a list of `{pattern, color}` compounds written by
/// `BannerPatternLayers.CODEC`. `color` is a dye **name**, so it is resolved
/// through the same 16-entry order the block colours use.
pub(crate) fn banner_layers(
    be: &rewo_world::block_entities::BlockEntity,
) -> Vec<(String, usize)> {
    let Some(rewo_proto::nbt::Nbt::List(items)) = be.data.get("patterns") else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|it| {
            let pattern = it.get("pattern").and_then(rewo_proto::nbt::Nbt::as_str)?;
            let colour = it
                .get("color")
                .and_then(rewo_proto::nbt::Nbt::as_str)
                .and_then(|n| {
                    rewo_data::block_entity_models::DYE_COLORS
                        .iter()
                        .position(|d| *d == n)
                })
                .unwrap_or(0);
            Some((pattern.to_string(), colour))
        })
        .collect()
}

/// A decorated pot's four sherds, in `PotDecorations`' stored order —
/// **back, left, right, front**.
///
/// The tag is `sherds`, a list of item ids written by
/// `PotDecorations.CODEC`; an absent list, a short one, or a slot holding
/// `minecraft:brick` all mean the plain side, which is what
/// `getSideSprite` falls through to. Returning `None` for those rather than the
/// literal item keeps that fall-through in one place.
pub(crate) fn pot_sherds(be: &rewo_world::block_entities::BlockEntity) -> [Option<String>; 4] {
    let mut out: [Option<String>; 4] = Default::default();
    let Some(rewo_proto::nbt::Nbt::List(items)) = be.data.get("sherds") else {
        return out;
    };
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = items
            .get(i)
            .and_then(rewo_proto::nbt::Nbt::as_str)
            .map(str::to_string);
    }
    out
}

/// `Font.prepare8xTextOutline` — the eight offsets a glowing sign's outline is
/// drawn at, in font px.
///
/// `for (xo = -1; xo <= 1; xo++) for (yo = -1; yo <= 1; yo++) if (xo|yo != 0)`,
/// each scaled by the glyph's `getShadowOffset()`, which is 1 for the default
/// font. Eight copies, not four: the diagonals are what close the outline's
/// corners.
const OUTLINE_OFFSETS: [(f32, f32); 8] = [
    (-1.0, -1.0),
    (-1.0, 0.0),
    (-1.0, 1.0),
    (0.0, -1.0),
    (0.0, 1.0),
    (1.0, -1.0),
    (1.0, 0.0),
    (1.0, 1.0),
];

/// How far behind the glyphs an outline copy sits, in font px.
///
/// Vanilla keeps them coplanar and separates them by draw order under
/// `Font.DisplayMode.POLYGON_OFFSET`. Rewo's world text rides the entity
/// pass's ordinary depth-tested buffer, so the separation is a real one. A
/// font px is 1/96 of a block, so this is ~1/10 mm in world terms — far below
/// the depth buffer's resolution at any distance a sign is legible from, and
/// far above the coplanar z-fighting it prevents.
const OUTLINE_DEPTH: f32 = -0.01;

/// Every sign face in the world, as text draws.
///
/// The board itself is an ordinary block model and has been drawn since M2;
/// this is only the text. A sign whose state is not in the table, or whose
/// block entity carries no `front_text`, contributes nothing.
///
/// The line's x is `-font.width(line) / 2` — `AbstractSignRenderer` centres
/// each line independently, which is why a short line sits centred rather than
/// left-aligned under a long one.
pub(crate) fn collect_sign_text(
    world: &rewo_world::World,
    signs: &rewo_data::sign_states::SignStates,
    lightmap: &LightmapState,
    advance: &[u8; 256],
) -> Vec<OwnedSignLine> {
    use rewo_data::sign_text;
    let mut out = Vec::new();
    for (pos, be) in world.block_entities.iter() {
        let Some(sign) = signs.get(world.block_state_at(pos.x, pos.y, pos.z)) else {
            continue;
        };
        let (front, back) = be.sign_text();
        let light = entity_light(
            world,
            pos.x as f64 + 0.5,
            pos.y as f64 + 0.5,
            pos.z as f64 + 0.5,
            lightmap,
        );
        for (face, is_front) in [(front, true), (back, false)] {
            let Some(face) = face else { continue };
            if face.is_blank() {
                continue;
            }
            // `submitSignText`'s colour branch (M27). Unglowing text is the
            // dye at 40%; glowing text is the dye at *full* strength, lit
            // fullbright, with the 40% version demoted to its outline — glow
            // is not "the same colour, brighter".
            let dye = sign_text::dye_text_color(face.color.as_deref());
            // `state.drawOutline` is `isOutlineVisible`: within 16 blocks of
            // the camera. Rewo has no camera here (the collector runs before
            // the view is known), so it takes the near branch — which only
            // ever *adds* an outline, and glowing black outlines regardless.
            let style = sign_text::text_style(dye, face.glowing, true);
            let rgb = |c: u32| linear_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
            let color = rgb(style.color);
            let outline = style.outline.map(rgb);
            let light = if style.fullbright {
                // `15728880` — both light nibbles at 15. Glowing ink is
                // legible in an unlit room, which is the point of it.
                sample(15, 15, lightmap)
            } else {
                light
            };
            let base = sign.text_transform(is_front);
            // The block origin is folded in here rather than in the renderer,
            // so a sign's transform is the same shape as a block entity's.
            let m = rewo_data::be_transform::mul(
                &rewo_data::be_transform::translation(
                    pos.x as f32,
                    pos.y as f32,
                    pos.z as f32,
                ),
                &base,
            );
            for (i, line) in face.lines.iter().enumerate() {
                // `getRenderMessages` splits every line against the board and
                // keeps fragment 0 — a sign does not wrap onto the next row,
                // it truncates at a word boundary (M27).
                let line = sign_text::split_first(line, sign.max_line_width, advance);
                if line.is_empty() {
                    continue;
                }
                let y = sign.line_y(i as i32);
                // Each line is centred on its *own* width, which is why a
                // short line sits centred under a long one.
                let x = -sign_text::width(&line, advance) / 2.0;
                if let Some(outline) = outline {
                    for (dx, dy) in OUTLINE_OFFSETS {
                        out.push(OwnedSignLine {
                            transform: m,
                            x: x + dx,
                            y: y + dy,
                            z: OUTLINE_DEPTH,
                            text: line.clone(),
                            color: outline,
                            light,
                        });
                    }
                }
                out.push(OwnedSignLine {
                    transform: m,
                    x,
                    y,
                    z: 0.0,
                    text: line,
                    color,
                    light,
                });
            }
        }
    }
    // Deterministic order, so a headless render is reproducible. The outline
    // copies share a line's `y`, so `x` and `z` join the key — without them
    // eight identical-looking entries would sort arbitrarily against each
    // other and the vertex buffer would differ run to run.
    out.sort_by(|a, b| {
        a.transform[0][3]
            .total_cmp(&b.transform[0][3])
            .then(a.transform[2][3].total_cmp(&b.transform[2][3]))
            .then(a.y.total_cmp(&b.y))
            .then(a.x.total_cmp(&b.x))
            .then(a.z.total_cmp(&b.z))
    });
    out
}

/// A [`rewo_gpu::entities::BlockEntityDraw`] that owns its model name.
///
/// The half-models' names are built per frame (`…_left` / `…_right`), so they
/// cannot borrow from the state table the way the single models did.
pub(crate) struct OwnedBlockEntityDraw {
    pub pos: [f32; 3],
    pub model: String,
    pub transform: rewo_data::be_transform::Affine,
    pub light: [f32; 3],
    pub part_transforms: [rewo_data::be_transform::Affine; rewo_gpu::entities::MAX_PARTS],
    pub part_pivots: [[f32; 3]; rewo_gpu::entities::MAX_PARTS],
    /// A linear tint multiplied into the vertex colour — `[1, 1, 1]` for
    /// everything but a banner's dyed pattern layers (M28c).
    pub tint: [f32; 3],
}

impl OwnedBlockEntityDraw {
    pub fn as_draw(&self) -> rewo_gpu::entities::BlockEntityDraw<'_> {
        rewo_gpu::entities::BlockEntityDraw {
            pos: self.pos,
            model: &self.model,
            transform: self.transform,
            light: self.light,
            part_transforms: self.part_transforms,
            part_pivots: self.part_pivots,
            tint: self.tint,
        }
    }
}

fn entity_light(
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
fn daylight_of(day_ticks: Option<i64>) -> rewo_world::daylight::SkyLighting {
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
struct DimensionLight {
    ambient_color: [f32; 3],
    sky_light_color: [f32; 3],
    sky_light_factor: f32,
    /// `Timelines.OVERWORLD_DAY`'s tracks modify this dimension.
    day_timeline: bool,
}

impl DimensionLight {
    /// Before login (and on every serverless path) there is no dimension: the
    /// `EnvironmentAttributes` codec defaults, with the day timeline applying.
    /// This is exactly the pre-M16 behaviour.
    const UNRESOLVED: Self = Self {
        ambient_color: rewo_world::lightmap::DEFAULT_AMBIENT_COLOR,
        sky_light_color: [1.0, 1.0, 1.0],
        sky_light_factor: 1.0,
        day_timeline: true,
    };
}

fn dimension_light(def: Option<&DimensionTypeDef>) -> DimensionLight {
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
fn dimension_timeline(
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
fn sky_mode_of(def: Option<&DimensionTypeDef>) -> SkyMode {
    match def.map(|d| d.skybox) {
        Some(Skybox::None) => SkyMode::None,
        Some(Skybox::End) => SkyMode::End,
        Some(Skybox::Overworld) | None => SkyMode::Overworld,
    }
}

/// Resolve the full camera `LightmapState` for one frame (M13).
///
/// The three independent clocks fold into one uniform: the day/night timeline
/// (`day_ticks` → sky factor + colour), the block-light flicker (already
/// advanced into `block_factor`), and the mob-effect `snapshot` (night vision +
/// darkness). `gamma` / `darkness_option` are the player's validated options.
/// Pure — the single seam the tests pin.
fn resolve_lightmap(
    day_ticks: Option<i64>,
    dimension: Option<&DimensionTypeDef>,
    block_factor: f32,
    snapshot: rewo_net::effects::VisualEffectSnapshot,
    gamma: f32,
    darkness_option: f32,
    partial: f32,
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
        // The timeline tracks are multipliers over the dimension's base.
        sky_factor: dim.sky_light_factor * sky.light_factor,
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
fn linear_rgb_to_argb(c: [f32; 3]) -> i32 {
    let ch = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round() as i32) & 0xFF;
    (0xFFu32 as i32) << 24 | (ch(c[0]) << 16) | (ch(c[1]) << 8) | ch(c[2])
}

fn argb_to_linear_rgb(c: i32) -> [f32; 3] {
    let ch = |s: u32| ((c as u32 >> s) & 0xFF) as f32 / 255.0;
    [ch(16), ch(8), ch(0)]
}

fn to_world_lightmap(s: &LightmapState) -> WorldLightmapState {
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
fn apply_lightmap(
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
fn apply_biome_sky_fog(wr: &mut WorldRenderer, session: &PlaySession) {
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
fn argb_to_linear(argb: i32) -> [f32; 3] {
    let r = ((argb >> 16) & 0xFF) as f32 / 255.0;
    let g = ((argb >> 8) & 0xFF) as f32 / 255.0;
    let b = (argb & 0xFF) as f32 / 255.0;
    [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)]
}

fn init_celestial_if_present(
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

fn to_gpu_celestial(cel: &assets::CelestialTextures) -> CelestialTextures<'_> {
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
    }
}

/// Exact clear-weather Overworld celestial timeline. Before the first server
/// time packet, noon matches the existing `SkyLighting::DAY` fallback.
fn celestial_state_of(day_ticks: Option<i64>) -> CelestialState {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_net::effects::VisualEffectSnapshot;

    // -- M87: the container panel the screen path builds --------------------

    fn layout(id: i32) -> &'static rewo_world::menu_layout::MenuLayout {
        rewo_world::menu_layout::layout_of(id).unwrap()
    }

    #[test]
    fn the_players_own_menu_has_no_container_panel() {
        // It is drawn from the pass's own `inventory.png` rect, and returning
        // a panel here would send it through the container path instead --
        // which is the change `inventoryshot` would catch, but only because
        // this stays None.
        assert!(container_panel(&rewo_world::menu_layout::PLAYER, None).is_none());
    }

    #[test]
    fn a_lectern_paints_no_panel_rather_than_someone_elses() {
        // LecternScreen is a BookViewScreen. Falling through to a default
        // would paint some other menu's sheet behind a book.
        assert!(container_panel(layout(17), None).is_none());
    }

    #[test]
    fn every_other_menu_resolves_to_a_sheet_in_the_atlas() {
        for id in 0..25 {
            let l = layout(id);
            if id == 17 {
                continue;
            }
            let p = container_panel(l, None).unwrap_or_else(|| panic!("{} has no panel", l.name));
            assert!(
                p.sheet < rewo_data::assets::MENU_BACKGROUND_TEXTURES.len(),
                "{} indexes past the atlas",
                l.name
            );
        }
    }

    #[test]
    fn a_chest_is_two_blits_that_take_the_right_bands() {
        // generic_9x3: the top 3*18 + 17 = 71 px from the sheet's top, then
        // 96 px from v = 126. The gap between them is the rows a three-row
        // chest does not want.
        let p = container_panel(layout(2), None).unwrap();
        assert_eq!(p.blits.len(), 2);
        assert_eq!((p.gui_w, p.gui_h), (176.0, 168.0));
        assert_eq!((p.blits[0].dy, p.blits[0].sy, p.blits[0].h), (0.0, 0.0, 71.0));
        assert_eq!((p.blits[1].dy, p.blits[1].sy, p.blits[1].h), (71.0, 126.0, 96.0));
    }

    #[test]
    fn the_merchants_source_pixels_come_back_off_a_512_sheet() {
        // The conversion this function exists for. menu_screen normalises the
        // merchant against 512, so multiplying by 512 must return the pixels
        // vanilla blits -- 0, 0, 276 wide. Multiplying by 256 (the other
        // twenty-one screens' sheet) would halve them.
        let p = container_panel(layout(19), None).unwrap();
        assert_eq!(p.blits.len(), 1);
        assert_eq!((p.blits[0].sx, p.blits[0].sy), (0.0, 0.0));
        assert_eq!(p.blits[0].w, 276.0);
        assert_eq!(p.gui_w, 276.0);
    }

    #[test]
    fn every_screens_sheet_index_resolves() {
        // sheet_index returning None would mean the cross-check in rewo-world
        // had been removed; this is the same claim from the consuming side.
        for id in (0..25).filter(|&i| i != 17) {
            let s = rewo_world::menu_screen::screen_of(id).unwrap();
            assert!(sheet_index(s.texture).is_some(), "{}", s.texture);
        }
    }

    #[test]
    fn slot_rects_follow_the_menus_own_panel() {
        // A six-row chest is 176x222; measuring its slots from a 176x166
        // origin would put every one of them 28 px low. Same window, two
        // menus, and the difference is exactly half the height difference.
        let (w, h) = (1280.0f32, 720.0f32);
        let chest = rewo_world::inventory::Inventory::with_layout(layout(5));
        let player = rewo_world::inventory::Inventory::default();
        let (_, ctop, scale) =
            rewo_gpu::container::gui_origin_for(w, h, 176.0, chest.layout().image_h as f32);
        let (_, ptop, _) = rewo_gpu::container::gui_origin(w, h);
        assert!(ctop < ptop, "the taller panel starts higher");
        let cr = menu_slot_rects(&chest, w, h);
        let pr = menu_slot_rects(&player, w, h);
        assert_eq!(cr.len(), 90);
        assert_eq!(pr.len(), 46);
        // Slot 0 of each sits at its own layout's first position.
        assert_eq!(cr[0].1, ctop + 18.0 * scale, "chest grid starts at y=18");
        assert_eq!(pr[0].1, ptop + 28.0 * scale, "player's result slot at y=28");
    }

    #[test]
    fn stale_mesh_output_is_rejected_by_generation() {
        assert!(!mesh_output_is_stale(7, 7));
        assert!(mesh_output_is_stale(6, 7));
        assert!(mesh_output_is_stale(u64::MAX, 0));
    }

    #[test]
    fn resolve_allay_dance_gates_on_kind() {
        use rewo_world::entities::{EntityState, EntityTable};
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_dancing(1, true);
        t.tick_lerp();
        // The Allay kind resolves the live dance from the counters.
        assert!(resolve_allay_dance(EntityModelKind::Allay, &t, 1, 1.0).is_some());
        // A non-Allay kind is inert even though the entity carries a dance clock
        // — the kind gate must survive the extraction.
        assert!(resolve_allay_dance(EntityModelKind::Zombie, &t, 1, 1.0).is_none());
        // No dance entry at all → None regardless of kind.
        assert!(resolve_allay_dance(EntityModelKind::Allay, &t, 999, 1.0).is_none());
    }

    #[test]
    fn resolve_attack_anim_extracts_the_armed_render_state() {
        use rewo_data::swing_anim::{SwingAnimation, SwingAnimationType};
        use rewo_gpu::mobs::SwingKind;
        use rewo_world::entities::{
            EntityState, EntityTable, HandItem, HeldItem, InteractionHand,
        };
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        // Nothing has happened: the neutral pose, and never `None` — vanilla's
        // render state always carries these fields.
        let idle = resolve_attack_anim(&t, 1, 1.0);
        assert_eq!(idle.attack_time, 0.0);
        assert!(!idle.left_arm);
        assert_eq!(idle.kind, SwingKind::Whack, "bare hand = SwingAnimation.DEFAULT");
        assert_eq!(idle.age_scale, 1.0);
        // A spear in the off hand + an off-hand swing: the attack arm flips and
        // the type comes from the item held by *that arm*.
        t.set_hand_item(
            1,
            InteractionHand::OffHand,
            HandItem::Held(HeldItem {
                item_id: 1329,
                swing: SwingAnimation::new(SwingAnimationType::Stab, 19),
                use_profile: rewo_data::use_item::UseProfile::UNUSABLE,
                charged: false,
                glint: false,
            }),
        );
        t.swing(1, InteractionHand::OffHand, true);
        t.tick_lerp();
        t.tick_lerp();
        let a = resolve_attack_anim(&t, 1, 1.0);
        assert!(a.left_arm, "off-hand swing → the opposite of the RIGHT main arm");
        assert_eq!(a.kind, SwingKind::Stab);
        assert!((a.attack_time - 1.0 / 19.0).abs() < 1e-6, "{}", a.attack_time);
        // A baby's `getAgeScale()` halves the arm-pivot swing.
        t.set_baby(1, true);
        assert_eq!(resolve_attack_anim(&t, 1, 1.0).age_scale, 0.5);
        assert!(resolve_attack_anim(&t, 1, 1.0).inputs_known);
        // An unresolvable hand suppresses the whole pose rather than guessing.
        t.set_hand_item(1, InteractionHand::MainHand, HandItem::Unknown);
        let sup = resolve_attack_anim(&t, 1, 1.0);
        assert!(!sup.inputs_known);
        assert_eq!(sup.attack_time, 0.0, "suppressed, not guessed");
    }

    /// The three built-in dimension types, with exactly the fields M16's light
    /// resolution reads, transcribed from
    /// `%APPDATA%/EwoClient/rewo/26.2/decompiled/data/minecraft/dimension_type/`.
    /// Everything else comes from `unresolved_holder` (Overworld-shaped), so
    /// each fixture states precisely what it depends on.
    fn dim(
        name: &str,
        has_fixed_time: bool,
        skybox: Skybox,
        ambient: u32,
        sky_light_color: u32,
        sky_light_factor: f32,
    ) -> DimensionTypeDef {
        DimensionTypeDef {
            name: name.into(),
            has_fixed_time,
            has_day_timeline: !has_fixed_time,
            skybox,
            ambient_light_color: ambient as i32,
            sky_light_color: sky_light_color as i32,
            sky_light_factor,
            ..DimensionTypeDef::unresolved_holder(0)
        }
    }

    /// `overworld.json`: no `has_fixed_time`, no `skybox` (→ codec default
    /// OVERWORLD), ambient `#0a0a0a`, and NO `sky_light_color` /
    /// `sky_light_factor` attribute — so both take the codec defaults.
    fn overworld() -> DimensionTypeDef {
        dim(
            "minecraft:overworld",
            false,
            Skybox::Overworld,
            0xFF0A_0A0A,
            0xFFFF_FFFF,
            1.0,
        )
    }

    /// `the_nether.json`: `has_fixed_time: true`, `skybox: "none"`, ambient
    /// `#302821`, `sky_light_color: "#7a7aff"`, `sky_light_factor: 0.0`.
    fn nether() -> DimensionTypeDef {
        dim(
            "minecraft:the_nether",
            true,
            Skybox::None,
            0xFF30_2821,
            0xFF7A_7AFF,
            0.0,
        )
    }

    /// `the_end.json`: `has_fixed_time: true`, `skybox: "end"`, ambient
    /// `#3f473f`, `sky_light_color: "#ac60cd"`, `sky_light_factor: 0.0`.
    fn the_end() -> DimensionTypeDef {
        dim(
            "minecraft:the_end",
            true,
            Skybox::End,
            0xFF3F_473F,
            0xFFAC_60CD,
            0.0,
        )
    }

    /// A snapshot with no active effects and a given player tick count.
    fn no_effects(tick_count: i32) -> VisualEffectSnapshot {
        VisualEffectSnapshot {
            night_vision_duration: None,
            darkness_blend_factor: 0.0,
            tick_count,
        }
    }

    /// `ARGB.vector3fFromRGB24`, restated: a plain `/255` of the low 24 bits.
    fn rgb24(argb: i32) -> [f32; 3] {
        [
            ((argb >> 16) & 0xFF) as f32 / 255.0,
            ((argb >> 8) & 0xFF) as f32 / 255.0,
            (argb & 0xFF) as f32 / 255.0,
        ]
    }

    #[test]
    fn neutral_state_with_gamma_half() {
        // No day/night clock, resting flicker (1.4), no effects, gamma 0.5:
        // full daylight sky, gamma flows straight through to brightness, and
        // every effect term is off.
        let s = resolve_lightmap(None, None, 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(s.sky_factor, 1.0);
        assert_eq!(s.block_factor, 1.4);
        assert_eq!(s.sky_light_color, [1.0, 1.0, 1.0]);
        assert_eq!(s.brightness_factor, 0.5);
        assert_eq!(s.darkness_scale, 0.0);
        assert_eq!(s.night_vision_factor, 0.0);
    }

    #[test]
    fn night_vision_duration_drives_the_factor() {
        // Absent → 0.
        assert_eq!(
            resolve_lightmap(None, None, 1.4, no_effects(0), 0.5, 1.0, 0.0).night_vision_factor,
            0.0
        );
        // Infinite (`-1`) and > 200 ticks both pin to the full 1.0 seed.
        let inf = VisualEffectSnapshot {
            night_vision_duration: Some(-1),
            ..no_effects(0)
        };
        assert_eq!(
            resolve_lightmap(None, None, 1.4, inf, 0.5, 1.0, 0.0).night_vision_factor,
            1.0
        );
        let long = VisualEffectSnapshot {
            night_vision_duration: Some(400),
            ..no_effects(0)
        };
        assert_eq!(
            resolve_lightmap(None, None, 1.4, long, 0.5, 1.0, 0.0).night_vision_factor,
            1.0
        );
        // Within the last 200 ticks it pulses below 1.0 (the fade-out).
        let ending = VisualEffectSnapshot {
            night_vision_duration: Some(200),
            ..no_effects(0)
        };
        let nv = resolve_lightmap(None, None, 1.4, ending, 0.5, 1.0, 0.0).night_vision_factor;
        assert!(
            nv > 0.0 && nv < 1.0,
            "expected a pulsing NV factor, got {nv}"
        );
    }

    #[test]
    fn darkness_lowers_brightness_and_raises_scale() {
        // Partial darkness (blend 0.3) at the pulse peak (tick 0, cos = 1):
        // brightness drops below gamma, darkness scale goes positive.
        let partial_dark = VisualEffectSnapshot {
            darkness_blend_factor: 0.3,
            ..no_effects(0)
        };
        let s = resolve_lightmap(None, None, 1.4, partial_dark, 0.5, 1.0, 0.0);
        assert!(
            s.brightness_factor < 0.5,
            "brightness {} should dip below gamma",
            s.brightness_factor
        );
        assert!(
            s.brightness_factor > 0.0,
            "brightness {} should stay positive",
            s.brightness_factor
        );
        assert!(
            s.darkness_scale > 0.0,
            "darkness {} should be positive",
            s.darkness_scale
        );

        // Full darkness (blend 1.0): brightness floors to 0 (gamma - 1), and
        // the darkness subtraction maxes at 0.45 * option. This pins the
        // brightness-vs-darkness ordering: as darkness climbs, brightness sinks.
        let full_dark = VisualEffectSnapshot {
            darkness_blend_factor: 1.0,
            ..no_effects(0)
        };
        let s = resolve_lightmap(None, None, 1.4, full_dark, 0.5, 1.0, 0.0);
        assert_eq!(s.brightness_factor, 0.0);
        assert!(
            (s.darkness_scale - 0.45).abs() < 1e-4,
            "darkness {} ~ 0.45",
            s.darkness_scale
        );
        assert!(s.darkness_scale > s.brightness_factor);
    }

    #[test]
    fn day_ticks_drive_the_sky_half() {
        // Midnight (tick 18000) dims and blues the sky half, independent of
        // the block factor (a torch stays as bright).
        let s = resolve_lightmap(Some(18000), None, 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(s.sky_factor, 0.24);
        assert_eq!(s.sky_light_color, [0.48, 0.48, 1.0]);
        assert_eq!(s.block_factor, 1.4);
    }

    #[test]
    fn world_lightmap_conversion_is_field_for_field() {
        let s = resolve_lightmap(Some(18000), None, 1.7, no_effects(0), 0.5, 1.0, 0.0);
        let w = to_world_lightmap(&s);
        assert_eq!(w.sky_factor, s.sky_factor);
        assert_eq!(w.block_factor, s.block_factor);
        assert_eq!(w.sky_color, s.sky_light_color);
        assert_eq!(w.ambient_color, s.ambient_color);
        assert_eq!(w.brightness_factor, s.brightness_factor);
        assert_eq!(w.darkness_scale, s.darkness_scale);
        assert_eq!(w.night_vision_factor, s.night_vision_factor);
    }

    /// A transition-like sequence of consecutive resolves — Overworld → Nether
    /// → End → Overworld — with the world clock *still running* underneath, as
    /// it does across a real `respawn`. Every frame must depend only on the
    /// dimension it was given: no term may carry over from the previous one.
    ///
    /// This is the failure `resolve_lightmap` exists to make impossible. It is
    /// a pure function, so the proof is that each step equals the same step
    /// computed in isolation, and that returning to the Overworld reproduces
    /// the Overworld's own value bit-for-bit.
    #[test]
    fn dimension_transitions_leave_no_stale_term() {
        let (ow, ne, en) = (overworld(), nether(), the_end());
        // The clock keeps ticking across the transitions.
        let ticks = [1000i64, 18000, 6000, 23000];
        let seq = [Some(&ow), Some(&ne), Some(&en), Some(&ow)];

        let mut states = Vec::new();
        for (t, d) in ticks.iter().zip(seq) {
            states.push(resolve_lightmap(
                Some(*t),
                d,
                1.4,
                no_effects(0),
                0.5,
                1.0,
                0.0,
            ));
        }

        // 1. Each step matches the isolated computation for its own inputs.
        for (i, (t, d)) in ticks.iter().zip(seq).enumerate() {
            let isolated = resolve_lightmap(Some(*t), d, 1.4, no_effects(0), 0.5, 1.0, 0.0);
            assert_eq!(states[i], isolated, "step {i} depends on history");
        }

        // 2. The two non-Overworld steps are exactly their registry values,
        //    even though they were preceded by a lit Overworld frame.
        assert_eq!(states[1].sky_factor, 0.0);
        assert_eq!(states[1].ambient_color, rgb24(0xFF30_2821u32 as i32));
        assert_eq!(states[1].sky_light_color, rgb24(0xFF7A_7AFFu32 as i32));
        assert_eq!(states[2].sky_factor, 0.0);
        assert_eq!(states[2].ambient_color, rgb24(0xFF3F_473Fu32 as i32));
        assert_eq!(states[2].sky_light_color, rgb24(0xFFAC_60CDu32 as i32));

        // 2b. Reject the specific "the timeline got multiplied in anyway"
        //     failure by name: step 1 sits at midnight, where the Overworld
        //     tracks are (0.24, [0.48, 0.48, 1.0]). A leaked multiply would not
        //     be zero or default — it would be a plausible-looking third
        //     colour, which is exactly why it needs its own assertion.
        for (i, base) in [(1usize, 0xFF7A_7AFFu32 as i32)] {
            let leaked: [f32; 3] = std::array::from_fn(|c| rgb24(base)[c] * [0.48, 0.48, 1.0][c]);
            assert_ne!(
                states[i].sky_light_color, leaked,
                "step {i} leaked the clock"
            );
        }

        // 3. Back in the Overworld at tick 23000 the timeline drives the sky
        //    again — the End's factor-0 must not have stuck.
        let fresh = resolve_lightmap(Some(23000), Some(&ow), 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(states[3], fresh);
        assert!(
            states[3].sky_factor > 0.0,
            "the Overworld sky came back dead: {}",
            states[3].sky_factor
        );
        assert_eq!(states[3].ambient_color, rgb24(0xFF0A_0A0Au32 as i32));

        // 4. And the sky *mode* follows the same sequence, so a transition can
        //    never leave the previous world's skybox on screen.
        let modes: Vec<SkyMode> = seq.iter().map(|d| sky_mode_of(*d)).collect();
        assert_eq!(
            modes,
            vec![
                SkyMode::Overworld,
                SkyMode::None,
                SkyMode::End,
                SkyMode::Overworld
            ]
        );
    }

    /// No dimension resolved yet (pre-login, or any serverless path) must give
    /// exactly the pre-M16 inputs: attribute defaults, day timeline on.
    #[test]
    fn unresolved_dimension_is_the_pre_m16_default() {
        let d = dimension_light(None);
        assert_eq!(d, DimensionLight::UNRESOLVED);
        assert_eq!(d.ambient_color, [0.0, 0.0, 0.0]);
        assert_eq!(d.sky_light_color, [1.0, 1.0, 1.0]);
        assert_eq!(d.sky_light_factor, 1.0);
        assert!(d.day_timeline);
        assert_eq!(sky_mode_of(None), SkyMode::Overworld);
        // And the resolved lightmap is unchanged from the legacy call.
        let s = resolve_lightmap(Some(18000), None, 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(s.sky_factor, 0.24);
        assert_eq!(s.sky_light_color, [0.48, 0.48, 1.0]);
        assert_eq!(s.ambient_color, [0.0, 0.0, 0.0]);
    }

    /// The Overworld keeps the day timeline, and its ambient is the dimension
    /// attribute `#0a0a0a` — NOT the codec default black the serverless paths
    /// use. The two must be distinguishable, or the field is doing nothing.
    #[test]
    fn overworld_keeps_the_day_timeline_and_gains_its_ambient() {
        let ow = overworld();
        let d = dimension_light(Some(&ow));
        assert!(d.day_timeline);
        assert_eq!(d.ambient_color, [10.0 / 255.0; 3]);
        assert_eq!(sky_mode_of(Some(&ow)), SkyMode::Overworld);

        // Noon: the timeline multiplier is 1.0 over the 1.0 base.
        let noon = resolve_lightmap(Some(6000), Some(&ow), 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(noon.sky_factor, 1.0);
        assert_eq!(noon.sky_light_color, [1.0, 1.0, 1.0]);
        // Midnight: 1.0 * 0.24, white * (0.48, 0.48, 1.0) — the legacy values.
        let mid = resolve_lightmap(Some(18000), Some(&ow), 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(mid.sky_factor, 0.24);
        assert_eq!(mid.sky_light_color, [0.48, 0.48, 1.0]);
        // Ambient is constant across the cycle (no timeline track keyframes it).
        assert_eq!(noon.ambient_color, mid.ambient_color);
        assert_eq!(mid.ambient_color, [10.0 / 255.0; 3]);
        assert_ne!(mid.ambient_color, [0.0; 3]);
    }

    /// The Nether and the End are fixed-time: the Overworld day timeline must
    /// not touch them, so their exact `sky_light_factor` / `sky_light_color`
    /// attributes survive at any world-clock tick.
    #[test]
    fn fixed_time_dimensions_ignore_the_overworld_clock() {
        for (def, ambient, sky, mode) in [
            (
                nether(),
                [48.0 / 255.0, 40.0 / 255.0, 33.0 / 255.0],
                [122.0 / 255.0, 122.0 / 255.0, 1.0],
                SkyMode::None,
            ),
            (
                the_end(),
                [63.0 / 255.0, 71.0 / 255.0, 63.0 / 255.0],
                [172.0 / 255.0, 96.0 / 255.0, 205.0 / 255.0],
                SkyMode::End,
            ),
        ] {
            let d = dimension_light(Some(&def));
            assert!(!d.day_timeline, "{} must be fixed-time", def.name);
            assert_eq!(sky_mode_of(Some(&def)), mode, "{} skybox", def.name);

            // Every tick of the day, and with no clock at all, resolves the same.
            let mut seen = Vec::new();
            for t in [
                None,
                Some(0),
                Some(6000),
                Some(13000),
                Some(18000),
                Some(23999),
            ] {
                let s = resolve_lightmap(t, Some(&def), 1.4, no_effects(0), 0.5, 1.0, 0.0);
                assert_eq!(s.sky_factor, 0.0, "{} sky factor at {t:?}", def.name);
                assert_eq!(s.sky_light_color, sky, "{} sky colour at {t:?}", def.name);
                assert_eq!(s.ambient_color, ambient, "{} ambient at {t:?}", def.name);
                // The sky/fog gradient multiplier is gated on the same test, so
                // a midnight clock cannot black out the End's sky either.
                let tl = dimension_timeline(t, &d);
                assert_eq!(tl, rewo_world::daylight::SkyLighting::DAY);
                seen.push(s);
            }
            assert!(seen.windows(2).all(|w| w[0] == w[1]));
        }
    }

    /// The exact leak this guards: at midnight the Overworld resolves a dim,
    /// blue sky half and a black sky gradient. Respawning into the Nether with
    /// the SAME `day_ticks` must not carry any of that across.
    #[test]
    fn overworld_midnight_does_not_leak_across_a_respawn() {
        const MIDNIGHT: Option<i64> = Some(18000);
        let ow = overworld();
        let nether = nether();
        let before = resolve_lightmap(MIDNIGHT, Some(&ow), 1.4, no_effects(0), 0.5, 1.0, 0.0);
        let after = resolve_lightmap(MIDNIGHT, Some(&nether), 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(
            before.sky_light_color,
            [0.48, 0.48, 1.0],
            "the OW night tint"
        );
        // The Nether's own #7a7aff, not the night-multiplied version of it.
        assert_eq!(after.sky_light_color, [122.0 / 255.0, 122.0 / 255.0, 1.0]);
        assert_ne!(after.sky_light_color, before.sky_light_color);
        assert_eq!(after.sky_factor, 0.0);
        assert_ne!(after.ambient_color, before.ambient_color);

        // The gradient tint too: midnight blacks the Overworld sky, the Nether
        // (and End) must stay at the identity multiplier.
        let ow_tl = dimension_timeline(MIDNIGHT, &dimension_light(Some(&ow)));
        assert_eq!(ow_tl.sky_color, [0.0, 0.0, 0.0], "OW midnight sky is black");
        let nether_tl = dimension_timeline(MIDNIGHT, &dimension_light(Some(&nether)));
        assert_eq!(nether_tl.sky_color, [1.0, 1.0, 1.0]);
        assert_eq!(nether_tl.fog_color, [1.0, 1.0, 1.0]);
    }

    /// The ambient survives the CPU→GPU conversion, and the conversion carries
    /// every field (a missed field would default-initialize to something bland).
    #[test]
    fn world_lightmap_conversion_carries_the_ambient() {
        let end = the_end();
        let s = resolve_lightmap(Some(18000), Some(&end), 1.7, no_effects(0), 0.5, 1.0, 0.0);
        let w = to_world_lightmap(&s);
        assert_eq!(w.ambient_color, s.ambient_color);
        assert_eq!(w.ambient_color, [63.0 / 255.0, 71.0 / 255.0, 63.0 / 255.0]);
        assert_ne!(w.ambient_color, WorldLightmapState::default().ambient_color);
    }

    /// `DimensionType.Skybox` → the renderer's mode, both ways round.
    #[test]
    fn sky_mode_maps_every_skybox() {
        assert_eq!(sky_mode_of(Some(&overworld())), SkyMode::Overworld);
        assert_eq!(sky_mode_of(Some(&nether())), SkyMode::None);
        assert_eq!(sky_mode_of(Some(&the_end())), SkyMode::End);
        // An unresolved holder degrades to the codec default, not to NONE.
        let unresolved = DimensionTypeDef::unresolved_holder(9);
        assert_eq!(unresolved.skybox, Skybox::DEFAULT);
        assert_eq!(sky_mode_of(Some(&unresolved)), SkyMode::Overworld);
    }

    #[test]
    fn validate_unit_bounds() {
        assert!(validate_unit("gamma", 0.0).is_ok());
        assert!(validate_unit("gamma", 1.0).is_ok());
        assert!(validate_unit("gamma", 0.5).is_ok());
        assert!(validate_unit("gamma", -0.01).is_err());
        assert!(validate_unit("gamma", 1.01).is_err());
        assert!(validate_unit("gamma", f32::NAN).is_err());
        assert!(validate_unit("gamma", f32::INFINITY).is_err());
    }
}

// -- M33: weather and clouds --------------------------------------------------

/// Everything the live client needs to draw weather, built once.
///
/// The cloud mesh is **cached across frames**: it is position-independent (the
/// per-frame motion rides in the uniform), so vanilla rebuilds it only when the
/// camera crosses a cell boundary or changes which side of the deck it is on.
/// This mirrors `CloudRenderer`'s own `needsRebuild` / `prevCellX` bookkeeping.
pub struct WeatherAssets {
    clouds: Option<rewo_gpu::clouds::CloudTexture>,
    /// The three `Biome` world-gen noises, built once per client as vanilla
    /// builds them once per JVM.
    noise: rewo_world::weather::ClimateNoise,
    directions: rewo_gpu::weather::ColumnDirections,
    /// The cached mesh and the state it was built for.
    cached: Option<(Vec<[i32; 3]>, i32, i32, rewo_gpu::clouds::RelativeCameraPos)>,
    /// The eased rain-fog multiplier — state, because it ramps over ~5 ticks
    /// rather than following the rain level directly.
    rain_fog: rewo_world::weather::RainFog,
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
const WEATHER_RADIUS: i32 = 10;
/// `cloudRange` — vanilla derives it from the render distance; Rewo pins it
/// rather than plumbing a video-options struct for one number.
const CLOUD_RANGE_CHUNKS: i32 = 12;
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
const ENV_FOG_START: f32 = 0.0;
const ENV_FOG_END: f32 = 1024.0;

/// `FogCloudsEnd`, the distance at which the deck has faded out completely.
///
/// **An approximation, not a transcription.** Vanilla's comes from
/// `FogRenderer`, which Rewo does not have. It must comfortably exceed the
/// mesh's own reach or the deck is culled by its own fade — the furthest cell
/// is ~192 blocks out horizontally, and the deck can sit a couple of hundred
/// blocks overhead as well (192.33 above a y=-60 flat world is 250-odd). Set
/// too tight, clouds simply never appear; the first live shot did exactly that.
const CLOUD_FOG_END: f32 = 1024.0;

/// Build the two passes. Clouds need no texture (the shader carries its six
/// face colours inline); rain and snow need both of theirs, and a missing one
/// means that precipitation simply does not draw.
fn init_weather_if_present(
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
const LOCAL_RENDER_DISTANCE_CHUNKS: i32 = 12;

/// This frame's world-border wall (M80).
///
/// `renderDistance` is `getEffectiveRenderDistance() * 16` and `depthFar` is
/// `Camera.update`'s `max(renderDistance * 4, cloudRange * 16)` — the wall's
/// half-height is literally the camera's far plane, so it always spans the
/// view vertically.
fn apply_border(
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
fn forced_weather() -> Option<(f32, f32)> {
    let raw = std::env::var("REWO_FORCE_WEATHER").ok()?;
    let mut parts = raw.split(',');
    let rain: f32 = parts.next()?.trim().parse().ok()?;
    let thunder: f32 = parts.next().and_then(|t| t.trim().parse().ok()).unwrap_or(0.0);
    Some((rain.clamp(0.0, 1.0), thunder.clamp(0.0, 1.0)))
}

/// The weather state this frame draws — the session's, unless the headless
/// knob overrides it.
fn effective_weather(session: &PlaySession) -> rewo_world::weather::WeatherState {
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
    sys: rewo_world::particles::ParticleSystem,
    sprite_base: u32,
    /// Per kind: first layer and frame count, resolved once from the bake.
    sets: Vec<(rewo_world::particles::ParticleKind, u32, u32)>,
    /// Last session tick the system was advanced for, so the 20 Hz simulation
    /// steps exactly once per game tick however fast frames arrive.
    ///
    /// `None` until the first frame that runs it: the session has usually
    /// ticked for several seconds by then (connect, chunk load, settle), and
    /// anchoring at 0 would make the first frame fast-forward every one of
    /// those ticks at once — which ages a whole burst past its lifetime before
    /// it is ever drawn.
    last_tick: Option<u64>,
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

    fn layer_for(&self, kind: rewo_world::particles::ParticleKind, frame: u32) -> Option<u32> {
        self.sets
            .iter()
            .find(|(k, _, _)| *k == kind)
            .map(|(_, off, n)| off + frame.min(n.saturating_sub(1)))
    }
}

/// Build the combined texture array and hand it to the pass.
fn init_particles_if_present(
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
fn init_crumbling_if_present(
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
fn apply_crumbling(
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
fn apply_particles(
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

fn apply_weather(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    session: &PlaySession,
    w: &mut WeatherAssets,
    partial_ticks: f32,
    // Frame time in ticks, for the rain-fog ease. `None` means "converge
    // immediately" — the headless path draws a single frame after settling,
    // where an eased multiplier would still be near zero.
    delta_ticks: Option<f32>,
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
fn weather_attributes(
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
fn apply_weather_to_celestial(
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
fn rain_fog_band(
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

/// The GUI-item atlas: one row of slots, each large enough for any item
/// texture the bake produces.
///
/// Deliberately its own atlas rather than the entity pass's. That one is a
/// demand-filled pool sized for mob skins and shared with the held-item path;
/// borrowing it would couple the HUD to the entity pass's residency policy for
/// the sake of at most nine small textures.
const GUI_ATLAS_SLOT: u32 = 64;
const GUI_ATLAS_COLS: u32 = 8;
const GUI_ATLAS_ROWS: u32 = 8;
const GUI_ATLAS_W: u32 = GUI_ATLAS_SLOT * GUI_ATLAS_COLS;
const GUI_ATLAS_H: u32 = GUI_ATLAS_SLOT * GUI_ATLAS_ROWS;

/// A packed GUI atlas plus where each source texture landed.
pub struct GuiAtlas {
    pub rgba: Vec<u8>,
    /// Texture index -> `(u0, v0, du, dv)`.
    pub uv: std::collections::HashMap<u16, [f32; 4]>,
}

/// Pack every item texture the bake produced, up to the atlas's capacity.
///
/// Built once at startup rather than per frame: the item set is fixed by the
/// bake, and a hotbar swap must not cost an atlas upload. Textures past the
/// capacity are dropped with a log — their items then draw nothing, which is
/// the same "nothing rather than garbage" rule the rest of the item path uses.
pub fn pack_gui_atlas(items: &rewo_gpu::held::HeldItems, wanted: &[u16]) -> GuiAtlas {
    let mut rgba = vec![0u8; (GUI_ATLAS_W * GUI_ATLAS_H * 4) as usize];
    let mut uv = std::collections::HashMap::new();
    let cap = (GUI_ATLAS_COLS * GUI_ATLAS_ROWS) as usize;
    let mut dropped = 0usize;
    for (slot, &tex) in wanted.iter().enumerate() {
        if slot >= cap {
            dropped += 1;
            continue;
        }
        let Some(src) = items.textures.get(tex as usize) else {
            continue;
        };
        if src.w > GUI_ATLAS_SLOT || src.h > GUI_ATLAS_SLOT {
            dropped += 1;
            continue;
        }
        let (ox, oy) = (
            (slot as u32 % GUI_ATLAS_COLS) * GUI_ATLAS_SLOT,
            (slot as u32 / GUI_ATLAS_COLS) * GUI_ATLAS_SLOT,
        );
        for y in 0..src.h {
            let s = (y * src.w * 4) as usize;
            let d = (((oy + y) * GUI_ATLAS_W + ox) * 4) as usize;
            let n = (src.w * 4) as usize;
            rgba[d..d + n].copy_from_slice(&src.rgba[s..s + n]);
        }
        uv.insert(
            tex,
            [
                ox as f32 / GUI_ATLAS_W as f32,
                oy as f32 / GUI_ATLAS_H as f32,
                src.w as f32 / GUI_ATLAS_W as f32,
                src.h as f32 / GUI_ATLAS_H as f32,
            ],
        );
    }
    if dropped > 0 {
        log::warn!("live: {dropped} item textures did not fit the GUI atlas — those icons will not draw");
    }
    GuiAtlas { rgba, uv }
}

/// Every texture index the hotbar could need, in a stable order.
///
/// The whole baked item set is far larger than the atlas, so this takes the
/// textures of the items the *player actually has*, which is at most nine
/// models' worth.
pub fn gui_atlas_wanted(
    items: &rewo_gpu::held::HeldItems,
    models: &[String],
) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    for name in models {
        if let Some(m) = items.any(name) {
            for q in &m.quads {
                if !out.contains(&q.tex) {
                    out.push(q.tex);
                }
            }
        }
    }
    out
}

/// This frame's hotbar, as model names per slot (`None` for an empty slot).
pub fn hotbar_models(
    inv: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    trim_materials: &[rewo_net::trim_parse::TrimMaterialDef],
) -> [Option<String>; 9] {
    std::array::from_fn(|i| {
        let s = inv.hotbar(i)?;
        let base = items.name(s.item_id)?;
        // M49: the same composed name the screen slots use, so a trimmed piece
        // wears its trim in the hotbar too.
        Some(match s.trim_material.and_then(|m| trim_materials.get(m as usize)) {
            Some(m) => format!("{base}#{}", m.id),
            None => base.to_string(),
        })
    })
}

/// The inventory preview's cape, from its slot in the **preview** pass's own
/// atlas (M64).
///
/// Extracted so `capeshot` grades the decision the client actually makes
/// rather than a restatement of it — M45's and M41's gates both quietly
/// stopped testing their subject by reimplementing a slice of the app.
///
/// All three angles are zero, which is not a simplification of vanilla so
/// much as the same consequence as the preview's still legs:
/// `capeFlap`/`capeLean`/`capeLean2` are driven entirely by the gap between
/// the player and their lagging cloak anchor, and a player standing in an
/// open inventory has let that gap close. What is genuinely missing is the
/// *moving* case, for the reason the limbs are missing — nothing in Rewo
/// consumes the local player's animation state.
///
/// `chest_humanoid` is false because the preview draws no armour at all
/// (`armor: [None; 4]`): nothing there can be wearing a chestplate to shift
/// the cape clear, and by the same token nothing can be wearing an elytra to
/// suppress it — so `CapeLayer`'s other two gates have nothing to act on.
pub(crate) fn preview_cape(origin: Option<(u32, u32)>) -> Option<rewo_gpu::entities::CapeDraw> {
    origin.map(|origin| rewo_gpu::entities::CapeDraw {
        origin,
        flap: 0.0,
        lean: 0.0,
        lean2: 0.0,
        chest_humanoid: false,
        wavy: None,
    })
}

/// The player model shown in the inventory screen's window (M36).
///
/// `extractEntityInInventoryFollowsMouse` poses it from the cursor: the body
/// turns `180 + xAngle`, the head turns `xAngle` on top of that, and the pitch
/// is `-yAngle`. The 180 is why the model faces you at rest — the same
/// convention every other entity in Rewo uses, where yaw 0 faces +Z.
///
/// It stands still: no limb swing, no gesture, no hurt flash. Vanilla poses it
/// from the live player's render state, so a walking player's legs move in the
/// preview too; that would need the local player's animation state, which
/// nothing else in Rewo consumes yet.
///
/// **The cape (M64)** hangs from `cape_origin`, an address in the preview
/// pass's own atlas — see [`preview_cape`] for why its three angles are zero.
fn preview_draw<'a>(
    session: &PlaySession,
    skin: Option<[f32; 2]>,
    slim: bool,
    cape_origin: Option<(u32, u32)>,
    held: [Option<&'a str>; 2],
    w: f32,
    h: f32,
    mouse: (f64, f64),
) -> (EntityDraw<'a>, [[f32; 4]; 4], ash::vk::Rect2D) {
    let (x_angle, y_angle) = rewo_gpu::container::preview_angles(mouse, w, h);
    // `LivingEntityRenderState.boundingBoxHeight / scale`, and the player's
    // scale is 1 — so this is the standing hitbox, which is what centres the
    // model in its window.
    const PLAYER_HEIGHT: f32 = 1.8;
    let draw = EntityDraw {
        pos: [0.0, 0.0, 0.0],
        width: 0.6,
        height: PLAYER_HEIGHT,
        color: [1.0, 1.0, 1.0],
        name: None,
        // M59: no health bar in a still — the gate renders its own.
        health: None,
        kind: if slim {
            EntityModelKind::PlayerSlim
        } else {
            EntityModelKind::Player
        },
        yaw: 180.0 + x_angle,
        death_time: 0.0,
        ground_item: None,
        armor: [None; 4],
        held_glint: [false; 2],
        ground_glint: false,
        ground_count: 0,
        bob_offset: 0.0,
        ground_seed: 0,
        ground_age: None,
        head_yaw: 180.0 + x_angle,
        pitch: -y_angle,
        limb_swing: 0.0,
        limb_amount: 0.0,
        gesture: None,
        events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
        shell: false,
        allay_dance: None,
        attack: rewo_gpu::mobs::SwingPose::NONE,
        mob: rewo_gpu::mobs::MobCombat::default(),
        hurt: false,
        held,
        arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
        skin_uv: skin,
        scale_mul: 1.0,
        mount: None,
        anim_id: 0.0,
        // `GuiEntityRenderer` sets `renderState.lightCoords = 15728880`, which
        // is both light channels at full — the preview is lit by the GUI's own
        // two-light rig, not by wherever the player happens to be standing.
        light: [1.0, 1.0, 1.0],
        // M52: no emissive state, no pack variant, no dye — the
        // vanilla defaults, which is what this gate renders.
        emissive: rewo_gpu::entities::EmissiveState::default(),
        variant: 0,
        dye: None,
        sheared: false,
        undercoat: false,
        fish_dye: None,
        cape: preview_cape(cape_origin),
    };
    let vp = rewo_gpu::container::preview_view_proj(w, h, PLAYER_HEIGHT, y_angle);
    let (rx, ry, rw, rh) = rewo_gpu::container::preview_rect(w, h);
    let rect = ash::vk::Rect2D {
        offset: ash::vk::Offset2D {
            x: rx as i32,
            y: ry as i32,
        },
        extent: ash::vk::Extent2D {
            width: rw as u32,
            height: rh as u32,
        },
    };
    let _ = session;
    (draw, vp, rect)
}

// -- the first-person hand (M38) ----------------------------------------------

/// The hand's atlas: the same 64-px grid the GUI icons use, with the player's
/// 64×64 skin parked in the bottom-left quadrant.
///
/// One texture rather than two draws, because the arm and the item are one
/// pass — and one pass because they share a matrix chain up to the point where
/// they diverge.
const HAND_ATLAS: u32 = 512;
const HAND_SKIN_X: u32 = 0;
const HAND_SKIN_Y: u32 = HAND_ATLAS - 64;

/// Everything the hand needs across frames.
pub struct HandState {
    /// The item textures resident in the atlas, and where each landed.
    resident: Vec<u16>,
    /// Whether the resident atlas was packed with the current skin. A skin
    /// that arrives after the first pack must force one, or the arm keeps
    /// sampling the default.
    skin_resident: bool,
    uv: std::collections::HashMap<u16, [f32; 4]>,
    /// The two equip clocks — `mainHandHeight` and `offHandHeight`.
    main_equip: rewo_gpu::hand::EquipHeight,
    off_equip: rewo_gpu::hand::EquipHeight,
    /// `LocalPlayer.xBob` / `yBob`.
    bob: rewo_gpu::hand::ViewBob,
    /// The skin in the hand atlas, and whether the model is slim.
    ///
    /// Seeded with the jar's own default so an empty hand shows an arm from
    /// the first frame — which is also what vanilla shows on an offline server,
    /// where no player carries a `textures` property. A real skin replaces it
    /// when one arrives.
    skin: Option<(Vec<u8>, bool)>,
    held: rewo_gpu::held::HeldItems,
    /// The tick the clocks were last advanced on, so they step once per client
    /// tick rather than once per frame.
    last_tick: u64,
    /// A swing frozen for a headless shot (`REWO_HAND_SWING`). `None` in a
    /// real session, where the clock is the entity table's.
    forced_attack: Option<f32>,
    /// When this session started, for the glint's wall-clock phase (M44).
    started: std::time::Instant,
    /// `misc/enchanted_glint_item.png` as `(rgba, w, h)`; `None` draws none.
    glint: Option<(Vec<u8>, u32, u32)>,
}

impl HandState {
    pub fn new(baked: &assets::BakedAssets) -> Self {
        Self {
            resident: Vec::new(),
            skin_resident: false,
            uv: std::collections::HashMap::new(),
            main_equip: Default::default(),
            off_equip: Default::default(),
            bob: Default::default(),
            // `entity/player/wide/steve.png`, the 64x64 the bake already
            // carries for the entity pass's default player.
            skin: baked
                .mob_textures
                .iter()
                .find(|t| t.key == "player")
                .map(|t| (t.rgba.clone(), false)),
            held: to_gpu_held_items(&baked.held_items),
            last_tick: 0,
            forced_attack: None,
            started: std::time::Instant::now(),
            glint: baked
                .glint
                .as_ref()
                .map(|i| (i.rgba.clone(), i.w, i.h)),
        }
    }

    /// Advance the two equip clocks and the view bob, once per client tick.
    ///
    /// Separate from the per-frame build because they are *tick* clocks:
    /// running them per frame would make the equip dip three frames long
    /// rather than three ticks, so it would vanish at any sane frame rate.
    pub fn tick(&mut self, session: &PlaySession, items: &rewo_data::items::Items) {
        let now = session.ticks;
        if now == self.last_tick {
            return;
        }
        self.last_tick = now;
        self.step(session, items);
    }

    /// Advance the clocks once, unconditionally.
    ///
    /// Separate from [`Self::tick`] because that one dedupes on the session's
    /// tick counter — which is right per frame and wrong for a headless shot,
    /// where the counter does not move and the equip clock would stay at the
    /// bottom with the item off screen.
    fn step(&mut self, session: &PlaySession, items: &rewo_data::items::Items) {
        let id = |s: Option<rewo_world::inventory::ItemSlot>| {
            s.and_then(|s| items.name(s.item_id).map(|_| s.item_id))
        };
        self.main_equip.tick(id(session.inventory.held()));
        self.off_equip.tick(id(session.inventory.offhand()));
        self.bob.tick(session.player.pitch, session.player.yaw);
    }

    /// Run the clocks to rest — the equip dip fully raised — for a shot.
    pub fn settle(&mut self, session: &PlaySession, items: &rewo_data::items::Items) {
        for _ in 0..8 {
            self.step(session, items);
        }
    }
}

/// The hand's own projection.
///
/// Vanilla renders it through the same perspective as the world but with the
/// FOV *unmodified* by the speed/effect multipliers — `getFov(camera, partial,
/// false)`. Rewo has no FOV modifiers yet, so this is the world's projection;
/// the near plane matters more, and it is shared, which is what keeps the
/// item's near corner from clipping.
fn hand_view_proj(aspect: f32) -> glam::Mat4 {
    glam::Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        70f32.to_radians(),
        aspect,
        0.05,
    ))
}

/// Pack the hand atlas: the item textures the two hands need, plus the skin.
fn pack_hand_atlas(
    held: &rewo_gpu::held::HeldItems,
    wanted: &[u16],
    skin: Option<&[u8]>,
) -> (Vec<u8>, std::collections::HashMap<u16, [f32; 4]>) {
    let mut rgba = vec![0u8; (HAND_ATLAS * HAND_ATLAS * 4) as usize];
    let mut uv = std::collections::HashMap::new();
    let cols = HAND_ATLAS / GUI_ATLAS_SLOT;
    for (slot, &tex) in wanted.iter().enumerate() {
        let (ox, oy) = (
            (slot as u32 % cols) * GUI_ATLAS_SLOT,
            (slot as u32 / cols) * GUI_ATLAS_SLOT,
        );
        // The bottom row is the skin's; an item that would land there is
        // dropped rather than overlapping it.
        if oy >= HAND_SKIN_Y {
            continue;
        }
        let Some(src) = held.textures.get(tex as usize) else {
            continue;
        };
        if src.w > GUI_ATLAS_SLOT || src.h > GUI_ATLAS_SLOT {
            continue;
        }
        for y in 0..src.h {
            let s = (y * src.w * 4) as usize;
            let d = (((oy + y) * HAND_ATLAS + ox) * 4) as usize;
            let n = (src.w * 4) as usize;
            rgba[d..d + n].copy_from_slice(&src.rgba[s..s + n]);
        }
        uv.insert(
            tex,
            [
                ox as f32 / HAND_ATLAS as f32,
                oy as f32 / HAND_ATLAS as f32,
                src.w as f32 / HAND_ATLAS as f32,
                src.h as f32 / HAND_ATLAS as f32,
            ],
        );
    }
    if let Some(skin) = skin {
        for y in 0..64u32 {
            let s = (y * 64 * 4) as usize;
            let d = (((HAND_SKIN_Y + y) * HAND_ATLAS + HAND_SKIN_X) * 4) as usize;
            if s + 256 <= skin.len() {
                rgba[d..d + 256].copy_from_slice(&skin[s..s + 256]);
            }
        }
    }
    (rgba, uv)
}

/// Build and upload this frame's hand.
#[allow(clippy::too_many_arguments)]
fn apply_hand(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    session: &PlaySession,
    items: &rewo_data::items::Items,
    state: &mut HandState,
    partial: f32,
    aspect: f32,
) {
    use rewo_gpu::hand::{Arm, HandDraw};

    // The item on screen is the one the equip clock says, which lags the held
    // one across a swap — that is the whole point of the dip.
    let model_of = |id: Option<i32>| {
        id.and_then(|i| items.name(i))
            .and_then(|n| state.held.any(n))
    };
    let main_item = model_of(state.main_equip.visible_item());
    let off_item = model_of(state.off_equip.visible_item());

    // Every texture the two hands need this frame.
    let mut wanted: Vec<u16> = Vec::new();
    for m in [main_item, off_item].into_iter().flatten() {
        for q in &m.quads {
            if !wanted.contains(&q.tex) {
                wanted.push(q.tex);
            }
        }
    }
    if wanted != state.resident || !state.skin_resident || !wr.hand_ready() {
        let (rgba, uv) = pack_hand_atlas(
            &state.held,
            &wanted,
            state.skin.as_ref().map(|(px, _)| px.as_slice()),
        );
        if let Err(e) = wr.init_hand(gpu, &rgba, HAND_ATLAS, HAND_ATLAS) {
            log::warn!("live: hand atlas upload failed: {e}");
            return;
        }
        // **After** `init_hand`, not before: that call destroys and rebuilds
        // the pass, so a glint installed first is thrown away with it. The
        // first build had these the other way round and drew no shimmer at
        // all — the GUI path gets this right by having grown in the same
        // order.
        if let Some(g) = state.glint.as_ref() {
            if let Err(e) = wr.init_hand_glint(gpu, &g.0, g.1, g.2) {
                log::warn!("live: hand glint upload failed: {e}");
            }
        }
        state.uv = uv;
        state.resident = wanted;
        state.skin_resident = state.skin.is_some();
    }

    let attack = state
        .forced_attack
        .unwrap_or_else(|| session.local_attack_anim(partial));
    // The rig comes from the item's own `SwingAnimation` — the seven spears
    // STAB, everything else WHACK, and an item whose animation is NONE holds
    // still. Resolved through the same prototype table M19 uses for every
    // other entity's swing, so a spear thrusts in first person exactly as it
    // does in third.
    let swing_kind = |id: Option<i32>| {
        use rewo_data::swing_anim::SwingAnimationType;
        use rewo_gpu::hand::SwingKind;
        match id.and_then(|i| session.swing_data.as_ref().and_then(|d| d.prototypes.of(i))) {
            Some(a) => match a.kind {
                SwingAnimationType::None => SwingKind::None,
                SwingAnimationType::Stab => SwingKind::Stab,
                SwingAnimationType::Whack => SwingKind::Whack,
            },
            // An item this build cannot resolve holds still rather than
            // guessing a rig — the rule the click arithmetic uses too.
            None => SwingKind::None,
        }
    };
    let sway = state
        .bob
        .sway(session.player.pitch, session.player.yaw, partial);
    let view = rewo_gpu::hand::view_sway(sway);

    // The use in progress, if any — it poses exactly one hand, and replaces
    // that hand's swing rather than combining with it.
    let use_state = session.local_use_state();
    let use_for = |hand: rewo_world::entities::InteractionHand| {
        use rewo_data::use_item::ItemUseAnimation as A;
        use rewo_gpu::hand::{UseAnim, UsePose};
        if !use_state.poses_hand(hand) {
            return None;
        }
        let id = use_state.item_id?;
        let profile = session.swing_data.as_ref()?.use_profiles.of(id)?;
        let anim = match profile.animation {
            A::None => UseAnim::None,
            A::Eat => UseAnim::Eat,
            A::Drink => UseAnim::Drink,
            A::Block => UseAnim::Block,
            A::Bow => UseAnim::Bow,
            A::Trident => UseAnim::Trident,
            A::Crossbow => UseAnim::Crossbow,
            A::Spyglass => UseAnim::Spyglass,
            A::TootHorn => UseAnim::TootHorn,
            A::Brush => UseAnim::Brush,
            A::Bundle => UseAnim::Bundle,
            A::Spear => UseAnim::Spear,
        };
        Some(UsePose {
            anim,
            remaining: use_state.remaining_ticks(),
            duration: profile.duration,
        })
    };
    // `case BLOCK` excepts a real shield, which carries its own display
    // transform for the context and would otherwise be posed twice.
    let is_shield = |id: Option<i32>| {
        id.and_then(|i| items.name(i)) == Some("minecraft:shield")
    };
    // `ItemStack.hasFoil()` for each hand (M44). It comes from the
    // **inventory**, not the equipment feed: the server never sends a player
    // their own equipment, which is the same reason M38's swing duration reads
    // the inventory too.
    let main_glint = session.inventory.held().is_some_and(|s| s.enchanted);
    let off_glint = session.inventory.offhand().is_some_and(|s| s.enchanted);

    let hands = [
        HandDraw {
            arm: Arm::Right,
            item: main_item,
            attack,
            inverse_height: state.main_equip.inverse(partial),
            swings: swing_kind(state.main_equip.visible_item()),
            main_hand: true,
            glint: main_glint,
            using: use_for(rewo_world::entities::InteractionHand::MainHand),
            is_shield: is_shield(state.main_equip.visible_item()),
        },
        HandDraw {
            arm: Arm::Left,
            item: off_item,
            // Only the swinging hand animates, and the local player's swing is
            // always the main hand's — `LocalPlayer.swing` passes MAIN_HAND.
            attack: 0.0,
            inverse_height: state.off_equip.inverse(partial),
            swings: swing_kind(state.off_equip.visible_item()),
            main_hand: false,
            glint: off_glint,
            using: use_for(rewo_world::entities::InteractionHand::OffHand),
            is_shield: is_shield(state.off_equip.visible_item()),
        },
    ];
    let arm_geo = state
        .skin
        .as_ref()
        .map(|(_, slim)| rewo_gpu::hand::ArmGeometry {
            skin_uv: [
                HAND_SKIN_X as f32 / HAND_ATLAS as f32,
                HAND_SKIN_Y as f32 / HAND_ATLAS as f32,
                64.0 / HAND_ATLAS as f32,
                64.0 / HAND_ATLAS as f32,
            ],
            slim: *slim,
        });
    let verts = rewo_gpu::hand::build_vertices(
        view,
        &hands,
        &|t| state.uv.get(&t).copied(),
        arm_geo.as_ref(),
    );
    // Wall-clock phase, exactly as the GUI glint uses — vanilla reads
    // `Util.getMillis()` for both.
    let millis = state.started.elapsed().as_secs_f64() * 1000.0;
    let glint = rewo_gpu::hand::build_glint_vertices(view, &hands, millis);
    let vp = hand_view_proj(aspect).to_cols_array_2d();
    if let Err(e) = wr.set_hand_with_glint(gpu, &verts, &glint, vp) {
        log::warn!("live: hand upload failed: {e}");
    }
}

/// The app's screen state: the framework's one slot (M82) plus the cursor.
///
/// Before M82 this was the inventory's `open: bool`. The slot is
/// `rewo_world::screen::Screens` — vanilla's `Gui.screen`, a single field, not
/// a stack — and the two accessors below are the seam every input path routes
/// through:
///
/// * [`Self::any_open`] is "a screen owns the cursor and the keyboard", which
///   is what frees the mouse, holds the camera still and swallows world input.
/// * [`Self::inventory_open`] is the *inventory* specifically, which is what
///   the slot hover, the drag and `click_screen` mean.
///
/// Conflating the two is the mistake this split exists to prevent: a death
/// screen must free the cursor without making `slot_at` meaningful.
#[derive(Default)]
pub struct ScreenState {
    pub screens: rewo_world::screen::Screens,
    /// Cursor position in screen pixels. Only tracked while a screen is
    /// open; the rest of the time the cursor is grabbed and its position is
    /// meaningless.
    pub mouse: (f64, f64),
    /// How many `container_close` packets had arrived last time the frame
    /// looked (M74). A watermark rather than a flag so a close that lands in
    /// the same frame as another cannot be swallowed, and so nothing has to
    /// reach into the session to clear state it does not own.
    pub close_requests_seen: u64,
    /// The container id whose screen is currently up, if a container's (M89).
    ///
    /// A watermark on the *menu*, not a mirror of the screen's open flag: a
    /// server re-opening the same slot gets a fresh menu, and comparing
    /// against the screen state would miss it.
    pub container_shown: Option<i32>,
}

impl ScreenState {
    /// Any screen at all.
    pub fn any_open(&self) -> bool {
        self.screens.is_open()
    }

    /// The inventory specifically.
    pub fn inventory_open(&self) -> bool {
        self.screens.is(rewo_world::screen::ScreenKind::Inventory)
    }

    /// The menu slot under the cursor, if any.
    ///
    /// Takes the layout on screen (M89): both halves of this are panel-sized.
    /// `screen_to_gui` centres the panel to find the origin, and `slot_at`
    /// scans that layout's own slots — so asking the player's 176x166 while a
    /// 176x222 chest is up shifts the cursor 28 px relative to the panel *and*
    /// then looks it up in the wrong slot list. The two errors do not cancel.
    fn hovered(
        &self,
        layout: &rewo_world::menu_layout::MenuLayout,
        w: f32,
        h: f32,
    ) -> Option<usize> {
        let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
            self.mouse,
            w,
            h,
            layout.image_w as f32,
            layout.image_h as f32,
        );
        layout.slot_at(gx, gy)
    }
}

/// The per-screen state M85's three screens need across frames.
///
/// The death screen keeps its own field (`LiveApp::death`) because M82 gave it
/// one and its lifecycle is driven by the wire rather than by a key; these
/// three are opened and closed by presses, so one slot mirroring
/// [`rewo_world::screen::Screens`]' one slot is the honest shape.
///
/// What each variant carries is exactly what a **resize rebuild** needs —
/// `Screen.resize` is `init()`, so the builder has to be re-runnable from
/// state the app still holds.
#[derive(Clone, Debug, Default)]
pub(crate) enum ScreenView {
    #[default]
    None,
    Pause(rewo_world::pause_screen::PauseLabels, bool),
    Links(rewo_world::server_links_screen::ServerLinksLabels),
    Disconnected(
        rewo_world::disconnect_screen::DisconnectLabels,
        rewo_world::disconnect_screen::DisconnectDetails,
    ),
}

/// Everything the death screen needs across frames (M82).
///
/// The model and its labels are separate because the labels come from the
/// language map (an asset) and the model comes from the wire, and only the
/// model changes while the screen is up.
pub(crate) struct DeathView {
    pub model: rewo_world::death_screen::DeathScreen,
    pub labels: rewo_world::death_screen::DeathLabels,
    /// The death message, parsed into styled spans once. A server's is
    /// routinely coloured, and `TRUSTED_STREAM_CODEC` means the style is the
    /// server's to set.
    pub cause: Option<rewo_net::chat_style::ChatLine>,
    /// `PlaySession::respawn_epoch` when the screen opened. The screen closes
    /// when it moves — see the field's docs for why that, and not the button
    /// press, is what ends it.
    pub respawn_epoch: u64,
}

impl DeathView {
    /// `DeathScreen`'s constructor + `init()`, given a decoded kill and the
    /// baked language map.
    pub(crate) fn open(
        kill: &rewo_net::CombatKill,
        hardcore: bool,
        score: i32,
        lang: &rewo_data::lang::Language,
        respawn_epoch: u64,
        gui_w: i32,
        gui_h: i32,
    ) -> (Self, rewo_world::screen::Screen) {
        use rewo_net::chat_style::{self, ChatStyle};
        let model = rewo_world::death_screen::DeathScreen {
            // The message is kept even when it flattens to nothing: vanilla's
            // `causeOfDeath` is `@Nullable` and a *present but empty* component
            // still takes the non-null branch and draws an empty line.
            cause_of_death: Some(kill.message.to_plain_text()),
            hardcore,
            score,
        };
        let labels = model.labels(lang);
        let cause = Some(chat_style::parse_component(&kill.message, ChatStyle::WHITE));
        let screen = model.build(&labels, gui_w, gui_h);
        (
            Self {
                model,
                labels,
                cause,
                respawn_epoch,
            },
            screen,
        )
    }

    /// `Screen.resize` → `repositionElements` → `rebuildWidgets` → `init()`.
    ///
    /// **`init()` resets `delayTicker` to 0 and disables the buttons again**,
    /// so resizing the window while dead restarts the one-second guard. That
    /// falls out of rebuilding rather than being coded, because
    /// [`rewo_world::death_screen::DeathScreen::build`] *is* `init()` and
    /// `Screen::new` starts its clock at zero.
    pub(crate) fn reposition(
        &self,
        screen: &mut rewo_world::screen::Screen,
        gui_w: i32,
        gui_h: i32,
    ) {
        *screen = self.model.build(&self.labels, gui_w, gui_h);
    }
}

/// Any screen's chrome: its background and its buttons (M82, generalised in
/// M85).
///
/// Pure, and takes the screen rather than the app, so the gate drives the same
/// builder the frame path does. Nothing here is death-screen-specific and
/// nothing ever was — M85 only had to teach it the two widget kinds that are
/// **not** buttons: a label draws through the text pass, and a
/// [`rewo_world::screen::WidgetKind::Reserved`] draws nothing at all, on
/// purpose (see `Widget::reserved`).
pub(crate) fn screen_chrome(
    screen: &rewo_world::screen::Screen,
    mouse: Option<(f64, f64)>,
) -> rewo_gpu::screen::ScreenDraw {
    use rewo_world::screen::{ButtonSprite as W, WidgetKind};
    let focused = screen.focused();
    rewo_gpu::screen::ScreenDraw {
        backdrop: screen.backdrop.map(|b| (b.top, b.bottom)),
        menu_background: screen
            .menu_background
            .map(|b| rewo_gpu::screen::MenuBackgroundDraw {
                in_world: b.in_world,
            }),
        buttons: screen
            .widgets
            .iter()
            .filter(|w| w.visible && w.kind == WidgetKind::Button)
            .map(|w| rewo_gpu::screen::ButtonDraw {
                x: w.x,
                y: w.y,
                width: w.width,
                height: w.height,
                sprite: match w.sprite(w.is_hovered(mouse), focused == Some(w.id)) {
                    W::Enabled => rewo_gpu::screen::ButtonSprite::Enabled,
                    W::Disabled => rewo_gpu::screen::ButtonSprite::Disabled,
                    W::Highlighted => rewo_gpu::screen::ButtonSprite::Highlighted,
                },
            })
            .collect(),
        // M84's statistics screen fills this; every other screen's chrome is
        // its backdrop and its buttons.
        sprites: Vec::new(),
    }
}

/// Every widget's text, for a screen whose widgets carry all of it (M85).
///
/// A button's label is centred in its own rect by `defaultScrollingHelper` and
/// coloured by `WithInactiveMessage`; a `StringWidget` draws at its own `x`; a
/// `MultiLineTextWidget` draws one line per 9 px, centred about the widget's
/// midpoint when `setCentered(true)`. A `Reserved` widget draws nothing.
///
/// `px` is the GUI scale, the same convention `death_screen_lines` uses.
pub(crate) fn screen_text_lines(
    screen: &rewo_world::screen::Screen,
    advance: &[u8; 256],
    px: f32,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_world::screen::WidgetKind;
    let mut out = Vec::new();
    let mut push = |text: &str, x: i32, y: i32, color: [f32; 3]| {
        if text.is_empty() {
            return;
        }
        out.push(rewo_gpu::world::OwnedTextLine {
            x: x as f32 * px,
            y: y as f32 * px,
            px,
            color,
            alpha: 1.0,
            shadow: true,
            text: text.to_string(),
        });
    };
    for widget in screen.widgets.iter().filter(|w| w.visible) {
        match &widget.kind {
            WidgetKind::Reserved => {}
            // M84's tabs and image buttons. Only the statistics screen builds
            // them and it has its own text builder (`stats_view::lines`),
            // because a tab's label is centred by `MenuTabButton.renderLabel`
            // rather than by `defaultScrollingHelper` — the two differ by the
            // 3-px drop an unselected tab takes.
            WidgetKind::Sprites { .. } => {}
            WidgetKind::Button => {
                let w = rewo_gpu::text::width(&widget.message, advance);
                let (anchor, top) = widget.label_anchor(w);
                push(&widget.message, anchor - w / 2, top, widget.label_color());
            }
            WidgetKind::Label { centered } => {
                // `StringWidget.visitLines`: `x = getX()`, and
                // `y = getY() + (getHeight() - 9) / 2`.
                let w = rewo_gpu::text::width(&widget.message, advance);
                let x = if *centered {
                    widget.x + widget.width / 2 - w / 2
                } else {
                    widget.x
                };
                let y = widget.y + (widget.height - 9) / 2;
                push(&widget.message, x, y, widget.label_color());
            }
            WidgetKind::MultiLabel { lines, centered } => {
                // `MultiLineLabel.visitLines(alignment, midX, y, 9, output)` —
                // `getTextY()` is the widget's own `y`, with no vertical
                // centring, because the widget's height *is* the text's.
                let mid = widget.x + widget.width / 2;
                for (i, line) in lines.iter().enumerate() {
                    let w = rewo_gpu::text::width(line, advance);
                    let x = if *centered { mid - w / 2 } else { widget.x };
                    push(line, x, widget.y + 9 * i as i32, widget.label_color());
                }
            }
        }
    }
    out
}

/// The death screen's four text runs — title, cause, score, and each button's
/// label (M82).
///
/// `px` is the GUI scale; every coordinate below is in GUI pixels and is
/// multiplied by it, which is the same convention `title_lines` uses.
pub(crate) fn death_screen_lines(
    view: &DeathView,
    screen: &rewo_world::screen::Screen,
    advance: &[u8; 256],
    px: f32,
    (screen_w, _screen_h): (f32, f32),
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_net::chat_style::{self, ChatSpan};
    use rewo_world::death_screen as ds;
    let gui_w = (screen_w / px) as i32;
    let mut out = Vec::new();

    // A run of spans laid end to end from a GUI-space top-left, at a
    // whole-number extra scale. `scale` multiplies the *font* pixel, which is
    // how the title comes out double-size without a second font.
    let mut run = |out: &mut Vec<rewo_gpu::world::OwnedTextLine>,
                   spans: &[ChatSpan],
                   x: i32,
                   y: i32,
                   scale: i32| {
        let mut pen = x;
        for span in spans {
            let w = rewo_gpu::text::width(&span.text, advance);
            if !span.text.is_empty() {
                out.push(rewo_gpu::world::OwnedTextLine {
                    x: pen as f32 * px,
                    y: y as f32 * px,
                    px: px * scale as f32,
                    color: span.color,
                    alpha: 1.0,
                    shadow: true,
                    text: span.text.clone(),
                });
            }
            pen += w * scale;
        }
    };

    // The title, at `TITLE_SCALE`. Its anchor truncates twice — see
    // `rewo_world::death_screen`.
    let title_w = rewo_gpu::text::width(&view.labels.title, advance);
    run(
        &mut out,
        &[plain_span(&view.labels.title)],
        ds::title_left(gui_w, title_w),
        ds::title_top(),
        ds::TITLE_SCALE,
    );

    // The death message, in the server's own styling.
    if let Some(cause) = &view.cause {
        let w = rewo_gpu::text::width(&chat_style::plain_text(cause), advance);
        let (x, y) = ds::cause_pos(gui_w, w);
        run(&mut out, cause, x, y, 1);
    }

    // `deathScreen.score.value` — "Score: %s" with the value in YELLOW. Two
    // spans, not one: `Component.translatable(key, scoreValue)` nests a styled
    // literal inside an unstyled template, so the number is yellow and the
    // word is not.
    let score = score_spans(&view.labels.score_template, view.model.score);
    let w = rewo_gpu::text::width(&chat_style::plain_text(&score), advance);
    let (x, y) = ds::score_pos(gui_w, w);
    run(&mut out, &score, x, y, 1);

    // Each button's label, centred in its own rect by
    // `defaultScrollingHelper` and coloured by `WithInactiveMessage`.
    for widget in screen.widgets.iter().filter(|w| w.visible) {
        let w = rewo_gpu::text::width(&widget.message, advance);
        let (anchor, top) = widget.label_anchor(w);
        let mut span = plain_span(&widget.message);
        span.color = widget.label_color();
        run(&mut out, &[span], anchor - w / 2, top, 1);
    }
    out
}

fn plain_span(text: &str) -> rewo_net::chat_style::ChatSpan {
    rewo_net::chat_style::ChatSpan {
        text: text.to_string(),
        color: [1.0, 1.0, 1.0],
        bold: false,
        italic: false,
        underlined: false,
        strikethrough: false,
        obfuscated: false,
    }
}

/// `Component.translatable("deathScreen.score.value", literal(score).withStyle(YELLOW))`.
///
/// The template is split on its one `%s`; the value takes
/// `ChatFormatting.YELLOW`'s `0xFFFF55`. A template with no `%s` — a resource
/// pack could ship one — yields the template alone, which is what
/// `decomposeTemplate` does with a pattern that consumes no argument.
pub(crate) fn score_spans(template: &str, score: i32) -> rewo_net::chat_style::ChatLine {
    const YELLOW: u32 = 0xFF_FF55;
    let mut out = Vec::new();
    let value = score.to_string();
    match template.split_once("%s") {
        Some((head, tail)) => {
            out.push(plain_span(head));
            let mut v = plain_span(&value);
            v.color = rewo_net::chat_style::rgb_f32(YELLOW);
            out.push(v);
            out.push(plain_span(tail));
        }
        None => out.push(plain_span(template)),
    }
    out.retain(|s| !s.text.is_empty());
    out
}

/// Build and hand over one frame of the open screen: icons, count labels,
/// the highlight and the player preview.
///
/// One function so the windowed and headless paths cannot drift — the headless
/// one exists to photograph exactly what the windowed one shows.
#[allow(clippy::too_many_arguments)]
fn apply_screen(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    session: &PlaySession,
    items: &rewo_data::items::Items,
    gui: &mut GuiItemState,
    baked: &assets::BakedAssets,
    skin: Option<&mut PreviewTextures>,
    mut glyphs: Option<&mut rewo_gpu::velvet_glyph::GlyphCache>,
    // `options.advancedItemTooltips` — F3+H (M66).
    flag: rewo_gpu::tooltip::TooltipFlag,
    mouse: (f64, f64),
    (w, h): (f32, f32),
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
) {
    // Which menu is on screen: the open container if there is one, else the
    // player's own. Chosen ONCE and threaded everywhere, because the panel,
    // the icons, the hover and the durability bars are all measured from the
    // same origin — and that origin depends on the menu's panel size. Picking
    // it per-consumer is how a chest ends up painted at one size with its
    // icons placed at another.
    let menu = session
        .menus
        .open()
        .map(|m| &m.menu)
        .unwrap_or(&session.inventory);
    let layout = menu.layout();

    // The container's own background sheet, or `None` for the player's
    // inventory, which the pass draws from its own `inventory.png` rect.
    wr.set_container_panel(container_panel(layout, session.menus.open()));

    let (mut icons, mut labels) = screen_icons(menu, items, &session.trim_materials, w, h);
    if let Some((icon, label)) = carried_icon(menu, items, &session.trim_materials, mouse, w, h) {
        icons.push(icon);
        labels.extend(label);
    }
    apply_gui_icons(wr, gpu, gui, &icons);

    let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
        mouse,
        w,
        h,
        layout.image_w as f32,
        layout.image_h as f32,
    );
    wr.set_container(
        true,
        layout
            .slot_at(gx, gy)
            .and_then(|s| layout.position(s))
            .map(|(x, y)| (x as i32, y as i32)),
    );

    // Every visible slot's durability bar, plus the cursor's. The screen's
    // rects and the hotbar's go through the same builder.
    {
        let rects = menu_slot_rects(menu, w, h);
        let mut stacks: Vec<_> = (0..menu.slot_count())
            .map(|i| (menu.menu_slot(i), rects[i]))
            .collect();
        if let Some(carried) = menu.carried() {
            if let Some((icon, _)) = carried_icon(menu, items, &session.trim_materials, mouse, w, h) {
                stacks.push((Some(carried), (icon.x, icon.y, icon.size)));
            }
        }
        // `!has(UNBREAKABLE)`. No item's *prototype* carries the component in
    // 26.2 — it is only ever patched on — so the patch flag is the whole
    // answer here.
    wr.set_item_bars(item_bars(&stacks, items, |s| {
        session.inventory.text_of(s).is_some_and(|t| t.unbreakable)
    }));
    }

    // The tooltip's box and its one line. Both need the font's advances, so a
    // build with no baked font simply draws no tooltip.
    let tooltip = wr.font_advance().and_then(|advance| {
        screen_tooltip(
            &session.inventory,
            items,
            &baked.item_names,
            &baked.lang,
            &session.enchantments,
            &baked.enchantment_text,
            &session.stack_details,
            session.component_names.as_deref(),
            flag,
            &advance,
            glyphs.as_deref_mut(),
            mouse,
            (w, h),
        )
    });
    wr.set_container_tooltip(tooltip.as_ref().map(|(draw, _, _)| draw.clone()));

    // The preview's pass is built the first time the screen opens, so a
    // session that never opens it never pays for the second entity atlas.
    if !wr.preview_ready() {
        if let Err(e) = wr.init_preview(gpu, font_data(baked), entity_textures(baked)) {
            log::warn!("live: inventory preview unavailable: {e}");
        }
    }
    if wr.preview_ready() {
        let held = [
            session
                .inventory
                .held()
                .and_then(|s| items.name(s.item_id)),
            session
                .inventory
                .offhand()
                .and_then(|s| items.name(s.item_id)),
        ];
        // The skin and the cape go into the preview's *own* atlas the first
        // time the screen opens, and stay there. Both have to be uploaded
        // again here rather than reusing the world pass's addresses: the two
        // passes hold separate atlases, and a cape address is an absolute
        // texel origin, so a borrowed one samples a fixed wrong rectangle.
        let (skin_uv, slim, cape_origin) = match skin {
            Some(t) => {
                if t.skin_uv.is_none() {
                    if let Some((rgba, _)) = t.skin.as_ref() {
                        t.skin_uv = wr.upload_preview_skin(gpu, rgba);
                    }
                }
                if t.cape_origin.is_none() {
                    if let Some(rgba) = t.cape.as_ref() {
                        t.cape_origin = wr.upload_preview_cape(gpu, rgba);
                    }
                }
                (
                    t.skin_uv,
                    t.skin.as_ref().is_some_and(|(_, slim)| *slim),
                    t.cape_origin,
                )
            }
            None => (None, false, None),
        };
        let (draw, vp, rect) =
            preview_draw(session, skin_uv, slim, cape_origin, held, w, h, mouse);
        if let Err(e) = wr.prepare_held_items(gpu, &held.iter().flatten().copied().collect::<Vec<_>>()) {
            log::warn!("live: preview held items: {e}");
        }
        wr.set_preview(Some((&draw, vp, rect)));
    }
    // The tooltip's text goes last so it draws over the icons and the count
    // labels, matching the order the box is drawn in.
        let velvet_runs: Vec<rewo_gpu::velvet_text::OwnedRun> = tooltip
        .as_ref()
        .map(|(_, _, r)| r.clone())
        .unwrap_or_default();
    labels.extend(tooltip.into_iter().flat_map(|(_, l, _)| l));
    (labels, velvet_runs)
}

/// The 46 slot rects the screen draws icons into, in screen pixels.
///
/// The same shape `hotbar_slot_rects` returns, because they feed the same
/// pass: an icon in an inventory slot is the same draw as an icon in a hotbar
/// slot, and only the rectangle differs.
/// The pass's panel description for a menu, or `None` for the player's own.
///
/// This is where `menu_screen`'s sheet-relative UVs are converted back to
/// PIXELS, which is the whole reason `PanelBlit` carries pixels: a screen's
/// UVs are normalised against the sheet size its blit declares — 256 for
/// twenty-one of them and **512 for the merchant** — while the atlas
/// normalises against its own dimensions. Handing the UVs straight across
/// would divide by the wrong number for exactly one screen.
///
/// `None` for a menu with no container screen (`lectern`, a `BookViewScreen`)
/// as well as for the player's, so an open lectern paints no panel rather than
/// some other menu's.
fn container_panel(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    open: Option<&rewo_world::menu::OpenMenu>,
) -> Option<rewo_gpu::container::ContainerPanel> {
    if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID {
        return None;
    }
    let screen = rewo_world::menu_screen::screen_of(layout.protocol_id)?;
    let sheet = sheet_index(screen.texture)?;
    let blits = rewo_world::menu_screen::background_quads(screen)
        .into_iter()
        .map(|q| rewo_gpu::container::PanelBlit {
            dx: q.dx as f32,
            dy: q.dy as f32,
            w: q.w as f32,
            h: q.h as f32,
            sx: q.u0 * screen.sheet_w,
            sy: q.v0 * screen.sheet_h,
        })
        .collect();
    Some(rewo_gpu::container::ContainerPanel {
        sheet,
        blits,
        gui_w: screen.image_w as f32,
        gui_h: screen.image_h as f32,
        overlays: menu_overlays(layout, open),
    })
}

/// A `menu_screen::ProgressBlit` in the render's own units.
fn to_blit(b: rewo_world::menu_screen::ProgressBlit) -> rewo_gpu::container::PanelBlit {
    rewo_gpu::container::PanelBlit {
        dx: b.dx as f32,
        dy: b.dy as f32,
        w: b.w as f32,
        h: b.h as f32,
        sx: b.sx as f32,
        sy: b.sy as f32,
    }
}

/// Everything an open menu paints over its background sheet, in draw order
/// (M91 the furnaces, M92 the rest).
///
/// Empty for a menu with no overlays *and* for one that is not open — a
/// `containershot` panel built with no `OpenMenu` has no data slots to read,
/// and inventing a plausible-looking half-lit furnace would make the gate's
/// panel witnesses grade a state no server ever sent.
fn menu_overlays(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    open: Option<&rewo_world::menu::OpenMenu>,
) -> Vec<(usize, rewo_gpu::container::PanelBlit)> {
    use rewo_data::assets as a;
    let mut out = Vec::new();
    let Some(m) = open else { return out };
    match layout.protocol_id {
        // The furnace family (M91): flame then arrow.
        id if a::progress_index(id).is_some() => {
            let base = a::progress_index(id).expect("guarded above");
            let (flame, arrow) = rewo_world::menu_screen::furnace_progress(
                m.furnace_is_lit(),
                m.furnace_lit_progress(),
                m.furnace_burn_progress(),
            );
            // The lit sprite is the pair's first, the burn its second.
            if let Some(f) = flame {
                out.push((base, to_blit(f)));
            }
            out.push((base + 1, to_blit(arrow)));
        }
        // brewing_stand (M92): fuel, arrow, bubbles — vanilla's own order.
        11 => {
            let (fuel, brew, bubbles) =
                rewo_world::menu_screen::brewing_progress(m.brewing_fuel(), m.brewing_ticks());
            for (sprite, blit) in [
                (a::BREW_FUEL, fuel),
                (a::BREW_PROGRESS, brew),
                (a::BREW_BUBBLES, bubbles),
            ] {
                if let Some(b) = blit {
                    out.push((sprite, to_blit(b)));
                }
            }
        }
        _ => {}
    }
    out
}

/// [`container_panel`] for `containershot`, which drives the production
/// builder rather than a copy of it — M45's finding: a gate that reimplements
/// a slice of the app's setup misses whatever the app adds to it.
pub(crate) fn container_panel_for_test(
    layout: &'static rewo_world::menu_layout::MenuLayout,
) -> Option<rewo_gpu::container::ContainerPanel> {
    container_panel(layout, None)
}

/// [`sheet_index`] for `containershot`.
pub(crate) fn sheet_index_for_test(texture: &str) -> Option<usize> {
    sheet_index(texture)
}

/// A texture's index in the atlas, by the path the bake loaded it under.
///
/// `menu_screen` spells paths in vanilla's `Identifier` form and the bake in
/// the jar-relative one, so the prefix comes off here. The two lists are
/// cross-checked by a test in `rewo-world`; this returning `None` would mean
/// that check had been removed.
fn sheet_index(texture: &str) -> Option<usize> {
    let want = texture.trim_start_matches("textures/");
    rewo_data::assets::MENU_BACKGROUND_TEXTURES
        .iter()
        .position(|t| *t == want)
}

/// Every slot's on-screen rect for a menu, in its own layout and at its own
/// panel size (M87).
///
/// Takes the menu rather than assuming the player's: a container is a
/// different slot count *and* a different panel, and the panel is what centres
/// it — a six-row chest is 176x222, so measuring its slots from a 176x166
/// origin puts every one of them 28 px low.
fn menu_slot_rects(menu: &rewo_world::inventory::Inventory, w: f32, h: f32) -> Vec<(f32, f32, f32)> {
    let layout = menu.layout();
    let (left, top, scale) = rewo_gpu::container::gui_origin_for(
        w,
        h,
        layout.image_w as f32,
        layout.image_h as f32,
    );
    (0..menu.slot_count())
        .map(|i| {
            let (x, y) = layout.position(i).unwrap_or((0, 0));
            (
                left + x as f32 * scale,
                top + y as f32 * scale,
                16.0 * scale,
            )
        })
        .collect()
}

/// The player inventory's slot rects — [`menu_slot_rects`] for the menu that
/// was the only one before M87.
fn screen_slot_rects(w: f32, h: f32) -> Vec<(f32, f32, f32)> {
    menu_slot_rects(&rewo_world::inventory::Inventory::default(), w, h)
}

/// This frame's icons and stack counts for the open screen.
///
/// Returns the draw list plus the count labels, which go through the text pass
/// — vanilla's `itemCount` draws them at `x + 19 - 2 - width` and `y + 6 + 3`,
/// right-aligned inside the slot, and **only when the count is not one**.
/// Load the Velvet families into a glyph cache (M52b).
///
/// Returns `None` if any face is missing, and the caller falls back to the
/// bitmap pass. Partial loading is deliberately not a state: a tooltip that
/// renders its upright spans and drops its italic ones would look like a
/// styling bug rather than a missing file.
fn load_velvet_fonts() -> Option<rewo_gpu::velvet_glyph::GlyphCache> {
    use rewo_gpu::velvet_glyph::{Family, GlyphCache};
    // Next to the executable first (a packaged build), then the workspace
    // path (cargo run) -- the same order the launcher's font resolution uses.
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("assets/fonts")));
    let dir = match beside_exe {
        Some(d) if d.is_dir() => d,
        _ => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts"),
    };
    let mut c = GlyphCache::new();
    for (fam, italic) in [
        (Family::Newsreader, false),
        (Family::Newsreader, true),
        (Family::Fraunces, false),
        (Family::JetBrainsMono, false),
    ] {
        let p = dir.join(format!("{}.ttf", fam.file_stem(italic)));
        let data = std::fs::read(&p).ok()?;
        if !c.load(fam, italic, data) {
            log::warn!("velvet: {} failed to parse", p.display());
            return None;
        }
    }
    log::info!("velvet: fonts loaded from {}", dir.display());
    Some(c)
}

/// The tooltip's Velvet body size, in **GUI pixels**.
///
/// Vanilla's tooltip text is the 8px bitmap font on a 10px line. Newsreader at
/// the same nominal size reads smaller, because a bitmap glyph fills its cell
/// and a proportional one does not -- so this is calibrated to the *cap
/// height* rather than the em, which is what makes the two look like the same
/// size next to each other.
pub const TOOLTIP_TEXT_GUI_PX: f32 = 9.0;

/// A tooltip line's Velvet key.
fn tooltip_key(italic: bool, scale: f32) -> rewo_gpu::velvet_glyph::ScalerKey {
    rewo_gpu::velvet_glyph::ScalerKey::new(
        rewo_gpu::velvet_glyph::Family::Newsreader,
        italic,
        TOOLTIP_TEXT_GUI_PX * scale,
        rewo_gpu::velvet_glyph::Axes::DEFAULT,
    )
}

/// Width of a styled tooltip line in **GUI pixels**, measured with the same
/// font that will draw it.
///
/// This is the half of the flip that is easy to skip and would have shown up
/// as text spilling out of its box: once the tooltip renders in Newsreader,
/// sizing it with the bitmap advances measures a font it no longer uses.
fn velvet_line_width(
    cache: &mut rewo_gpu::velvet_glyph::GlyphCache,
    line: &rewo_gpu::tooltip::Line,
    scale: f32,
) -> f32 {
    line.iter()
        .map(|sp| cache.measure_tracked(tooltip_key(sp.italic, scale), &sp.text, 0.0))
        .sum::<f32>()
        / scale.max(0.001)
}
/// The hovered slot's tooltip: the box to draw, and the text line inside it
/// (M40).
///
/// Vanilla builds the lines with `Screen.getTooltipFromItem`, which starts
/// with the stack's hover name and then appends everything its **components**
/// say — enchantments, lore, durability, attribute modifiers, the "When on
/// body" block. Rewo can see none of those: `rewo_net::item_stack` reports
/// only *whether* a patch was present. So a Rewo tooltip is the first line and
/// nothing else, which is exactly what vanilla shows for a plain stack, and
/// short of what it shows for an enchanted one. Drawing a wrong second line
/// would be worse than drawing none.
///
/// The name is also always in the common rarity's white: rarity rides on
/// `DataComponents.RARITY`, which is another component.
/// The durability bars for a set of slots (M41).
///
/// `ItemStack.isBarVisible()` is `isDamaged()`, so a pristine tool has **no
/// bar**, not a full one — which is why this returns nothing for an undamaged
/// stack rather than a 13-wide green one.
///
/// The two halves of the fraction come from different places, and that is the
/// point of the milestone: the numerator `minecraft:damage` rides on the wire
/// as a component patch, the denominator does not — every diamond pickaxe has
/// the same 1561, so it lives in the generated item table. A patch that
/// overrides `max_damage` still wins, because a plugin may.
fn item_bars(
    slots: &[(Option<rewo_world::inventory::ItemSlot>, (f32, f32, f32))],
    items: &rewo_data::items::Items,
    unbreakable: impl Fn(rewo_world::inventory::ItemSlot) -> bool,
) -> Vec<rewo_gpu::container::ItemBar> {
    let mut out = Vec::new();
    for (stack, (x, y, size)) in slots {
        let Some(stack) = stack else { continue };
        let max = stack
            .max_damage
            .or_else(|| items.name(stack.item_id).and_then(rewo_data::item_props_table::max_damage));
        // An item whose maximum this build cannot resolve gets no bar. A bar
        // needs a denominator, and inventing one would draw a confident and
        // wrong amount of remaining life.
        let Some(max) = max.filter(|m| *m > 0) else {
            continue;
        };
        // `isBarVisible()` is `isDamaged()`, and that is
        // `has(MAX_DAMAGE) && !has(UNBREAKABLE) && has(DAMAGE) && damage > 0`
        // — so an **Unbreakable** tool never shows one however much damage it
        // carries (M66 corrected this; M41 read the damage alone). The damage
        // is also clamped to the maximum, which is what stops a server sending
        // more than the item can take from producing a negative width.
        let damage = stack.damage.unwrap_or(0).clamp(0, max);
        if damage <= 0 || unbreakable(*stack) {
            continue;
        }
        out.push(rewo_gpu::container::ItemBar {
            x: *x,
            y: *y,
            // The slot rects are in screen pixels at `size` per 16 GUI pixels.
            scale: size / 16.0,
            width: rewo_gpu::container::bar_width(damage, max),
            color: rewo_gpu::container::bar_color(damage, max),
        });
    }
    out
}

/// `ItemStack.getRarity()` — the id whose colour the hover name takes (M50).
///
/// ```java
/// Rarity baseRarity = this.getOrDefault(DataComponents.RARITY, Rarity.COMMON);
/// if (!this.isEnchanted()) return baseRarity;
/// return switch (baseRarity) {
///    case COMMON, UNCOMMON -> Rarity.RARE;
///    case RARE             -> Rarity.EPIC;
///    default               -> baseRarity;
/// };
/// ```
///
/// Two halves, and Rewo could previously see only one of them. `getOrDefault`
/// answers from the item's **prototype** when the patch says nothing, and the
/// patch is all the wire carries — so `rarity.unwrap_or(COMMON)` painted all
/// **115** of 26.2's non-common items white. The prototype half is
/// [`rewo_data::item_props_table::rarity`], generated from the datagen
/// component report.
///
/// `is_enchanted` is `minecraft:enchantments` alone — see
/// [`rewo_world::inventory::SlotText::is_enchanted`] on why an enchanted book
/// is not enchanted.
///
/// An id outside the enum passes through the `default` arm unchanged, exactly
/// as a future `Rarity` constant would.
pub(crate) fn stack_rarity(item_name: Option<&str>, patch: Option<i32>, is_enchanted: bool) -> i32 {
    let base = patch.unwrap_or_else(|| {
        item_name
            .map(rewo_data::item_props_table::rarity)
            .unwrap_or(rewo_data::item_props_table::DEFAULT_RARITY)
    });
    if !is_enchanted {
        return base;
    }
    match base {
        0 | 1 => 2,
        2 => 3,
        other => other,
    }
}

/// `Rarity.color()` — the hover name's colour, by rarity id.
///
/// An unknown id is treated as common rather than wrapping into another
/// colour: the enum is a small one today and a version that grows it should
/// not repaint every item.
fn rarity_color(rarity: i32) -> [f32; 3] {
    let rgb = match rarity {
        1 => 0xFFFF55u32, // UNCOMMON — yellow
        2 => 0x55FFFF,    // RARE — aqua
        3 => 0xFF55FF,    // EPIC — light purple
        _ => 0xFFFFFF,    // COMMON
    };
    [
        ((rgb >> 16) & 0xFF) as f32 / 255.0,
        ((rgb >> 8) & 0xFF) as f32 / 255.0,
        (rgb & 0xFF) as f32 / 255.0,
    ]
}

/// `ItemLore`'s style — dark purple and italic. Rewo has no italic face, so
/// only the colour carries.
const LORE_COLOR: [f32; 3] = [170.0 / 255.0, 0.0, 170.0 / 255.0];
/// `ItemStack.UNBREAKABLE_TOOLTIP`, which is blue.
const UNBREAKABLE_COLOR: [f32; 3] = [85.0 / 255.0, 85.0 / 255.0, 1.0];

/// `ChatFormatting.GRAY` — an ordinary enchantment line.
const ENCHANT_COLOR: [f32; 3] = [170.0 / 255.0, 170.0 / 255.0, 170.0 / 255.0];
/// `ChatFormatting.RED` — a curse's.
const CURSE_COLOR: [f32; 3] = [1.0, 85.0 / 255.0, 85.0 / 255.0];

/// `Enchantment.getFullname` for each of a stack's enchantments, in
/// `ItemEnchantments.addToTooltip`'s order (M42).
///
/// Three rules, each of which is a way to be visibly wrong:
///
/// - **The level numeral is suppressed only when `level == 1 && maxLevel == 1`.**
///   So a level-1 Mending (max 1) reads "Mending" and a level-1 Sharpness
///   (max 5) reads "Sharpness I". Suppressing on `level == 1` alone loses the
///   numeral from every single-level enchant a player actually applies.
/// - **A curse is red**, everything else grey.
/// - **The order is the `minecraft:tooltip_order` tag first**, then whatever
///   the stack carries that the tag does not mention — appended after, in the
///   stack's own order, whatever their ids.
///
/// An id the registry does not contain yields **no line**. That case means the
/// server sent an enchantment this session never synced, and inventing a name
/// for it would be worse than the omission.
pub(crate) fn enchantment_lines(
    enchantments: &[(i32, i32)],
    registry: &[rewo_net::enchantment_parse::EnchantmentDef],
    text: &rewo_data::enchantments::EnchantmentText,
) -> Vec<rewo_gpu::tooltip::Line> {
    let mut rows: Vec<(Option<usize>, usize, String, [f32; 3])> = Vec::new();
    for (order, &(id, level)) in enchantments.iter().enumerate() {
        let Some(def) = usize::try_from(id).ok().and_then(|i| registry.get(i)) else {
            continue;
        };
        // A datapack may name an enchantment with literal text rather than a
        // translation key; translating that would find nothing.
        let name = if def.literal {
            Some(def.description_key.clone())
        } else {
            text.translate(&def.description_key).map(str::to_string)
        };
        let Some(name) = name else { continue };
        let line = if level == 1 && def.max_level == 1 {
            name
        } else {
            format!("{name} {}", text.level(level))
        };
        let color = if text.is_curse(&def.id) {
            CURSE_COLOR
        } else {
            ENCHANT_COLOR
        };
        rows.push((text.tooltip_rank(&def.id), order, line, color));
    }
    // `None` sorts after `Some` for an `Option` key, which is exactly the
    // behaviour wanted here: the tag's members first, in tag order, then the
    // rest in the order the stack listed them.
    rows.sort_by_key(|(rank, order, _, _)| (rank.is_none(), *rank, *order));
    // One span per line here -- an enchantment name is uniformly coloured. The
    // span model earns its keep on lore (italic) and leaves room for the
    // mid-line colour changes vanilla does elsewhere.
    rows.into_iter()
        .map(|(_, _, l, c)| vec![rewo_gpu::tooltip::Span::new(l, c)])
        .collect()
}

/// `ItemContainerContents.addToTooltip` (M66) — the shulker-box preview.
///
/// The line count comes from [`rewo_gpu::tooltip::container_plan`], which is
/// the loop verbatim; what happens here is the translation, and the two keys
/// it needs (`item.container.item_count`, `item.container.more_items`) exist
/// **only after M54's deprecation pass** — `en_us.json` still carries them
/// under their pre-rename `container.shulkerBox.*` names, so a raw read of the
/// language file produces no container lines at all.
///
/// A present stack whose item id this session cannot name drops the **whole**
/// block rather than one line: the remainder is computed from the stack count,
/// so omitting a line silently makes "and 2 more…" wrong as well.
pub(crate) fn container_lines(
    slots: &[Option<rewo_net::item_stack::ContainerSlot>],
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
    lang: &rewo_data::lang::Language,
) -> Vec<rewo_gpu::tooltip::Line> {
    use rewo_gpu::tooltip::Span;
    // `nonEmptyItemsStream` — a gap is a real slot position, and the tooltip
    // is the one consumer that does not care. The filter belongs here rather
    // than in the decode, which is exactly why M63 keeps the gaps.
    let entries: Vec<&rewo_net::item_stack::ContainerSlot> = slots.iter().flatten().collect();
    if entries.is_empty() {
        return Vec::new();
    }
    let (Some(count_key), Some(more_key)) = (
        lang.get("item.container.item_count"),
        lang.get("item.container.more_items"),
    ) else {
        return Vec::new();
    };
    let plan = rewo_gpu::tooltip::container_plan(entries.len());
    let mut out = Vec::new();
    for e in entries.iter().take(plan.shown) {
        let Some(translated) = items.name(e.item_id).and_then(|n| names.get(n)) else {
            return Vec::new();
        };
        out.push(vec![Span::new(
            rewo_data::lang::format(
                count_key,
                &[e.hover_name(translated), &e.count.to_string()],
            ),
            GRAY_TEXT,
        )]);
    }
    if plan.more > 0 {
        // `.withStyle(ChatFormatting.ITALIC)` — expressible since M52b's span
        // model, and rendered as the italic face by the Velvet pass.
        out.push(vec![Span::new(
            rewo_data::lang::format(more_key, &[&plan.more.to_string()]),
            GRAY_TEXT,
        )
        .italic()]);
    }
    out
}

/// `ItemStack.addDetailsToTooltip`'s advanced block, translated (M66).
///
/// The order and the arguments are [`rewo_gpu::tooltip::advanced_lines`]'s;
/// this resolves the two keys and the registry id, which is a **literal** and
/// therefore never goes through the language file.
pub(crate) fn advanced_tooltip_lines(
    lines: &[rewo_gpu::tooltip::AdvancedLine],
    registry_key: &str,
    lang: &rewo_data::lang::Language,
) -> Vec<rewo_gpu::tooltip::Line> {
    use rewo_gpu::tooltip::{AdvancedLine, Span, DARK_GRAY};
    lines
        .iter()
        .filter_map(|l| match l {
            AdvancedLine::Durability { remaining, max } => {
                let key = lang.get("item.durability")?;
                Some(vec![Span::new(
                    rewo_data::lang::format(key, &[&remaining.to_string(), &max.to_string()]),
                    WHITE_TEXT,
                )])
            }
            AdvancedLine::RegistryId => {
                Some(vec![Span::new(registry_key.to_string(), DARK_GRAY)])
            }
            AdvancedLine::Components { count } => {
                let key = lang.get("item.components")?;
                Some(vec![Span::new(
                    rewo_data::lang::format(key, &[&count.to_string()]),
                    DARK_GRAY,
                )])
            }
        })
        .collect()
}

/// `PatchedDataComponentMap.has(type)` for a decoded stack (M66).
///
/// The prototype answers first and the patch overrides it, which is what
/// separates an addition from an override for
/// [`rewo_net::item_stack::StackComponents::component_count`] — and is also
/// how `isDamageableItem`'s three `has(...)` calls are resolved.
///
/// `None` means unanswerable: either the item is outside this build's
/// prototype table or the component id has no name in the runtime registry.
pub(crate) fn stack_has_component(
    item: &str,
    component: &str,
    detail: Option<&rewo_net::item_stack::StackDetail>,
    registry: Option<&rewo_data::components::DataComponentRegistry>,
) -> Option<bool> {
    let mut has = rewo_data::item_components_table::prototype_has_component(item, component)?;
    if let (Some(d), Some(reg)) = (detail, registry) {
        for &id in &d.added {
            if reg.name_of(id)? == component {
                has = true;
            }
        }
        for &id in &d.removed {
            if reg.name_of(id)? == component {
                has = false;
            }
        }
    }
    Some(has)
}

/// `ChatFormatting.GRAY` — the container's own preview lines.
const GRAY_TEXT: [f32; 3] = [170.0 / 255.0, 170.0 / 255.0, 170.0 / 255.0];
/// An unstyled tooltip line, which `GuiGraphics.tooltip` draws in white.
const WHITE_TEXT: [f32; 3] = [1.0, 1.0, 1.0];

#[allow(clippy::too_many_arguments)]
fn screen_tooltip(
    inv: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
    lang: &rewo_data::lang::Language,
    enchant_registry: &[rewo_net::enchantment_parse::EnchantmentDef],
    enchant_text: &rewo_data::enchantments::EnchantmentText,
    details: &rewo_net::item_stack::StackDetails,
    component_registry: Option<&rewo_data::components::DataComponentRegistry>,
    flag: rewo_gpu::tooltip::TooltipFlag,
    advance: &[u8; 256],
    glyphs: Option<&mut rewo_gpu::velvet_glyph::GlyphCache>,
    mouse: (f64, f64),
    (w, h): (f32, f32),
) -> Option<(
    rewo_gpu::container::TooltipDraw,
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
)> {
    // A stack on the cursor suppresses the tooltip: vanilla's guard is
    // `hoveredSlot.hasItem() && getCarried().isEmpty()`, so picking something
    // up hides the label of whatever you drag it over.
    if inv.carried().is_some() {
        return None;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui(mouse, w, h);
    let slot = rewo_world::inventory::slot_at(gx, gy)?;
    let stack = inv.menu_slot(slot)?;
    let item_name = items.name(stack.item_id)?;
    let translated = names.get(item_name)?;

    // `getTooltipLines` in vanilla's order: the styled hover name, then the
    // details each component contributes. Rewo produces the three it can read
    // exactly — the name, the lore, and the `Unbreakable` marker.
    //
    let text = inv.text_of(stack);
    // A line is a sequence of styled spans (M52b), not a string and a colour.
    // Vanilla styles per run, and the old model was strictly less: it could
    // not say "italic", so `ItemLore.LORE_STYLE`'s italic was silently
    // dropped. Geometry is unchanged -- the box is still measured from the
    // plain text with the vanilla advances.
    let mut lines: Vec<rewo_gpu::tooltip::Line> = Vec::new();
    lines.push(vec![rewo_gpu::tooltip::Span::new(
        text.and_then(|t| t.name.as_deref())
            .unwrap_or(translated)
            .to_string(),
        rarity_color(stack_rarity(
            Some(item_name),
            text.and_then(|t| t.rarity),
            text.is_some_and(|t| t.is_enchanted),
        )),
    )]);
    if let Some(t) = text {
        // Vanilla's order: the enchantments come before the lore.
        lines.extend(enchantment_lines(
            &t.enchantments,
            enchant_registry,
            enchant_text,
        ));
        for line in &t.lore {
            // `ItemLore.LORE_STYLE` is
            // `Style.EMPTY.withColor(DARK_PURPLE).withItalic(true)`. The
            // colour was already right; the italic had nowhere to live until
            // the span model, so Rewo rendered lore upright.
            lines.push(vec![
                rewo_gpu::tooltip::Span::new(line.clone(), LORE_COLOR).italic(),
            ]);
        }
        if t.unbreakable {
            lines.push(vec![rewo_gpu::tooltip::Span::new(
                "Unbreakable".to_string(),
                UNBREAKABLE_COLOR,
            )]);
        }
    }
    // M66 stage 4: `minecraft:container`'s preview. Vanilla's component
    // tooltips run in `DataComponents` registration order and `container`
    // comes after the lore block, which is where this sits.
    let detail = details.get(stack.components);
    if let Some(d) = detail {
        lines.extend(container_lines(&d.container, items, names, lang));
    }
    // M66 stage 3: the advanced block, last of everything a component adds.
    {
        let has = |c: &str| stack_has_component(item_name, c, detail, component_registry);
        let durability = rewo_gpu::tooltip::DurabilityState {
            damage: stack.damage.unwrap_or(0),
            max: stack
                .max_damage
                .or_else(|| rewo_data::item_props_table::max_damage(item_name))
                .unwrap_or(0),
            has_max_damage: has("minecraft:max_damage").unwrap_or(false),
            has_damage: has("minecraft:damage").unwrap_or(false),
            unbreakable: has("minecraft:unbreakable").unwrap_or(false),
        };
        // `PatchedDataComponentMap.size()`. Both halves can decline — an item
        // outside the prototype table, or a component id with no name — and
        // either way the line is dropped rather than guessed.
        let count = match detail {
            Some(d) => rewo_net::item_stack::StackComponents {
                added: d.added.clone(),
                removed: d.removed.clone(),
                ..Default::default()
            }
            .component_count(
                || rewo_data::item_components_table::prototype_component_count(item_name),
                |id| {
                    let name = component_registry?.name_of(id)?;
                    rewo_data::item_components_table::prototype_has_component(item_name, name)
                },
            ),
            // No patch at all: the merged size is the prototype's.
            None => rewo_data::item_components_table::prototype_component_count(item_name),
        };
        // `display.shows(DataComponents.DAMAGE)` — Rewo does not interpret
        // `minecraft:tooltip_display`, so this is `TooltipDisplay.DEFAULT`,
        // which shows everything. A server that hid the damage line would
        // still see it here.
        let advanced = rewo_gpu::tooltip::advanced_lines(flag, durability, true, count);
        lines.extend(advanced_tooltip_lines(&advanced, item_name, lang));
    }

    // Measure with the font that will DRAW. Once the tooltip renders in
    // Newsreader, sizing it with the bitmap advances measures a font it no
    // longer uses -- which shows up as text spilling out of its box, and is
    // the half of this flip that is easiest to skip.
    let scale = rewo_gpu::hud::gui_scale(w, h);
    let mut glyphs = glyphs;
    let widths: Vec<i32> = match glyphs.as_deref_mut() {
        Some(cache) => lines
            .iter()
            .map(|l| velvet_line_width(cache, l, scale).ceil() as i32)
            .collect(),
        None => lines
            .iter()
            .map(|l| {
                rewo_data::sign_text::width(&rewo_gpu::tooltip::line_text(l), advance).round()
                    as i32
            })
            .collect(),
    };
    let (tw, th) = rewo_gpu::container::tooltip_size(&widths);
    // The positioner works in GUI pixels, and so does the screen size it
    // clamps against — `guiWidth()`/`guiHeight()` are the *scaled* dimensions,
    // not the framebuffer's. Passing raw pixels here would let a tooltip run
    // off the right of a small window before the flip ever triggered.
    let scale = rewo_gpu::hud::gui_scale(w, h);
    let (sw, sh) = ((w / scale) as i32, (h / scale) as i32);
    let (mx, my) = ((mouse.0 / scale as f64) as i32, (mouse.1 / scale as f64) as i32);
    let (tx, ty) = rewo_gpu::container::tooltip_position(sw, sh, mx, my, tw, th);
    // `localY += line.getHeight(font) + (i == 0 ? 2 : 0)` — the gap goes after
    // the **first** line only, which is what separates the name from the
    // details without spacing the details apart.
    let mut y = ty;
    // With the cache present the text goes through the Velvet pass, which is
    // what makes italic lore actually slant. The bitmap path stays as the
    // fallback for a build with no fonts on disk.
    if let Some(cache) = glyphs.as_deref_mut() {
        let mut runs: Vec<rewo_gpu::velvet_text::OwnedRun> = Vec::new();
        let mut ly = ty;
        for (i, spans) in lines.iter().enumerate() {
            // Vanilla's `y` is the line's TOP; Velvet lays out from the
            // BASELINE. Dropping the ascent would raise every tooltip line by
            // most of its own height.
            let ascent = cache
                .metrics(tooltip_key(false, scale))
                .map(|m| m.ascent)
                .unwrap_or(TOOLTIP_TEXT_GUI_PX * scale * 0.75);
            let baseline = ly as f32 * scale + ascent;
            let mut pen = tx as f32 * scale;
            for sp in spans {
                let key = tooltip_key(sp.italic, scale);
                let mut g = Vec::new();
                let adv = cache.layout_run(key, &sp.text, 0.0, (pen, baseline), &mut g);
                pen += adv;
                if !g.is_empty() {
                    runs.push(rewo_gpu::velvet_text::OwnedRun {
                        glyphs: g,
                        color: sp.color,
                        alpha: 1.0,
                    });
                }
            }
            ly += rewo_gpu::container::TOOLTIP_LINE_HEIGHT + if i == 0 { 2 } else { 0 };
        }
        return Some((
            rewo_gpu::container::TooltipDraw {
                pos: (tx, ty),
                size: (tw, th),
                bundle: None,
            },
            Vec::new(),
            runs,
        ));
    }
    let out = lines
        .into_iter()
        .enumerate()
        .map(|(i, spans)| {
            // The bitmap pass takes one colour per line, so the vanilla-font
            // path still draws the first span's. That is a property of THAT
            // pass, not of the model: the styled line already carries every
            // span, and `tooltip::to_velvet_spans` renders it in full through
            // the Velvet type stack. Switching which pass a tooltip uses is a
            // rendering decision, deliberately left open while the HUD's
            // visual direction settles.
            let color = spans.first().map(|s| s.color).unwrap_or([1.0; 3]);
            let line = rewo_gpu::world::OwnedTextLine {
                x: tx as f32 * scale,
                y: y as f32 * scale,
                px: scale,
                color,
                alpha: 1.0,
                shadow: true,
                text: rewo_gpu::tooltip::line_text(&spans),
            };
            y += rewo_gpu::container::TOOLTIP_LINE_HEIGHT + if i == 0 { 2 } else { 0 };
            line
        })
        .collect();
    Some((
        rewo_gpu::container::TooltipDraw {
            pos: (tx, ty),
            size: (tw, th),
            // **The decode exists; the carrier does not.** M61 made the patch
            // reader *keep* `minecraft:bundle_contents` — see
            // `rewo_net::item_stack::StackComponents::bundle_contents`, which
            // returns the stacks as `ItemTemplate`s. What is still missing is
            // a way to get them from there to here.
            //
            // The tooltip reads its stack out of `rewo_world`'s `Inventory`,
            // and neither of that crate's two carriers takes a `Vec` today:
            // `ItemSlot` is `Copy` on purpose (the click arithmetic moves it
            // through a dozen struct-update expressions, and growing it would
            // make every one of those a clone), and `SlotText` — the non-`Copy`
            // side-channel keyed by the component fingerprint — is the natural
            // home but would need its `is_empty` taught the new field, or a
            // bundle carrying *only* `bundle_contents` would be recorded as
            // having no text and dropped. That is the exact bug M42's
            // enchantments hit.
            //
            // Left `None` rather than guessed: `container::bundle_chrome` and
            // `tooltip::bundle_image` are both graded by `inventoryshot`
            // against synthetic bundles, so an empty grid drawn from a full
            // bundle would be a confident wrong answer with a green gate
            // behind it.
            bundle: None,
        },
        out,
        Vec::new(),
    ))
}

fn screen_icons(
    inv: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    trim_materials: &[rewo_net::trim_parse::TrimMaterialDef],
    w: f32,
    h: f32,
) -> (Vec<rewo_gpu::gui_item::GuiItem>, Vec<rewo_gpu::world::OwnedTextLine>) {
    let rects = menu_slot_rects(inv, w, h);
    let (_, _, scale) = rewo_gpu::container::gui_origin(w, h);
    let mut icons = Vec::new();
    let mut labels = Vec::new();
    for (slot, rect) in rects.iter().enumerate() {
        if let Some(stack) = inv.menu_slot(slot) {
            if let Some(icon) = icon_for(items, trim_materials, stack, rect.0, rect.1, rect.2) {
                icons.push(icon);
            }
            labels.extend(count_label(stack, rect.0, rect.1, scale));
        }
    }
    (icons, labels)
}

fn icon_for(
    items: &rewo_data::items::Items,
    trim_materials: &[rewo_net::trim_parse::TrimMaterialDef],
    stack: rewo_world::inventory::ItemSlot,
    x: f32,
    y: f32,
    size: f32,
) -> Option<rewo_gpu::gui_item::GuiItem> {
    let base = items.name(stack.item_id)?;
    // M49: a trimmed stack asks for its variant. `HeldItems::any` falls back to
    // the plain item when the bake has no such variant, so composing here is
    // always safe.
    let model = match stack
        .trim_material
        .and_then(|m| trim_materials.get(m as usize))
    {
        Some(m) => format!("{base}#{}", m.id),
        None => base.to_string(),
    };
    Some(rewo_gpu::gui_item::GuiItem {
        model,
        x,
        y,
        size,
        // `ItemStack.hasFoil()` (M43). Every slot icon goes through this one
        // constructor, so the hotbar, the screen and the cursor stack all get
        // the glint from the same place.
        glint: stack.enchanted,
    })
}

/// `GuiGraphicsExtractor.itemCount` — bottom-right of the slot, and **only
/// when the count is not one**, which is why a single sword shows no label.
fn count_label(
    stack: rewo_world::inventory::ItemSlot,
    x: f32,
    y: f32,
    scale: f32,
) -> Option<rewo_gpu::world::OwnedTextLine> {
    if stack.count == 1 {
        return None;
    }
    let text = stack.count.to_string();
    // Vanilla measures the string; the digits are a uniform 6 px including
    // their one-pixel gap, and the trailing gap is not part of the width.
    let width = text.chars().count() as f32 * 6.0 - 1.0;
    Some(rewo_gpu::world::OwnedTextLine {
        // `x + 19 - 2 - width`, `y + 6 + 3`.
        x: x + (17.0 - width) * scale,
        y: y + 9.0 * scale,
        px: scale,
        color: [1.0, 1.0, 1.0],
        alpha: 1.0,
        shadow: true,
        text,
    })
}

/// `AbstractContainerScreen.extractCarriedItem` — the cursor stack, offset by
/// half a slot so it sits under the pointer rather than beside it.
fn carried_icon(
    inv: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    trim_materials: &[rewo_net::trim_parse::TrimMaterialDef],
    mouse: (f64, f64),
    w: f32,
    h: f32,
) -> Option<(
    rewo_gpu::gui_item::GuiItem,
    Option<rewo_gpu::world::OwnedTextLine>,
)> {
    let stack = inv.carried()?;
    let (_, _, scale) = rewo_gpu::container::gui_origin(w, h);
    let (x, y) = (
        mouse.0 as f32 - 8.0 * scale,
        mouse.1 as f32 - 8.0 * scale,
    );
    let icon = icon_for(items, trim_materials, stack, x, y, 16.0 * scale)?;
    Some((icon, count_label(stack, x, y, scale)))
}

/// Resolve an item id into the two facts the click arithmetic needs.
///
/// `None` for an id the registry does not contain, which makes the whole click
/// decline rather than predicting against a guessed stack cap.
fn item_props(
    items: &rewo_data::items::Items,
    id: i32,
) -> Option<rewo_world::inventory::ItemProps> {
    use rewo_data::item_props_table::{equip_slot, max_stack_size, EquipSlot};
    use rewo_world::inventory::ArmorPiece;
    let name = items.name(id)?;
    Some(rewo_world::inventory::ItemProps {
        max_stack: max_stack_size(name),
        // M91 — the furnace quick-move's two predicates, resolved here because
        // this is where the numeric id becomes a name.
        is_fuel: rewo_data::fuel_table::is_fuel(name),
        smeltable: [
            rewo_data::smelting_table::accepts(10, name) == Some(true),
            rewo_data::smelting_table::accepts(14, name) == Some(true),
            rewo_data::smelting_table::accepts(22, name) == Some(true),
        ],
        equips: match equip_slot(name) {
            Some(EquipSlot::Head) => Some(ArmorPiece::Head),
            Some(EquipSlot::Chest) => Some(ArmorPiece::Chest),
            Some(EquipSlot::Legs) => Some(ArmorPiece::Legs),
            Some(EquipSlot::Feet) => Some(ArmorPiece::Feet),
            // `body` and `saddle` are animal equipment and `mainhand`/`offhand`
            // are not armour slots, so none of them satisfies an `ArmorSlot`.
            _ => None,
        },
    })
}

/// Handle a click on the open screen: predict, send, then apply locally.
///
/// Sending before applying, so a send that fails cannot leave the screen
/// showing a move the server never heard about. The packet carries the
/// prediction and the *pre-click* state id, so the order is invisible on the
/// wire — it only decides what happens when the socket is broken.
///
/// A click that cannot be predicted is dropped entirely rather than sent with
/// an empty changed-slot map, which the server would reject and answer with a
/// full resynchronisation.
/// What the player did to the hovered slot (M35, M39, M40).
///
/// One enum rather than a pile of booleans because each variant is a
/// **different `ContainerInput`**, not a modifier on one — `doClick` branches
/// on the input before it ever reads the button, so shift-clicking is not
/// "a click with shift" in the protocol's terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotAction {
    /// A plain click. `0` primary, `1` secondary.
    Pickup(i8),
    /// Shift-click.
    QuickMove,
    /// A number key (an **inventory** index, `0..9`) or F (`40`).
    Swap(i32),
    /// Q, or Ctrl+Q for the whole stack.
    Throw { all: bool },
    /// The second click of a double click.
    PickupAll,
}

/// The screen's own keys (M40): a number key or F swaps, Q throws.
///
/// `AbstractContainerScreen.checkHotbarKeyPressed` maps the nine hotbar
/// keys and the off-hand key to a `SWAP` whose button is the **inventory
/// index**, and `keyPressed` maps the drop key to a `THROW` whose button
/// distinguishes one item from the stack.
fn screen_key_action(key: PhysicalKey, ctrl: bool) -> Option<SlotAction> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    if let Some(n) = digit_key(code) {
        return Some(SlotAction::Swap(n as i32));
    }
    match code {
        // `Inventory.SLOT_OFFHAND`, the one button outside `0..9`.
        KeyCode::KeyF => Some(SlotAction::Swap(
            rewo_world::inventory::SWAP_OFFHAND_BUTTON,
        )),
        KeyCode::KeyQ => Some(SlotAction::Throw { all: ctrl }),
        _ => None,
    }
}

/// A quick-craft drag in progress (M40).
///
/// The drag is **three packets**, not one: a begin, one add per slot, and an
/// end that carries the whole changed-slot map. Only the end predicts
/// anything, so this holds the slots as they are touched and does the
/// arithmetic once at release.
#[derive(Clone, Debug, Default)]
pub(crate) struct DragState {
    /// `quickcraftType` — 0 spreads the stack evenly, 1 puts one in each.
    kind: i32,
    /// Every slot the cursor has crossed, in order. Filtered at release, not
    /// here, so the same list can be re-tested if the cursor stack changed.
    touched: Vec<usize>,
    active: bool,
}

impl DragState {
    /// Begin a drag with the button that started it. Returns the phase-0
    /// packet to send.
    fn begin(&mut self, button: i8) -> i32 {
        // Left drag spreads, right drag places one each — vanilla reads the
        // button at `mouseDragged` time, not at press.
        self.kind = if button == 0 {
            rewo_world::inventory::QUICK_CRAFT_SPLIT
        } else {
            rewo_world::inventory::QUICK_CRAFT_ONE
        };
        self.touched.clear();
        self.active = true;
        self.kind
    }

    fn add(&mut self, slot: usize) -> bool {
        if !self.active || self.touched.contains(&slot) {
            return false;
        }
        self.touched.push(slot);
        true
    }
}

/// Run a drag to its end: send the accepted adds, then the end packet.
///
/// The three phases are sent here rather than as they happen because a slot
/// only joins the drag if the server would accept it, and that test needs the
/// cursor stack — which is unchanged throughout, so testing once at release
/// gives the same answer while keeping the state machine in one place.
fn finish_drag(
    session: &mut PlaySession,
    items: &rewo_data::items::Items,
    drag: &mut DragState,
) {
    let touched = std::mem::take(&mut drag.touched);
    let kind = drag.kind;
    drag.active = false;
    if touched.is_empty() || session.inventory.carried().is_none() {
        return;
    }
    let props = |id: i32| item_props(items, id);
    let accepted = session.inventory.quick_craft_accepts(&touched, kind, &props);
    if accepted.is_empty() {
        return;
    }
    // A one-slot drag is a plain click in disguise — vanilla resets the
    // quick-craft state and re-dispatches it as `PICKUP`, so sending it as a
    // drag would desync a prediction the server never makes.
    if let Some((slot, button)) =
        rewo_world::inventory::Inventory::quick_craft_is_pickup(&accepted, kind)
    {
        if let Some(p) = session.shown_menu_mut().click_pickup(slot as i32, button, &props) {
            if session.container_click_input(&p, 0).is_ok() {
                session.shown_menu_mut().apply_prediction(&p);
            }
        }
        return;
    }
    let Some(end) = session.shown_menu_mut().click_quick_craft(&accepted, kind, &props) else {
        return;
    };
    use rewo_world::inventory::Inventory as Inv;
    let input = rewo_world::inventory::CONTAINER_INPUT_QUICK_CRAFT;
    let carried = session.inventory.carried();
    // Phase 0 and the phase-1 adds change nothing; only their button and slot
    // carry information.
    let phase = |slot: i16, header: i32| rewo_world::inventory::ClickPrediction {
        slot,
        button: Inv::quick_craft_button(kind, header),
        changed: Vec::new(),
        carried,
    };
    if session
        .container_click_input(&phase(rewo_world::inventory::QUICK_CRAFT_NO_SLOT, 0), input)
        .is_err()
    {
        return;
    }
    for &slot in &accepted {
        if session
            .container_click_input(&phase(slot as i16, 1), input)
            .is_err()
        {
            return;
        }
    }
    if session.container_click_input(&end, input).is_ok() {
        session.shown_menu_mut().apply_prediction(&end);
    }
}

fn click_screen(
    session: &mut PlaySession,
    items: &rewo_data::items::Items,
    screen: &ScreenState,
    action: SlotAction,
    w: f32,
    h: f32,
) {
    let Some(slot) = screen.hovered(session.shown_menu().layout(), w, h) else {
        return;
    };
    let props = |id: i32| item_props(items, id);
    use rewo_world::inventory as inv;
    let slot = slot as i32;
    let (input, button, predicted) = match action {
        SlotAction::Pickup(b) => (0, b, session.shown_menu_mut().click_pickup(slot, b, &props)),
        SlotAction::QuickMove => (
            inv::CONTAINER_INPUT_QUICK_MOVE,
            0,
            session.shown_menu_mut().click_quick_move(slot, &props),
        ),
        // The button here is an inventory index, not a menu slot — see
        // `Inventory::click_swap`.
        SlotAction::Swap(index) => (
            inv::CONTAINER_INPUT_SWAP,
            index as i8,
            session.shown_menu_mut().click_swap(slot, index, &props),
        ),
        SlotAction::Throw { all } => {
            let b = i8::from(all);
            (
                inv::CONTAINER_INPUT_THROW,
                b,
                session.shown_menu_mut().click_throw(slot, b, &props),
            )
        }
        SlotAction::PickupAll => (
            inv::CONTAINER_INPUT_PICKUP_ALL,
            0,
            session.shown_menu_mut().click_pickup_all(slot, 0, &props),
        ),
    };
    let _ = button;
    let Some(prediction) = predicted else {
        log::debug!("live: {action:?} on slot {slot} not predictable — not sent");
        return;
    };
    if prediction.changed.is_empty() && prediction.carried == session.shown_menu().carried() {
        // Nothing moved (an empty slot with an empty cursor, or a placement the
        // slot refuses). Vanilla still sends it; there is no reason to.
        return;
    }
    if let Err(e) = session.container_click_input(&prediction, input) {
        log::warn!("live: container_click: {e}");
        return;
    }
    session.shown_menu_mut().apply_prediction(&prediction);
}

/// State the hotbar icons need across frames: the atlas currently uploaded and
/// what it was built for.
pub struct GuiItemState {
    /// The texture set the resident atlas holds, so a hotbar change that needs
    /// no new textures costs nothing.
    resident: Vec<u16>,
    /// Where each resident texture landed, kept alongside so a frame that needs
    /// no new textures does not repack a megabyte of atlas to look them up.
    uv: std::collections::HashMap<u16, [f32; 4]>,
    lights: rewo_gpu::gui_item::ItemLights,
    /// The app's own copy of the baked items. `WorldRenderer` owns one too, but
    /// building the icons needs `&items` while uploading them needs `&mut wr`,
    /// and one copy cannot be both.
    held: rewo_gpu::held::HeldItems,
    /// When this session started, for the glint's wall-clock phase (M43).
    started: std::time::Instant,
    /// `misc/enchanted_glint_item.png` as `(rgba, w, h)`; `None` draws none.
    glint: Option<(Vec<u8>, u32, u32)>,
}

impl GuiItemState {
    pub fn new(baked: &assets::BakedAssets) -> Self {
        Self {
            resident: Vec::new(),
            uv: std::collections::HashMap::new(),
            lights: rewo_gpu::gui_item::ItemLights::default(),
            held: to_gpu_held_items(&baked.held_items),
            started: std::time::Instant::now(),
            glint: baked
                .glint
                .as_ref()
                .map(|i| (i.rgba.clone(), i.w, i.h)),
        }
    }
}

/// Place, shade and upload this frame's hotbar icons.
///
/// Rebuilds the atlas only when the hotbar needs a texture it does not hold —
/// switching between two swords you already carry costs one vertex upload.
fn apply_hotbar_icons(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    session: &PlaySession,
    items: &rewo_data::items::Items,
    state: &mut GuiItemState,
    extent: (f32, f32),
) {
    let held = &state.held;
    let names = hotbar_models(&session.inventory, items, &session.trim_materials);
    let models: Vec<String> = names.iter().flatten().map(|n| n.to_string()).collect();
    let wanted = gui_atlas_wanted(held, &models);

    // Repack only when the hotbar needs a texture the resident atlas does not
    // hold. Switching between two swords you already carry costs one vertex
    // upload; packing every frame would rebuild a megabyte for nothing.
    if wanted != state.resident || !wr.gui_items_ready() {
        let atlas = pack_gui_atlas(held, &wanted);
        if let Err(e) = wr.init_gui_items(gpu, &atlas.rgba, GUI_ATLAS_W, GUI_ATLAS_H) {
            log::warn!("live: gui-item atlas upload failed: {e}");
            return;
        }
        // The glint rides on the item pass, and the item pass is rebuilt
        // whenever its atlas is repacked — so the glint is rebuilt here too.
        if let Some(g) = state.glint.as_ref() {
            if let Err(e) = wr.init_gui_glint(gpu, &g.0, g.1, g.2) {
                log::warn!("live: glint upload failed: {e}");
            }
        }
        // The UVs come from the same packing the atlas was built from, so the
        // two cannot disagree.
        state.uv = atlas.uv;
        state.resident = wanted;
    }
    let slots = rewo_gpu::hud::hotbar_slot_rects(182.0, 22.0, extent.0, extent.1);
    let gui: Vec<rewo_gpu::gui_item::GuiItem> = names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            n.as_ref().map(|name| rewo_gpu::gui_item::GuiItem {
                model: name.clone(),
                x: slots[i].0,
                y: slots[i].1,
                size: slots[i].2,
                glint: session.inventory.hotbar(i).is_some_and(|s| s.enchanted),
            })
        })
        .collect();
    upload_gui_icons(wr, gpu, state, &gui);
    // The nine hotbar stacks' durability bars, in the same rects the icons
    // were placed from.
    let stacks: Vec<_> = (0..rewo_world::inventory::HOTBAR_SIZE)
        .map(|i| (session.inventory.hotbar(i), slots[i]))
        .collect();
    // `!has(UNBREAKABLE)`. No item's *prototype* carries the component in
    // 26.2 — it is only ever patched on — so the patch flag is the whole
    // answer here.
    wr.set_item_bars(item_bars(&stacks, items, |s| {
        session.inventory.text_of(s).is_some_and(|t| t.unbreakable)
    }));
}

/// Place, shade and upload an arbitrary list of icons (M35).
///
/// The screen's 46 slots and the hotbar's nine go through exactly this, which
/// is why the pass never learns which it is drawing.
fn apply_gui_icons(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    state: &mut GuiItemState,
    icons: &[rewo_gpu::gui_item::GuiItem],
) {
    let names: Vec<String> = icons.iter().map(|i| i.model.clone()).collect();
    let wanted = gui_atlas_wanted(&state.held, &names);
    if wanted != state.resident || !wr.gui_items_ready() {
        let atlas = pack_gui_atlas(&state.held, &wanted);
        if let Err(e) = wr.init_gui_items(gpu, &atlas.rgba, GUI_ATLAS_W, GUI_ATLAS_H) {
            log::warn!("live: gui-item atlas upload failed: {e}");
            return;
        }
        // The glint rides on the item pass, and the item pass is rebuilt
        // whenever its atlas is repacked — so the glint is rebuilt here too.
        if let Some(g) = state.glint.as_ref() {
            if let Err(e) = wr.init_gui_glint(gpu, &g.0, g.1, g.2) {
                log::warn!("live: glint upload failed: {e}");
            }
        }
        state.uv = atlas.uv;
        state.resident = wanted;
    }
    upload_gui_icons(wr, gpu, state, icons);
}

fn upload_gui_icons(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    state: &GuiItemState,
    icons: &[rewo_gpu::gui_item::GuiItem],
) {
    let verts = rewo_gpu::gui_item::build_vertices(&state.held, icons, &state.lights, &|t| {
        state.uv.get(&t).copied()
    });
    // The glint's phase is wall-clock, not the game tick: vanilla reads
    // `Util.getMillis()` directly, so it keeps scrolling on a paused screen
    // and does not stutter with the tick rate (M43).
    let millis = state.started.elapsed().as_secs_f64() * 1000.0;
    let glint = rewo_gpu::gui_item::build_glint_vertices(&state.held, icons, millis);
    if let Err(e) = wr.set_gui_items_with_glint(gpu, &verts, &glint) {
        log::warn!("live: gui-item upload failed: {e}");
    }
}
