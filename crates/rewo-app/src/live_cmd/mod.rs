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
use rewo_net::sound_engine::{ChannelSink, LiveSounds};
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

mod app;
mod frame;
mod input;
mod screen_pumps;
mod block_entities;
pub(crate) use block_entities::*;
mod entities;
pub(crate) use entities::*;
mod gui;
pub(crate) use gui::*;
mod headless;
pub(crate) use headless::*;
mod hud;
pub(crate) use hud::*;
mod render_check;
pub(crate) use render_check::*;
mod icons;
pub(crate) use icons::*;
mod menus;
pub(crate) use menus::*;
mod recipe_book;
pub(crate) use recipe_book::*;
mod slots;
pub(crate) use slots::*;
mod tooltips;
pub(crate) use tooltips::*;
mod screens;
pub(crate) use screens::*;
mod sky;
pub(crate) use sky::*;
#[cfg(test)]
mod tests;
mod view;
pub(crate) use view::*;
mod world_fx;
pub(crate) use world_fx::*;

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
    /// Open an audio device and actually play sounds (M143). Off by default,
    /// and a no-op unless the binary was built `--features audio` — a default
    /// build links no audio stack at all, which is what keeps the 34 gates
    /// free of one. Also read from `REWO_AUDIO=1`.
    ///
    /// **A successful open does not mean anything is audible.** Absent, muted,
    /// exclusive-mode and unplugged devices are indistinguishable from inside
    /// the process, so the listening pass is a human's — see
    /// `REWO_AUDIO_PLAN.md` §4.
    #[arg(long, default_value_t = false)]
    audio: bool,
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

pub fn run(args: LiveArgs) -> Result<(), String> {
    if args.render_check && !cfg!(feature = "gates") {
        return Err("--render-check needs a build with `--features gates`".into());
    }
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
    // M92 — the beacon's six effect icons, by NAME from the report-backed
    // table (M92c). Derived here beside `spears` because it is the same kind
    // of fact: a small constant slice of `GameData` the render needs.
    let beacon_effects = BeaconEffectIds::resolve(&data.mob_effects);
    // Shared with the entity collector for held-item id → name (M22).
    let items = std::sync::Arc::new(data.items.clone());
    let blocks = std::sync::Arc::new(data.blocks.clone());
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
    session.block_physics = baked.physics.clone();
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
    session.bee_type_id = data.entity_types.id_of("minecraft:bee");
    session.guardian_type_id = data.entity_types.id_of("minecraft:guardian");
    session.elder_guardian_type_id = data.entity_types.id_of("minecraft:elder_guardian");
    session.sniffer_type_id = data.entity_types.id_of("minecraft:sniffer");
    session.happy_ghast_type_id = data.entity_types.id_of("minecraft:happy_ghast");
    session.nautilus_type_ids = [
        data.entity_types.id_of("minecraft:nautilus"),
        data.entity_types.id_of("minecraft:zombie_nautilus"),
    ];
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
    session.bubble_column_drag = baked.bubble_column_drag.clone();
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
    // M93y — the recipe book's display registries, supplied for the same
    // reason: built-in registries live in the report, not on the wire.
    session.recipe_display_ids = Some(data.recipe_display_ids);
    // M113 — the `command_argument_type` registry, for the same reason and
    // from the same place: a built-in registry lives in the report, not on the
    // wire, and the Brigadier tree cannot be read past its first
    // non-singleton argument without it.
    session.command_argument_types = Some(data.command_argument_types.clone());
    // M125 — the language table, so a `translate` component in chat resolves.
    // Vanilla reaches `Language.getInstance()`, a global; this is the same
    // handover the two lines above use, and the session is where chat is
    // decoded.
    session.lang = Some(std::sync::Arc::new(baked.lang.clone()));
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
            let settle = crate::knobs::var("REWO_SETTLE")
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
                beacon_effects,
                want_validation,
                out,
                settle,
                dirt_item,
                args.pack.clone(),
                args.gamma,
                args.darkness_effect_scale,
                build_sounds(
                    &args.version,
                    &data.sound_events,
                    args.render_check,
                    wants_audio(&args),
                ),
            )
        }
        _ => {
            // Built before `args` moves into the call — `build_sounds` borrows
            // `args.version`, and arguments are evaluated left to right.
            let sounds = build_sounds(
                &args.version,
                &data.sound_events,
                args.render_check,
                wants_audio(&args),
            );
            run_windowed(
                session,
                baked,
                etypes,
                stat_registries,
                spears,
                chest_states,
                sign_states,
                bow_item,
                items,
                blocks,
                beacon_effects,
                args,
                want_validation,
                dirt_item,
                sounds,
                username,
            )
        }
    }
}

#[derive(Default)]
struct Keys {
    w: bool,
    a: bool,
    s: bool,
    d: bool,
    jump: bool,
    sneak: bool,
    sprint: bool,
    /// `Options.keyPlayerList` — GLFW **258**, Tab (`Options.java:671`).
    ///
    /// **A hold, not a toggle.** `Hud.extractTabList`'s gate is
    /// `keyPlayerList.isDown()`, read fresh every frame, so the list is on
    /// screen exactly while the key is down. Nothing latches it.
    tab_list: bool,
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
    /// The beacon's six effect icons (M92), by name from the report.
    beacon_effects: BeaconEffectIds,
    /// Item registry, for id → name when resolving held models (M22).
    items: std::sync::Arc<rewo_data::items::Items>,
    /// Block registry, for the command suggester's ids and property table
    /// (M119). Held here rather than reached through the bake because the
    /// bake is *taken* when the window opens — the same reason `equipment` is
    /// cloned out.
    blocks: std::sync::Arc<rewo_data::blocks::Blocks>,
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
    /// Whether left alt is held (M93t). Only the edit box reads it, and only
    /// to REFUSE: `isCopy` and friends require alt up, so Ctrl+Alt+C is not a
    /// copy and falls through to the screen.
    alt: bool,
    /// An in-process clipboard (M93t).
    ///
    /// **Not the OS clipboard.** Rewo pulls in no clipboard crate and `winit`
    /// exposes none, so copy/cut/paste are exact against each other and
    /// isolated from the desktop. Swapping in a real one is a change at this
    /// one field.
    clipboard: String,
    /// `ChatScreen` (M110), when it is open.
    ///
    /// `Option` rather than a flag on the screen framework because it owns an
    /// `EditBox` and a history cursor, and because vanilla's own model is a
    /// screen instance rather than a mode — `ChatComponent.isChatFocused()` is
    /// `gui.screen() instanceof ChatScreen`.
    chat_screen: Option<rewo_world::chat_screen::ChatScreen>,
    /// `ChatComponent.latestDraft`, which outlives the screen that made it.
    chat_draft: Option<rewo_world::chat_screen::Draft>,
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
    /// M131 — vanilla's sound model, driven once per client tick. Without
    /// `--audio` it opens no device and makes no noise; what it does is resolve
    /// every sound the server asks for, through the real registry, the real
    /// `sounds.json` index and the real channel budget. `LiveSounds::stats` is
    /// the readout, and `sink_diagnostics` is the backend's.
    sounds: LiveSounds,
    /// M143 — the last backend counters logged, so a change is reported once
    /// rather than every tick.
    ///
    /// **The instrument for a listening pass.** A device that will not open is
    /// an error at startup; everything after that fails silently by nature, and
    /// these four numbers are what separate "the sound never resolved" from
    /// "the ring filled because the callback stopped" from "it all arrived and
    /// the speakers are muted" — which are indistinguishable to a listener.
    last_audio: rewo_net::sound_engine::SinkDiagnostics,
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
    /// The local player's profile name, which IS its scoreboard name
    /// (`ScoreHolder.getScoreboardName`). The sidebar needs it to ask which
    /// team the viewer is on, and the answer changes which objective is
    /// shown — so this is not a cosmetic label.
    username: String,
    /// `Hud.isHidden()` — F1 (M70). Vanilla's is a plain `toggle()` on press,
    /// with none of F3's modifier dance, and it starts `false`.
    ///
    /// Rewo consumes it only where vanilla's *label* path does: it suppresses
    /// floating nametags and health bars on un-teamed entities. Vanilla also
    /// hides the whole GUI layer from it (`guiRenderState.isHudHidden`);
    /// hiding Rewo's hotbar/hearts/F3 block is a separate concern this
    /// milestone deliberately leaves alone, and is recorded as open.
    hud_hidden: bool,
    /// `PlayerTabOverlay.healthStates` — the per-profile blink clock (M155).
    ///
    /// **On the app rather than the view**, because a `HealthState` is
    /// meaningful only across frames: a fresh one is seeded with its own value
    /// and has nothing to catch up to, so a map rebuilt each frame would never
    /// blink at all.
    ///
    /// Cleared when the list is hidden, which is `reset()` — and is why a
    /// blink does not survive closing and reopening the tab list.
    tab_health: std::collections::HashMap<u128, rewo_gpu::tab_list::HealthState>,
    /// `options.txt`, loaded once at start-up (M157).
    ///
    /// **Read once rather than per frame**, which is vanilla's shape too: an
    /// `OptionInstance` is a field the game holds, not a file it re-reads. The
    /// one consequence worth knowing is that an external edit while the client
    /// is running does not take effect, which is also true of vanilla.
    options: rewo_net::options::Options,
    /// F3 is held — the `keyDebugModifier` half (M66).
    f3_down: bool,
    /// `usedDebugKeyAsModifier` — a chord fired while F3 was down, so its
    /// release must not also toggle the overlay.
    f3_used_as_modifier: bool,
    /// `Options.advancedItemTooltips` — F3+H (M66). Vanilla persists it to
    /// `options.txt`; Rewo has no options file, so it resets each session.
    advanced_tooltips: bool,
    /// M92 — whether the second (brewing-stand) injection has happened.
    brewing_injected: bool,
    /// M94 — whether `--render-check` has opened the recipe book yet.
    book_injected: bool,
    /// M104 — whether it has injected the which-of-these overlay yet.
    book_overlay_injected: bool,
    /// M94 — whether the crafting-table injection has happened.
    book_menu_injected: bool,
    /// M88 — whether `--render-check` has injected its container open yet.
    /// Latched so the inject happens once rather than every frame past the
    /// threshold, which would re-open the menu and reset its state each frame.
    container_injected: bool,
    sign_editor_injected: bool,
    /// M132 — whether the scoreboard sidebar injection has happened.
    sidebar_injected: bool,
    /// M151 — whether the tab-list player injection has happened.
    tab_list_injected: bool,
    /// M162 — the two sound-carrying packet tails, injected once.
    sound_tails_injected: bool,
    /// M166 — whether the PLAY-state `resource_pack_push` has been
    /// injected. Its own flag rather than riding another block's: the
    /// claim is that the play arm exists at all, and sharing a trigger
    /// would make it disappear with whatever it shared.
    play_pack_injected: bool,
    /// Whether the two-of-one-kind mob injection has happened (r54).
    crowd_injected: bool,
    /// M172 — the open written-book reader, `None` when no book is up. The
    /// app-side view state the frame's render arm keys on (the M84 pattern);
    /// the framework `Screen` in `screens` carries only the Done button.
    book: Option<rewo_world::book_view_screen::BookViewScreen>,
    /// M174 — the sign editor's app-side state, `None` when closed. Same
    /// one-slot discipline as the book, with one difference: anything that
    /// REPLACES the editor's screen still COMMITS the edit, because
    /// vanilla's `Gui.setScreen` calls `removed()` on the outgoing screen
    /// unconditionally and `removed()` is where the packet lives.
    sign_edit: Option<SignEditView>,
    /// M173 — which options page is open, `None` when none. The app-side
    /// view state the frame's render arm keys on (the M84 pattern) — the
    /// options were model-only end to end until now (the pause OPTIONS
    /// button logged "not implemented").
    options_view: Option<rewo_world::options_screen::OptionsPage>,
    /// M173 — the slider being dragged, if any. App-side (the stonecutter
    /// pattern): armed by the press, driven from CursorMoved, cleared on
    /// release. UNLIKE the stonecutter's grab, a slider press CONSUMES.
    options_drag: Option<rewo_world::screen::WidgetId>,
    /// M179 — the advancements screen's drag machine (`activeButton` +
    /// `isScrolling`, [`crate::advancements_view::AdvDrag`]). Not an Option:
    /// the machine is total, and the screen-open checks live at the call
    /// sites where they already exist.
    adv_drag: crate::advancements_view::AdvDrag,
    /// M169 — the injected horse + saddle + set_passengers that mounts the bot.
    jump_injected: bool,
    leash_injected: bool,
    /// M172 — the injected written book + open_book (r61). NOT M94's
    /// `book_injected`, which is the RECIPE book's force-open flag.
    book_view_injected: bool,
    /// M173 — the staged walk through the options pages (r62).
    options_staged: u8,
    /// Whether `--render-check` has force-opened the chat screen yet (M110).
    chat_injected: bool,
    /// Whether it has typed a `/`-command yet (M116).
    command_injected: bool,
    /// Whether it has typed a coordinate command yet (M120).
    coords_injected: bool,
    /// M124 — whether the `scoreboard objectives setdisplay ` typing has run.
    literal_table_injected: bool,
    /// M134 — whether the unknown-command typing has run.
    bad_command_injected: bool,
    /// `CommandSuggestions.currentParse` (M117) — the parse the syntax
    /// highlighting reads, cached against the text it was made from.
    ///
    /// Vanilla invalidates on
    /// `!currentParse.getReader().getString().equals(command)`, which is the
    /// same rule as "recompute when the text changed" — so the cache is an
    /// optimisation and not a behaviour, and keying it on the string keeps it
    /// that way.
    chat_parse: Option<(String, rewo_net::dispatcher::ParseResults)>,
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
    /// The advancements screen's app-side state (M178) — `None` when shut.
    advancements: Option<crate::advancements_view::AdvancementsView>,
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
    blocks: std::sync::Arc<rewo_data::blocks::Blocks>,
    beacon_effects: BeaconEffectIds,
    args: LiveArgs,
    want_validation: bool,
    dirt_item: Option<i32>,
    // M131 — the sound model. Silent by construction; see `build_sounds`.
    sounds: LiveSounds,
    // The local player's profile name — the scoreboard name M132's sidebar
    // asks the team map about.
    username: String,
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
    if args.render_check && crate::knobs::var_os("REWO_FORCE_WEATHER").is_none() {
        std::env::set_var("REWO_FORCE_WEATHER", "1.0");
    }
    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let pool = MeshPool::new(MeshTables {
        render: baked.render.clone(),
        models: baked.models.clone(),
        // M164 — without this the windowed client meshes no waterlogged water
        // at all, and looks EXACTLY as it did before. `r48` is what asks.
        fluid: baked.fluid.clone(),
        cull: baked.cull.clone(),
    })?;
    let mut app = LiveApp {
        tab_health: std::collections::HashMap::new(),
        options: load_options(),
        blocks,
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
        beacon_effects,
        items,
        pool,
        weather: None,
        gui_items: None,
        screen: ScreenState::default(),
        stat_registries: std::sync::Arc::new(stat_registries),
        stats: None,
        advancements: None,
        hand: None,
        shift: false,
        ctrl: false,
        alt: false,
        clipboard: String::new(),
        chat_screen: None,
        chat_draft: None,
        chat_injected: false,
        command_injected: false,
        coords_injected: false,
        literal_table_injected: false,
        bad_command_injected: false,
        sidebar_injected: false,
        tab_list_injected: false,
        sound_tails_injected: false,
        play_pack_injected: false,
        crowd_injected: false,
        book: None,
        sign_edit: None,
        options_view: None,
        options_drag: None,
        adv_drag: crate::advancements_view::AdvDrag::default(),
        jump_injected: false,
        leash_injected: false,
        book_view_injected: false,
        options_staged: 0,
        username,
        chat_parse: None,
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
        sounds,
        last_audio: rewo_net::sound_engine::SinkDiagnostics::default(),
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
        sign_editor_injected: false,
        book_injected: false,
        book_overlay_injected: false,
        book_menu_injected: false,
        brewing_injected: false,
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

/// Auto GUI scale (vanilla: largest integer fitting a ~320×240 base).
///
/// Delegates to [`rewo_gpu::hud::gui_scale`] rather than repeating its body.
/// This was a third copy of that expression, and "it is the same expression"
/// was a claim the plan had to make in prose — which is how M135's four
/// producers came to multiply by it a second time on top of the pass. One
/// definition makes the identity a fact instead.
fn gui_px(w: u32, h: u32) -> f32 {
    rewo_gpu::hud::gui_scale(w as f32, h as f32)
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
        // M93t — what `EditBox.keyPressed` switches on, beyond the arrows this
        // already had. The letters are for its four shortcuts; GLFW's letter
        // codes are ASCII, which is why `KeyA` is 65 and not a table entry.
        KeyCode::Backspace => 259,
        KeyCode::Delete => 261,
        KeyCode::Home => 268,
        KeyCode::End => 269,
        KeyCode::KeyA => 65,
        KeyCode::KeyC => 67,
        KeyCode::KeyV => 86,
        KeyCode::KeyX => 88,
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

#[cfg(test)]
mod m93m_beacon {
    use super::*;
    use rewo_world::menu::Menus;
    use rewo_world::menu_screen::BeaconEffect;

    /// A beacon menu with the given data slots.
    fn beacon(container_id: i32, data: &[(i16, i16)]) -> Menus {
        let mut m = Menus::new();
        assert!(m.apply_open_screen(container_id, 9, "B".into()));
        for &(id, v) in data {
            assert!(m.apply_set_data(container_id, id, v));
        }
        m
    }

    /// The six ids as the report would give them, so `of`/`id_of` round-trip.
    fn ids() -> BeaconEffectIds {
        BeaconEffectIds(std::array::from_fn(|i| Some(100 + i as i32)))
    }

    #[test]
    fn a_click_survives_a_frame_but_not_a_data_write() {
        // THE rule, and it reads as a bug until you see the listener:
        // `ContainerListener.dataChanged` re-reads BOTH effects on ANY slot
        // id, so a pyramid growing under you discards an unconfirmed pick.
        // Vanilla's behaviour, not something to design around.
        let mut sc = ScreenState::default();
        let m = beacon(1, &[(0, 4)]);
        let open = m.open().unwrap();
        let first = beacon_live(&mut sc, open, &ids());
        assert_eq!(first.primary, None);

        // A click moves the screen's own copy.
        sc.beacon.as_mut().unwrap().choice.primary = Some(BeaconEffect::ALL[0]);
        let kept = beacon_live(&mut sc, open, &ids());
        assert_eq!(kept.primary, Some(BeaconEffect::ALL[0]), "it survives a frame");

        // ...and a data write of ANY slot re-seeds it from the menu.
        let mut m2 = m.clone();
        assert!(m2.apply_set_data(1, 0, 3));
        let after = beacon_live(&mut sc, m2.open().unwrap(), &ids());
        assert_eq!(
            after.primary, None,
            "a write to the LEVELS slot still clobbers the pick"
        );
    }

    #[test]
    fn a_new_container_re_seeds_even_at_the_same_write_count() {
        let mut sc = ScreenState::default();
        let a = beacon(1, &[]);
        beacon_live(&mut sc, a.open().unwrap(), &ids());
        sc.beacon.as_mut().unwrap().choice.primary = Some(BeaconEffect::ALL[2]);
        // A different beacon, opened with the same number of data writes.
        let b = beacon(2, &[]);
        let after = beacon_live(&mut sc, b.open().unwrap(), &ids());
        assert_eq!(after.primary, None, "a new menu is a new beacon");
    }

    #[test]
    fn levels_and_payment_are_the_MENUS_every_frame_not_the_screens() {
        // `updateStatus(levels)` is handed the menu's value each time and
        // `hasPayment()` reads the slot, so a payment arriving mid-selection
        // must light Confirm WITHOUT disturbing the pick.
        let mut sc = ScreenState::default();
        let m = beacon(1, &[(0, 4)]);
        beacon_live(&mut sc, m.open().unwrap(), &ids());
        sc.beacon.as_mut().unwrap().choice.primary = Some(BeaconEffect::ALL[1]);
        // Stale copies that must NOT leak through.
        sc.beacon.as_mut().unwrap().choice.levels = 0;
        sc.beacon.as_mut().unwrap().choice.has_payment = true;
        let live = beacon_live(&mut sc, m.open().unwrap(), &ids());
        assert_eq!(live.levels, 4, "levels come from the menu");
        assert!(!live.has_payment, "and so does the payment");
        assert_eq!(
            live.primary,
            Some(BeaconEffect::ALL[1]),
            "while the pick is still the screen's"
        );
    }

    #[test]
    fn the_effect_id_lookup_round_trips() {
        // `id_of` is new and only used by `set_beacon`; if it disagreed with
        // `of`, the packet would name a different effect from the one lit.
        let e = ids();
        for i in 0..6 {
            let eff = BeaconEffect::ALL[i];
            let id = e.id_of(eff).expect("resolvable");
            assert_eq!(e.of(id), Some(eff), "{eff:?}");
        }
    }
}

/// M131 — the sound model's live constructor.
#[cfg(test)]
mod m131_sounds {
    use super::{attach_backend, build_sounds};

    /// A backend that does nothing, so the success path is reachable without a
    /// device. Every method is a no-op; the claim is only that it lands.
    struct Nothing;

    impl rewo_net::sound_engine::ChannelSink for Nothing {
        fn submit(&mut self, _: u32, _: &rewo_net::sound_engine::ChannelCall) {}
        fn release(&mut self, _: u32) {}
        fn set_listener(&mut self, _: rewo_net::sound_engine::ListenerTransform) {}
        fn stopped(&self, _: u32) -> Option<bool> {
            None
        }
    }

    /// **M143 — an opened backend is attached, and a failed one is not.**
    ///
    /// The half `build_sounds` can never be tested on: whether `open` succeeds
    /// depends on a real device and no test here opens one. Taking the resolved
    /// `Result` is what makes the decision reachable, and both directions are
    /// asserted because a function that attached unconditionally, or never,
    /// would pass a one-sided version of this.
    #[test]
    fn an_opened_backend_is_attached_and_a_failed_one_leaves_the_client_silent() {
        let registry = rewo_data::sound_events::SoundEvents::default();
        let empty = rewo_data::sounds_json::SoundsIndex::new();

        let mut live = rewo_net::sound_engine::LiveSounds::new(empty.clone(), registry.clone());
        attach_backend(&mut live, Err("no device".into()));
        assert!(!live.has_sink(), "a failed open must leave the client silent");

        let mut live = rewo_net::sound_engine::LiveSounds::new(empty, registry);
        attach_backend(&mut live, Ok(Box::new(Nothing)));
        assert!(live.has_sink());
    }

    /// The one thing about the live wiring a unit test can reach.
    ///
    /// What it does NOT reach is that `run_headless` and `LiveApp::frame`
    /// actually *call* `LiveSounds::drive`: those are composition roots in a
    /// binary crate with no seam, and only `live --render-check` could see
    /// them. Deleting either call site survives the whole suite — measured
    /// with the mutation battery, not assumed.
    #[test]
    fn build_sounds_produces_a_system_that_can_resolve_a_real_event() {
        let Some(paths) = rewo_data::DataPaths::for_version("26.2") else {
            eprintln!("SKIP: no config dir");
            return;
        };
        if !paths.registries_json().exists() {
            eprintln!("SKIP: no datagen report");
            return;
        }
        let registry =
            rewo_data::sound_events::SoundEvents::load(&paths.registries_json()).expect("registry");
        let live = build_sounds("26.2", &registry, false, false);
        // M143 — `audio: false` is the whole suite's path and every gate's, and
        // it must attach no backend at all. Not merely "no device opened": a
        // client with a sink but no device answers `stopped()` from the sink,
        // which is a different reclaim clock, so the silent path has to be the
        // silent path exactly.
        assert!(!live.has_sink(), "the default path must open nothing");
        // The registry half: the id table came across intact, and id 0 is the
        // one that separates a `protocol_id` table from an alphabetised one.
        assert_eq!(
            live.registry.name(0),
            Some("minecraft:entity.allay.ambient_with_item")
        );
        // The SKIP must be decided by the **store on disk**, not by what
        // `build_sounds` handed back. Guarding on `sounds.is_empty()` reads
        // "this machine has no assets" and "the loader is broken" identically,
        // and the mutation battery proved it: replacing the whole loader with
        // an empty index left this test green (it merely SKIPped).
        let store_present = rewo_data::sounds_json::asset_index_id("26.2")
            .and_then(|id| rewo_data::sounds_json::shared_assets_dir().map(|r| (r, id)))
            .map(|(root, id)| root.join("indexes").join(format!("{id}.json")).exists())
            .unwrap_or(false);
        if !store_present {
            eprintln!("SKIP: no unpacked asset store, so no sounds.json");
            return;
        }
        assert!(
            !live.system.sounds.is_empty(),
            "the asset store is present but build_sounds resolved no events"
        );
        // The index half. **An event name and its file path are unrelated
        // strings** — `block.stone.break` resolves to `dig/stone1..4`, a
        // pre-1.13 path the sound file never got renamed out of. This witness
        // asserted `block/stone/…` on its first run and was wrong; the
        // mismatch is exactly what M66 means by "the event names four files
        // and none of them is the event".
        let got = live
            .system
            .sounds
            .get_sound_seeded("minecraft:block.stone.break", 0)
            .expect("a variant");
        assert!(got.name.starts_with("minecraft:dig/stone"), "{}", got.name);
        assert_ne!(got.name, "minecraft:block.stone.break");
        assert_eq!(
            got.asset_path(),
            format!("minecraft/sounds/dig/stone{}.ogg", &got.name["minecraft:dig/stone".len()..])
        );
        assert_eq!(got.attenuation_distance, 16);
    }
}
