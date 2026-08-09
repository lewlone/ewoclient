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
    /// M94 — the most quads the recipe book drew in any frame **with no
    /// which-of-these overlay open**. Split from the field below by M104 so
    /// the two are comparable; r23's threshold is a floor, so its meaning is
    /// unchanged.
    book_quads_max: usize,
    /// M111 — frames on which the chat SCROLLBAR was drawn.
    ///
    /// It exists only while the screen is open AND the backlog exceeds the box
    /// (`virtualHeight != chatHeight`), so the gate injects 25 lines to reach
    /// it — otherwise this would be a witness over a path the run cannot enter,
    /// which is worse than no witness at all.
    chat_scrollbar_frames: u64,
    /// M110 — frames on which the CHAT SCREEN was open and drew its input bar.
    ///
    /// Separate from `chat_line_frames`, which counts the read-only HUD box
    /// M108 built: that one is non-zero from the first message whether or not
    /// a screen exists, so it cannot see whether `T` opens anything. The gate
    /// force-opens the screen a fifth of the way in, the same way it injects a
    /// container for r19 — a windowed run has no keyboard.
    chat_screen_frames: u64,
    /// M115 — frames on which the SUGGESTION POPUP put fills in the list.
    ///
    /// A strictly narrower claim than r27's: the screen can be open with no
    /// popup at all, which is its state until something is typed. The gate
    /// reaches it through the production chain rather than a fake — a
    /// `custom_chat_completions` packet, then a real keystroke — so a break
    /// anywhere from the decode to the render drops this to zero.
    suggestion_popup_frames: u64,
    /// M117 — frames on which the chat field was drawn as COLOURED RUNS
    /// rather than one flat line.
    ///
    /// Narrower than r27 and than r30: it needs a `/`-command in the field
    /// *and* a parse of it, and it is the only witness that can see the
    /// highlighting reach the windowed client at all.
    highlighted_command_frames: u64,
    /// M117 — frames on which the USAGE BOX was drawn.
    ///
    /// Mutually exclusive with r29 by construction, so it cannot be satisfied
    /// by the same moment: the popup and the box never coexist.
    usage_box_frames: u64,
    /// M118 — how many times an `@`-selector was offered by the client's own
    /// `EntitySelectorParser`.
    ///
    /// Narrower than r30: a literal completion needs only the tree, where this
    /// needs `minecraft:entity` to have stopped being `Unknown`.
    local_selector_completions: u64,
    /// M119 — how many times a registry id (a block or an item) was offered
    /// by the client's own parser.
    ///
    /// Narrower than r30 and disjoint from r33: it needs `block_state` or
    /// `item_stack` to have stopped being `Unknown`, and it counts a colon,
    /// which no literal or selector-option name carries.
    local_resource_completions: u64,
    /// M120 — how many times a COORDINATE default was offered by the
    /// client's own parser. Disjoint from r33 and r34: `~` appears in no
    /// literal, selector or registry id.
    local_coordinate_completions: u64,
    /// M124 — how many local answers offered a name from one of the seven
    /// literal tables. `sidebar.team.` prefixes only `scoreboard_slot`'s, so
    /// this is disjoint from r33/r34/r35 by construction.
    local_literal_table_completions: u64,
    /// M116 — how many times a `/`-command's completion was answered by the
    /// CLIENT rather than the server.
    ///
    /// The claim r30 makes is the milestone's whole point, and it is a
    /// negative one: M114 sent a packet for every keystroke on a command line
    /// and this counts the ones that no longer leave. A witness on "the popup
    /// opened" cannot see it, because both paths open a popup.
    local_command_completions: u64,
    /// M108 — frames on which the chat box put at least one line into the
    /// windowed frame's label list.
    ///
    /// The count comes from the production derivation (`build_text` returns
    /// it) rather than from the gate re-deriving `chat_lines` — a gate that
    /// recomputes the rule it grades agrees with any implementation, which is
    /// M93q's finding. It needs no caller staging: `--render-check` sends its
    /// own chat line, so an unstaged run still exercises the whole path from
    /// `player_chat` through the signature cache to the wrapped line.
    chat_line_frames: u64,
    /// M125 — frames on which a drawn chat line was a RESOLVED translatable.
    ///
    /// The scene is the `/give` this gate ALREADY stages for r14, so this adds
    /// no caller requirement — and it is a much stronger claim than the one it
    /// replaces, because the server's success message is a translatable THREE
    /// LEVELS DEEP whose middle argument is a bare integer:
    ///
    /// ```text
    /// commands.give.success.single   "Gave %s %s to %s"
    ///   with[0] = 1                             (a raw IntTag)
    ///   with[1] = chat.square_brackets "[%s]"
    ///               with[0] = item.minecraft.diamond_sword
    ///   with[2] = <the player's display-name component>
    /// ```
    ///
    /// So `Gave 1 [Diamond Sword]` cannot be produced unless the lookup, the
    /// substitution, the recursion into a component argument, and the
    /// heterogeneous-list unwrap all work. **None of those five words appears
    /// anywhere in the raw component**, so the string cannot leak through from
    /// a flattener that resolved nothing.
    ///
    /// The first version of this witness drove the JOIN message instead, on
    /// the stated premise that a server announces a joining player to that
    /// player. **It does not**, and the gate said so by scoring zero:
    /// `PlayerList.placeNewPlayer` broadcasts at line 202 and does
    /// `this.players.add(player)` at line 210, and `broadcastSystemMessage`
    /// iterates `this.players` — so the joiner is not yet in the list it is
    /// announced to. Suspect the witness first.
    translated_chat_frames: u64,
    /// M126d — frames on which the drawn chat carried MORE THAN ONE colour.
    ///
    /// The claim r26 and r37 cannot make. Both are satisfied by a chat box
    /// that flattens every message to one white string: r26 counts rows and
    /// r37 reads their characters, and neither can see whether the spans
    /// survived the wrap and reached the renderer as separately-coloured
    /// lines. A zero here with the scene injected means the pipeline is
    /// carrying spans and then throwing them away at the last step, which is
    /// exactly the failure the milestone exists to prevent.
    ///
    /// Counted over the DRAWN lines in `build_text`'s chat range, not over the
    /// chat store — M125's rule, one surface further on.
    styled_chat_frames: u64,
    /// M126d — frames on which a drawn chat line carried a non-plain
    /// `TextStyle`.
    ///
    /// Separate from the row above because the two halves travel by different
    /// routes: the colour rides `OwnedTextLine::color`, which existed before
    /// this milestone, while the five flags ride the `style` field it added.
    /// A wiring that dropped `style` on the floor would leave the colour
    /// witness green.
    flagged_chat_frames: u64,
    /// Frames on which a drawn chat line still carried a raw translation key.
    ///
    /// Not redundant with the row above: that one would stay green if the
    /// outer template resolved and an inner one did not, and this names all
    /// three keys of the same scene, so a break at ANY level turns it red.
    /// It must stay at zero.
    unresolved_key_frames: u64,
    /// M105 — frames on which the book drew its `x/y` page counter.
    ///
    /// A LABEL, not a quad, so `book_quads_max` cannot see it: the counter goes
    /// through the text pass. It needs a book of more than one page, which
    /// needs more unlocked recipes than a fresh player has — hence the caller
    /// requirement, the same shape as r14's hotbar staging.
    book_page_label_frames: u64,
    /// M104 — and the most it drew on a frame where one WAS open.
    ///
    /// A pair rather than one max, because the claim is a DIFFERENCE: an
    /// overlay adds a nine-sliced panel and a button each. One max over the
    /// whole run could not tell an overlay that drew from one that did not,
    /// since the book alone already clears any absolute threshold.
    book_overlay_quads_max: usize,
    /// The panel height the RENDERER was holding while a container was open.
    ///
    /// Read back from `WorldRenderer::container_panel_height`, not from the
    /// open menu's layout — the first cut asked the layout, which answers 168
    /// for a chest whether or not the panel builder returned one, so it could
    /// not tell a working container from one that had silently fallen back to
    /// the player's 166-tall panel. A value witness is only a value witness if
    /// it reads the value the draw used.
    container_panel_h: Option<f32>,
    /// The most overlay sprites the renderer's panel carried on any frame
    /// (M92).
    ///
    /// The chest injected at 0.4 has none — a chest's screen paints one sheet
    /// and stops — so this stays 0 unless the second injection at 0.85 (a
    /// brewing stand, with its data slots set) reaches the overlay builder.
    /// `containershot` grades those overlays offscreen; this is the only check
    /// that says the WINDOWED client draws them, which is the gap M88 closed
    /// for the panel itself and M86 for nine features before that.
    container_overlays_max: usize,
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
        // M94 — and that the recipe book itself was drawn. A shut book carries
        // no quads at all, so this is 0 unless the book's builder ran.
        //
        // The minimum is the panel, the book's OWN tab count and the filter
        // toggle, with no unlocked recipes to fill the grid. Read from
        // `CRAFTING_TABS` rather than written as a literal: M95 corrected that
        // count from four to five, and a literal would have been quietly
        // generous by one from then on.
        let min_quads = 1 + rewo_world::recipe_book_screen::CRAFTING_TABS.len() + 1;
        row(
            "r23 the recipe book was drawn in the windowed client",
            self.book_quads_max >= min_quads,
            format!(
                "{} quads at peak (panel + {} crafting tabs + filter = {min_quads} with an empty grid)",
                self.book_quads_max,
                rewo_world::recipe_book_screen::CRAFTING_TABS.len()
            ),
        );
        // M104 — and that the which-of-these overlay reached the windowed
        // draw. Nothing here can right-click a recipe cell (that needs the
        // cursor over a specific cell AND a server that has sent a
        // multi-recipe group), so the `Open` is injected through the same
        // `open_overlay` the click path calls — M17's rule, that injection is
        // the deterministic proof where a live encounter depends on timing
        // nothing here controls. What that leaves to the unit tests is the
        // click ROUTING; what it proves is the render path, which is the only
        // thing this gate can see and M86's whole reason for existing.
        //
        // The floor is derived from the claim rather than from the emitter: an
        // overlay is at least one panel quad plus one button each, so three
        // buttons must add at least four. Reading `overlay_chrome`'s own
        // length here would make it self-calibrating.
        row(
            "r24 the which-of-these overlay was drawn in the windowed client",
            self.book_overlay_quads_max >= self.book_quads_max + 4,
            format!(
                "{} quads at peak with an overlay open against {} without — at least a panel quad and one per button",
                self.book_overlay_quads_max, self.book_quads_max
            ),
        );
        // M105 — the counter reaches the windowed frame's label list. It is a
        // caller requirement like r14's hotbar: a fresh player's book is one
        // page and draws no counter at all, so `recipe give @s *` has to be
        // staged. Failing closed on an unstaged run is the gate refusing to
        // certify a path it never saw.
        row(
            "r25 the recipe book drew its page counter",
            self.book_page_label_frames > 0,
            format!(
                "{} of {} frames — needs a multi-page book, i.e. REWO_PRECMD with `recipe give @s *`",
                self.book_page_label_frames, self.frames
            ),
        );
        // M108 — the chat box reached the windowed frame. Unlike r14 and r25
        // this needs no caller staging: the run sends its own chat line, and
        // the server echoing it back is what drives `player_chat` through the
        // signature cache, the trust level, the wrap and the geometry. A zero
        // here means the whole chain is dead in the windowed client, which is
        // the failure M86 existed to catch.
        //
        // **It is structurally blind to the fade**, and the near-total count is
        // the tell rather than a worry: `RENDER_CHECK_SECONDS` is 8, i.e. 160
        // ticks, and `AlphaCalculator.timeBased` holds full alpha until 180, so
        // no message this run receives can fade before it ends. The fade is
        // graded by unit tests on both sides of the seam
        // (`the_fade_and_the_text_opacity_both_reach_the_line` here,
        // `a_message_holds_full_alpha_for_one_hundred_and_eighty_ticks` in
        // `rewo_world::chat`). Lengthening the run to reach the fade would turn
        // a gate into a soak for one property two tests already pin.
        row(
            "r26 the chat box drew a line in the windowed client",
            self.chat_line_frames > 0,
            format!(
                "{} of {} frames carried at least one wrapped chat line                  (near-total is CORRECT: the run is 8 s = 160 ticks and the                  fade starts at 180, so nothing can fade inside it)",
                self.chat_line_frames, self.frames
            ),
        );
        // M125 — and that a line was a RESOLVED translatable, which r26 cannot
        // see: before M125 the chat box drew `multiplayer.player.joined` and
        // scored a full r26.
        row(
            "r37 a chat line resolved a nested translatable component",
            self.translated_chat_frames > 0 && self.unresolved_key_frames == 0,
            format!(
                "{} of {} frames drew \"Gave 1 [Diamond Sword]\"; {} drew a raw key                  (must be 0). Three nesting levels and a bare-integer argument,                  off the `/give` r14 already stages — a count of 0 with the                  sword staged means the resolution is dead.",
                self.translated_chat_frames, self.frames, self.unresolved_key_frames
            ),
        );
        // M126d — that the spans survived to the renderer. r26 and r37 are
        // both satisfied by a chat box that flattens everything to one white
        // string, so neither can see this.
        row(
            "r38 a drawn chat line carried more than one colour",
            self.styled_chat_frames > 0,
            format!(
                "{} of {} frames drew ONE ROW in 2+ distinct colours (from one                  injected section-sign-coded system message: the codes must                  survive `parse_component`, the wrap's part list, and                  `chat_lines`' per-span emit. Across the whole box would be a                  weaker claim a flattening client also satisfies)",
                self.styled_chat_frames, self.frames
            ),
        );
        row(
            "r39 a drawn chat line carried a non-plain style flag",
            self.flagged_chat_frames > 0,
            format!(
                "{} of {} frames drew italic/underline/strikethrough (the flags                  ride `TextStyle`, a different field from the colour, so a                  wiring that dropped them would leave r38 green)",
                self.flagged_chat_frames, self.frames
            ),
        );
        // M110 — the chat SCREEN, as distinct from r26's read-only box. A zero
        // means `T` reaches nothing in the windowed client, which is the
        // failure M86 existed to catch and which r26 structurally cannot see.
        row(
            "r27 the chat screen opened and drew its input bar",
            self.chat_screen_frames > 0,
            format!(
                "{} of {} frames had the screen up with its input bar in the                  backdrop list",
                self.chat_screen_frames, self.frames
            ),
        );
        // M111 — and the scrollbar within it, which needs more chat than the
        // box holds and so is a strictly narrower claim than r27's.
        row(
            "r28 the chat scrollbar was drawn",
            self.chat_scrollbar_frames > 0,
            format!(
                "{} of {} frames drew the bar's two rects (needs a backlog past                  the focused box's 20 rows, which the run injects)",
                self.chat_scrollbar_frames, self.frames
            ),
        );
        // M115 — and the popup within it, which needs a completion word AND a
        // keystroke, so it is narrower again than r28's.
        row(
            "r29 the suggestion popup was drawn",
            self.suggestion_popup_frames > 0,
            format!(
                "{} of {} frames drew the popup's row fills (needs a                  custom_chat_completions word and a keystroke, both of which the run                  drives through the production path)",
                self.suggestion_popup_frames, self.frames
            ),
        );
        // M117 — the syntax highlighting reached the frame at all.
        row(
            "r31 the command line was drawn as coloured runs",
            self.highlighted_command_frames > 0,
            format!(
                "{} of {} frames drew the field from a parse rather than as one                  flat line",
                self.highlighted_command_frames, self.frames
            ),
        );
        // M117 — the usage box, which `extractRenderState` draws only when
        // the popup is absent, so this and r29 name disjoint moments.
        row(
            "r32 the usage box was drawn under the field",
            self.usage_box_frames > 0,
            format!(
                "{} of {} frames drew a usage line (needs an ARGUMENT expected                  at the cursor and no popup over it)",
                self.usage_box_frames, self.frames
            ),
        );
        // M118 — and a SELECTOR among them, which needs the entity argument
        // type rather than just the tree.
        row(
            "r33 an entity selector was offered locally",
            self.local_selector_completions > 0,
            format!(
                "{} completions containing an @-selector (needs                  minecraft:entity to parse, which it did not before M118)",
                self.local_selector_completions
            ),
        );
        // M119 — and a registry id among them, which needs the block/item
        // argument types rather than the tree or the selector.
        row(
            "r34 a registry id was offered locally",
            self.local_resource_completions > 0,
            format!(
                "{} completions containing a namespaced id (needs block_state or                  item_stack to parse, which they did not before M119)",
                self.local_resource_completions
            ),
        );
        // M120 — and a coordinate among them.
        row(
            "r35 a coordinate default was offered locally",
            self.local_coordinate_completions > 0,
            format!(
                "{} completions containing a `~` (needs the coordinate family to                  parse, which it did not before M120)",
                self.local_coordinate_completions
            ),
        );
        // M124 — and a name from one of the seven tables that live outside
        // their own argument class.
        row(
            "r36 a literal table was offered locally",
            self.local_literal_table_completions > 0,
            format!(
                "{} completions offering a `sidebar.team.*` slot (needs                  scoreboard_slot's table, which read as a bare word before M124)",
                self.local_literal_table_completions
            ),
        );
        // M116 — the dispatcher answered a command locally, i.e. WITHOUT a
        // round trip. Both paths open a popup, so r29 cannot see this.
        row(
            "r30 a command completion was answered locally",
            self.local_command_completions > 0,
            format!(
                "{} completions answered by the client's own dispatcher (the run                  types `/` then a letter, which reaches only literals)",
                self.local_command_completions
            ),
        );
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
            "r22 the windowed client drew a container's overlays",
            self.container_overlays_max > 0,
            format!(
                "{} overlay sprites at peak (a brewing stand draws fuel + arrow + bubbles;                  the chest injected earlier draws none)",
                self.container_overlays_max
            ),
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
                beacon_effects,
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
            blocks,
            beacon_effects,
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
    beacon_effects: BeaconEffectIds,
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
        //
        // `--render-check` supplies its own (M108) rather than making this a
        // third caller requirement beside r14's hotbar and r25's recipe book.
        // The server echoing the line back is what drives `player_chat`
        // through the signature cache, the trust level, the wrap and the
        // geometry, so the whole chain is exercised by the run itself.
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
                // M93i — `CrafterScreen.slotClicked` runs its toggle BEFORE
                // the ordinary click and then falls through to it, so this is
                // additive: whatever it does, the click below still happens.
                let toggle = session.crafter_slot_click(slot, button, input);
                if toggle != rewo_world::menu::CrafterToggle::None {
                    println!("[rewo-m93i] CRAFTER slot {slot}: {toggle:?}");
                }
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
            beacon_effects,
            // The headless path drives no screen, so the menu is the answer.
            None,
            // …and it holds no scroll, so a stonecutter would draw its first
            // page. `run_headless` never opens one.
            None,
            // …nor an anvil, so there is no field to draw.
            None,
            // …nor a merchant.
            None,
            // …and the book's selection is its default, since nothing can
            // click it headlessly, with an empty search field.
            Default::default(),
            "",
            &rewo_world::edit_box::EditBox::new(
                rewo_world::recipe_book_screen::SEARCH_MAX_LENGTH,
            ),
            // Headless: a fixed clock, so a caret's blink cannot make the same
            // scene render two ways between runs.
            0,
            // Headless never opens the which-of-these overlay: nothing can
            // right-click a recipe cell without a cursor.
            None,
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
    // Same ordering as the windowed path: drain the chat events before the
    // text is built, or a headless `--out` render shows an empty chat box for
    // messages the session has already decoded.
    apply_chat(&mut session, world_renderer.font_advance().copied());
    let (mut headless_text, _, _) = build_text(
        &session,
        gui_px(1280, 720),
        720.0,
        None,
        true,
        false,
        world_renderer.font_advance().copied(),
    );
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
    /// Whether `--render-check` has force-opened the chat screen yet (M110).
    chat_injected: bool,
    /// Whether it has typed a `/`-command yet (M116).
    command_injected: bool,
    /// Whether it has typed a coordinate command yet (M120).
    coords_injected: bool,
    /// M124 — whether the `scoreboard objectives setdisplay ` typing has run.
    literal_table_injected: bool,
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
                if matches!(event.physical_key, PhysicalKey::Code(KeyCode::AltLeft)) {
                    self.alt = p;
                }
                // M110 — the chat screen owns the keyboard entirely while it
                // is open, and it goes ahead of every other screen because
                // `Gui.screen` is ONE slot: with a chat screen in it there is
                // no inventory to route to. Opening it is handled after this
                // block, so `T` cannot both open the screen and be typed into
                // it on the same event.
                if self.chat_screen.is_some() {
                    if p {
                        if let Some(key) = glfw_key(event.physical_key) {
                            let mods = (i32::from(self.shift))
                                | (i32::from(self.ctrl) << 1)
                                | (i32::from(self.alt) << 2);
                            self.chat_key(key, mods);
                        }
                        if let Some(text) = event.text.as_ref() {
                            let chars: Vec<char> = text.chars().collect();
                            for ch in chars {
                                self.chat_char(ch);
                            }
                        }
                    }
                    // Shift is tracked either way: it holds the wheel to one
                    // line and is read by `mouse_scrolled`.
                    if !matches!(
                        event.physical_key,
                        PhysicalKey::Code(KeyCode::ShiftLeft)
                            | PhysicalKey::Code(KeyCode::ShiftRight)
                    ) {
                        return;
                    }
                }
                if self.screen.inventory_open() {
                    if p {
                        // M93t — the anvil's name field runs FIRST and, while
                        // it can consume input, swallows everything but Escape.
                        // Vanilla's `AnvilScreen.keyPressed` reaches `super`
                        // only when the box neither handled the key nor could
                        // have, so with an item in slot 0 no screen shortcut
                        // fires at all.
                        let items = self.items.clone();
                        let mods = (i32::from(self.shift))
                            | (i32::from(self.ctrl) << 1)
                            | (i32::from(self.alt) << 2);
                        if let (Some(session), Some(key)) =
                            (self.session.as_mut(), glfw_key(event.physical_key))
                        {
                            let mut clip = std::mem::take(&mut self.clipboard);
                            // M99 — the book's search field first, when it has
                            // focus. Only one field can be focused at a time
                            // (a click on the book unfocuses nothing else, but
                            // the anvil's is only reachable while an anvil is
                            // open and the book's only while the book is), so
                            // the order is belt-and-braces rather than a
                            // contract.
                            if self.screen.book_search.is_focused() {
                                let input = rewo_world::edit_box::Input::new(key, mods);
                                let consumed =
                                    self.screen.book_search.key_pressed(input, &mut clip);
                                self.clipboard = clip;
                                if consumed {
                                    follow_cursor(
                                        &mut self.screen.book_search,
                                        self.baked.as_ref(),
                                        rewo_world::recipe_book_screen::SEARCH_INNER_W,
                                    );
                                    return;
                                }
                                clip = self.clipboard.clone();
                            }
                            let consumed = anvil_key(
                                session,
                                &mut self.screen,
                                &items,
                                self.baked.as_ref(),
                                rewo_world::edit_box::Input::new(key, mods),
                                &mut clip,
                            );
                            self.clipboard = clip;
                            if consumed {
                                return;
                            }
                        }
                        // A typed character. winit reports it separately from
                        // the key, which is exactly the seam Rewo never read —
                        // `KeyEvent.text`, not `PhysicalKey`.
                        if let (Some(session), Some(text)) =
                            (self.session.as_mut(), event.text.as_ref())
                        {
                            let mut any = false;
                            for ch in text.chars() {
                                if self.screen.book_search.is_focused()
                                    && self.screen.book_search.char_typed(ch)
                                {
                                    any = true;
                                    follow_cursor(
                                        &mut self.screen.book_search,
                                        self.baked.as_ref(),
                                        rewo_world::recipe_book_screen::SEARCH_INNER_W,
                                    );
                                }
                                any |= anvil_char(session, &mut self.screen, &items, self.baked.as_ref(), ch);
                            }
                            if any {
                                return;
                            }
                        }
                        if let Some(action) = screen_key_action(event.physical_key, self.ctrl) {
                            let ext = self.state.as_ref().map(|s| s.window.inner_size());
                            let items = self.items.clone();
                            if let (Some(session), Some(ext)) = (self.session.as_mut(), ext) {
                                click_screen(
                                    session,
                                    &items,
                                    &mut self.screen,
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
                    // T and `/` open the chat screen (M110).
                    //
                    // Two keys, one screen, differing only in the prefix the
                    // field starts with — and in which drafts they will
                    // restore, which is `ChatMethod.isDraftRestorable` and not
                    // symmetric. The routing branch above returns before this
                    // whenever a chat screen is already open, so `T` cannot
                    // both open the screen and be typed into it.
                    PhysicalKey::Code(KeyCode::KeyT) if p => {
                        self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Message);
                    }
                    PhysicalKey::Code(KeyCode::Slash) if p => {
                        self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Command);
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
                    // M98 — the recipe book is pressed before EVERYTHING:
                    // `AbstractRecipeBookScreen.mouseClicked` runs the book and
                    // only calls `super` when the book declines.
                    // The bake's display names, or an empty map when there is
                    // no bake — in which case `search_entry_of` falls back to
                    // the id's prettified path, which is the same fallback the
                    // tooltip takes. Degrading rather than disabling the book's
                    // clicks.
                    static NO_NAMES: std::sync::OnceLock<
                        std::collections::HashMap<String, String>,
                    > = std::sync::OnceLock::new();
                    let display = self
                        .baked
                        .as_ref()
                        .map(|b| &b.item_names)
                        .unwrap_or_else(|| NO_NAMES.get_or_init(Default::default));
                    if book_press(
                        session,
                        &mut self.screen,
                        &items,
                        // M107 — `event.hasShiftDown()`, straight through to
                        // `useMaxItems`.
                        self.shift,
                        display,
                        b == 1,
                        ext.width as f32,
                        ext.height as f32,
                    ) {
                        return;
                    }
                    // M92f — an enchanting row is pressed BEFORE the slot
                    // logic, and only then. `EnchantmentScreen.mouseClicked`
                    // runs its three-row loop first and calls
                    // `super.mouseClicked` only when no row took the press, so
                    // a click on a *disabled* row still falls through to the
                    // normal slot handling (which finds nothing there).
                    if enchant_press(session, &self.screen, ext.width as f32, ext.height as f32) {
                        return;
                    }
                    // M93m — the beacon's buttons, on the same seam and for
                    // the same reason: a live widget consumes the click and
                    // it never reaches the slot logic, while a DARK one falls
                    // through exactly as a disabled enchanting row does.
                    if beacon_press(
                        session,
                        &mut self.screen,
                        &self.beacon_effects,
                        ext.width as f32,
                        ext.height as f32,
                    ) {
                        return;
                    }
                    // M93s — the stonecutter's grid. Same seam, and vanilla's
                    // own order: the recipe loop runs first and returns true
                    // on a hit, then the scrollbar's grab box sets `scrolling`
                    // and DOES NOT consume the press — it falls through to
                    // `super.mouseClicked`, so a grab still reaches the slots.
                    if cut_press(
                        session,
                        &mut self.screen,
                        &items,
                        ext.width as f32,
                        ext.height as f32,
                    ) {
                        return;
                    }
                    // M93u — the merchant's trade buttons, on the same seam.
                    if merchant_press(
                        session,
                        &mut self.screen,
                        ext.width as f32,
                        ext.height as f32,
                    ) {
                        return;
                    }
                    // `AbstractContainerScreen.mouseClicked`'s double click:
                    // the **same slot**, the **left** button, and under 250 ms
                    // since the last one. Not "two clicks anywhere in
                    // 250 ms" — moving to a neighbouring slot resets it.
                    let layout = session.shown_menu().layout();
                    let slot =
                        self.screen
                            .hovered(
                            layout,
                            ext.width as f32,
                            ext.height as f32,
                            book_visible(session),
                        );
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
                        &mut self.screen,
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
                // `StonecutterScreen.mouseReleased` clears `scrolling`
                // unconditionally — not gated on `displayRecipes`, so a list
                // that vanishes mid-drag does not strand the grab (M93s).
                if let Some(c) = self.screen.cut.as_mut() {
                    c.scrolling = false;
                }
                if let Some(m) = self.screen.merchant.as_mut() {
                    m.dragging = false;
                }
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
                // M93s — `mouseDragged` while the stonecutter's thumb is held.
                // Guarded on `isScrollBarActive` as well as `scrolling`, so a
                // list that shrinks under a held thumb stops moving.
                self.cut_drag();
                self.merchant_drag();
                // Crossing a slot with the button down extends a drag. The
                // slot only *joins* it if the server would accept it, and that
                // is decided at release — this records the path.
                if self.screen.inventory_open() {
                    if let Some(ext) = self.state.as_ref().map(|s| s.window.inner_size()) {
                        let layout = self.shown_layout();
                        let book_open =
                            self.session.as_ref().is_some_and(book_visible);
                        if let Some(slot) = self.screen.hovered(
                            layout,
                            ext.width as f32,
                            ext.height as f32,
                            book_open,
                        ) {
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
                // M93s — and the stonecutter's grid. `mouseScrolled` returns
                // true whether or not the bar is active, so the screen
                // swallows every notch; only an active bar moves.
                self.cut_wheel(dy);
                self.merchant_wheel(dy);
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
    /// `StonecutterScreen.mouseDragged` (M93s).
    ///
    /// ```java
    /// if (this.scrolling && this.isScrollBarActive()) {
    ///    int yscr = this.topPos + 14;
    ///    this.scrollOffs = ((float)event.y() - yscr - 7.5F) / ((yscr + 54) - yscr - 15.0F);
    ///    this.scrollOffs = Mth.clamp(this.scrollOffs, 0.0F, 1.0F);
    ///    this.startIndex = (int)(this.scrollOffs * this.getOffscreenRows() + 0.5) * 4;
    /// }
    /// ```
    ///
    /// Both halves of the guard matter: `scrolling` alone would move the list
    /// whenever the cursor did, and `isScrollBarActive` alone would let a list
    /// that shrank under a held thumb keep scrolling past its end.
    fn cut_drag(&mut self) {
        use rewo_world::menu_screen as ms;
        if !self.screen.cut.is_some_and(|c| c.scrolling) {
            return;
        }
        let Some(ext) = self.state.as_ref().map(|s| s.window.inner_size()) else {
            return;
        };
        let items = self.items.clone();
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some(open) = session.menus.open() else {
            return;
        };
        if open.layout.protocol_id != ms::STONECUTTER_MENU_PROTOCOL_ID {
            return;
        }
        let name = open.menu.menu_slot(0).and_then(|s| items.name(s.item_id));
        let visible = name.map_or(0, |n| rewo_data::stonecutter_table::select_by_input(n).len());
        if !ms::cut_scroll_active(ms::cut_display_recipes(name.is_some(), visible), visible) {
            return;
        }
        let (_, gy) = rewo_gpu::container::screen_to_gui_for(
            self.screen.mouse,
            ext.width as f32,
            ext.height as f32,
            open.layout.image_w as f32,
            open.layout.image_h as f32,
        );
        if let Some(c) = self.screen.cut.as_mut() {
            c.scroll_offs = ms::cut_scroll_offs_from_drag(gy);
        }
    }

    /// `MerchantScreen.mouseDragged` / `mouseScrolled` (M93u).
    fn merchant_drag(&mut self) {
        use rewo_world::merchant_screen as ms;
        if !self.screen.merchant.is_some_and(|l| l.dragging) {
            return;
        }
        let Some(ext) = self.state.as_ref().map(|s| s.window.inner_size()) else {
            return;
        };
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some(open) = session.menus.open() else {
            return;
        };
        if open.layout.protocol_id != ms::MERCHANT_MENU_PROTOCOL_ID {
            return;
        }
        let n = session.merchant.as_ref().map_or(0, |m| m.offers.len());
        // `mouseDragged` is NOT gated on `canScroll` — only on `isDragging` —
        // but `maxScrollOff` goes negative for a short list, so the clamp is
        // what keeps it at 0. Guarding here would be a deviation that happens
        // to agree.
        let (_, gy) = rewo_gpu::container::screen_to_gui_for(
            self.screen.mouse,
            ext.width as f32,
            ext.height as f32,
            open.layout.image_w as f32,
            open.layout.image_h as f32,
        );
        if let Some(l) = self.screen.merchant.as_mut() {
            l.scroll_off = ms::scroll_off_from_drag(gy, n);
        }
    }

    fn merchant_wheel(&mut self, dy: f64) {
        use rewo_world::merchant_screen as ms;
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some(open) = session.menus.open() else {
            return;
        };
        if open.layout.protocol_id != ms::MERCHANT_MENU_PROTOCOL_ID {
            return;
        }
        let n = session.merchant.as_ref().map_or(0, |m| m.offers.len());
        if !ms::can_scroll(n) {
            return;
        }
        if let Some(l) = self.screen.merchant.as_mut() {
            l.scroll_off = ms::scroll_off_from_wheel(l.scroll_off, dy, n);
        }
    }

    /// `StonecutterScreen.mouseScrolled` (M93s) — one notch is one ROW, so a
    /// long list scrolls proportionally slower per notch.
    ///
    /// The sign is vanilla's `scrollOffs - scrollY / offscreenRows`, the same
    /// minus M84 records for `AbstractScrollArea`.
    fn cut_wheel(&mut self, dy: f64) {
        use rewo_world::menu_screen as ms;
        if self.screen.cut.is_none() {
            return;
        }
        let items = self.items.clone();
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some(open) = session.menus.open() else {
            return;
        };
        if open.layout.protocol_id != ms::STONECUTTER_MENU_PROTOCOL_ID {
            return;
        }
        let name = open.menu.menu_slot(0).and_then(|s| items.name(s.item_id));
        let visible = name.map_or(0, |n| rewo_data::stonecutter_table::select_by_input(n).len());
        if !ms::cut_scroll_active(ms::cut_display_recipes(name.is_some(), visible), visible) {
            return;
        }
        if let Some(c) = self.screen.cut.as_mut() {
            c.scroll_offs = ms::cut_scroll_offs_from_wheel(c.scroll_offs, dy, visible);
        }
    }

    /// Falls back to `PLAYER` with no session, which is what the screen shows
    /// before a connection anyway.
    fn shown_layout(&self) -> &'static rewo_world::menu_layout::MenuLayout {
        self.session
            .as_ref()
            .map(|s| s.shown_menu().layout())
            .unwrap_or(&rewo_world::menu_layout::PLAYER)
    }

    fn set_screen_open(&mut self, open: bool) {
        // M104 — the which-of-these overlay does not survive the screen.
        // `RecipeBookComponent.setVisible(false)` calls
        // `recipeBookPage.setInvisible()`, and the component's `init` rebuilds
        // the page from scratch — so neither closing a screen nor opening one
        // can leave an overlay stranded over a book that has moved on. Cleared
        // on OPEN as well as close, because it is a snapshot of a page that
        // may no longer exist.
        self.screen.book_overlay = None;
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

    /// `getCustomTabSuggestions()` — the online players unioned with whatever
    /// `custom_chat_completions` has set, which is what plain chat completes
    /// from. Empty with no session, so an offline harness offers nothing
    /// rather than panicking.
    fn tab_words(&self) -> Vec<String> {
        match self.session.as_ref() {
            Some(session) => session
                .suggestions
                .tab_suggestions(session.world.entities.all_names()),
            None => Vec::new(),
        }
    }

    /// The input field's geometry, as `CommandSuggestions` measures from it.
    fn suggestion_metrics(&self) -> rewo_world::command_suggestions::InputMetrics {
        let (gui_w, gui_h) = self.gui_size();
        let (x, _y, w, _h) = rewo_world::chat_screen::input_rect(gui_w, gui_h);
        rewo_world::command_suggestions::InputMetrics {
            x,
            // `getInnerWidth()` is `bordered ? width - 8 : width`, and the
            // chat field calls `setBordered(false)`.
            inner_width: w,
            screen_height: gui_h,
        }
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

    /// One key press while the chat screen is up (M110).
    ///
    /// The adapter M97's lesson keeps producing: every rule lives in
    /// `rewo_world::chat_screen` and reaches a test; this turns the returned
    /// `ChatAction` into the two things only the app can do — talk to the
    /// socket and close the screen.
    fn chat_key(&mut self, key: i32, modifiers: i32) {
        use rewo_world::chat_screen::{ChatAction, ExitReason};
        // Esc is `onClose`, and it is checked here rather than inside the
        // model because `Screen.keyPressed` handles it before the focused
        // widget — an `EditBox` with `canLoseFocus` false would otherwise
        // swallow it.
        if key == 256 {
            if let Some(s) = self.chat_screen.as_mut() {
                s.close();
            }
            self.close_chat_screen();
            return;
        }
        let (recent, per_page) = match self.session.as_ref() {
            Some(session) => (
                session.chat.recent_chat().to_vec(),
                rewo_world::chat::ChatOptions::default().lines_per_page(true),
            ),
            None => (Vec::new(), 20),
        };
        let mut clip = std::mem::take(&mut self.clipboard);
        let metrics = self.suggestion_metrics();
        let words = self.tab_words();
        let advance_for_width = self.advance();
        let width_of = move |s: &str| match &advance_for_width {
            Some(a) => rewo_gpu::text::width(s, a),
            None => 0,
        };
        let env = rewo_world::chat_screen::SuggestionEnv {
            metrics,
            width: &width_of,
            tab_words: &words,
            // `Options.autoSuggestions` defaults true, and Rewo has no
            // options screen to turn it off.
            auto_suggestions: true,
        };
        let action = match self.chat_screen.as_mut() {
            Some(s) => s.key_pressed(
                rewo_world::edit_box::Input { key, modifiers },
                &mut clip,
                &recent,
                per_page,
                &env,
            ),
            None => ChatAction::None,
        };
        self.clipboard = clip;
        self.resolve_command_suggestions();
        let advance = self.advance();
        // Which variant it was, captured before `action` is consumed.
        let is_command = matches!(action, ChatAction::Command(_));
        match action {
            ChatAction::Send(msg) | ChatAction::Command(msg) => {
                if let Some(session) = self.session.as_mut() {
                    // `addRecentChat` takes the message as TYPED, before
                    // `handleChatInput` strips the slash — the history replays
                    // what you wrote, not what went on the wire.
                    let typed = if is_command {
                        format!("/{msg}")
                    } else {
                        msg.clone()
                    };
                    session.chat.add_recent_chat(&typed);
                    let sent = if is_command {
                        session.send_command(&msg)
                    } else {
                        session.send_chat(&msg)
                    };
                    if let Err(e) = sent {
                        log::warn!("chat: send failed: {e}");
                    }
                }
            }
            ChatAction::Scroll(n) => {
                if let Some(session) = self.session.as_mut() {
                    let width_of =
                        move |s: &str, st: rewo_world::chat_style::ChatStyle| match &advance {
                            Some(a) => rewo_gpu::text::width_styled(s, a, st.bold),
                            None => 0,
                        };
                    let ctx = rewo_world::chat::WrapContext {
                        options: rewo_world::chat::ChatOptions::default(),
                        // The box is TALLER while the screen is open, and
                        // `scrollChat` clamps against `getLinesPerPage()` — so
                        // `false` here would clamp the focused view against the
                        // unfocused box's ten rows and stop the scroll short.
                        focused: true,
                        width_of: &width_of,
                        deleted_marker_text: DELETED_CHAT_MESSAGE,
                    };
                    session.chat.scroll_chat(n, &ctx);
                }
            }
            ChatAction::Close | ChatAction::None | ChatAction::NotHandled => {}
        }
        // `closeOnSubmit` is true for this screen, so a submitted message
        // closes it — the model records that as `ExitReason::Done` rather than
        // returning a `Close`, because the two are the same event in vanilla.
        if matches!(
            self.chat_screen.as_ref().map(|s| s.exit_reason()),
            Some(ExitReason::Done)
        ) {
            self.close_chat_screen();
        }
    }

    /// One printable character typed into the chat screen.
    ///
    /// Split out of the event loop because `onEdited` now needs the
    /// suggestion environment, which needs `&self` while the screen needs
    /// `&mut self`.
    fn chat_char(&mut self, ch: char) {
        let metrics = self.suggestion_metrics();
        let words = self.tab_words();
        let advance_for_width = self.advance();
        let width_of = move |s: &str| match &advance_for_width {
            Some(a) => rewo_gpu::text::width(s, a),
            None => 0,
        };
        let env = rewo_world::chat_screen::SuggestionEnv {
            metrics,
            width: &width_of,
            tab_words: &words,
            auto_suggestions: true,
        };
        if let Some(s) = self.chat_screen.as_mut() {
            s.char_typed(ch, &env);
        }
        self.resolve_command_suggestions();
    }

    /// The coloured runs for the chat field, or `None` when there is nothing
    /// to colour (M117).
    ///
    /// `formatChat` returns null while `currentParse` is null, and
    /// `updateCommandInfo` only ever builds one for a `/`-command — so an
    /// ordinary chat message is drawn in the field's own colour, which is a
    /// state vanilla passes through too.
    /// The usage box's fills and text for this frame (M117), or two empty
    /// lists when there is nothing to show.
    ///
    /// Built from the cached parse rather than a fresh one, so it and the
    /// syntax highlighting cannot disagree about what the field says.
    ///
    /// A free-standing associated function rather than a method, for the same
    /// reason `chat_runs` is: the frame already holds the session borrowed.
    fn usage_box_parts(
        cs: Option<&rewo_world::chat_screen::ChatScreen>,
        session: &PlaySession,
        cache: &Option<(String, rewo_net::dispatcher::ParseResults)>,
        advance: Option<[u8; 256]>,
        gui: (i32, i32),
        px: f32,
    ) -> (Vec<rewo_gpu::hud::HudFill>, Vec<rewo_gpu::world::OwnedTextLine>) {
        let empty = (Vec::new(), Vec::new());
        let Some(cs) = cs else {
            return empty;
        };
        let Some((text, parsed)) = cache.as_ref() else {
            return empty;
        };
        if text != &cs.input.value() {
            return empty;
        }
        let cursor = cs.input.cursor_position();
        let lines = rewo_net::command_format::usage_lines(
            &session.commands,
            parsed,
            cursor,
            cs.suggestions.pending().is_none_or(|s| s.is_empty()),
        );
        if lines.is_empty() {
            return empty;
        }
        let width_of = move |s: &str| match &advance {
            Some(a) => rewo_gpu::text::width(s, a),
            None => 0,
        };
        let (gui_w, gui_h) = gui;
        let _ = gui_w;
        let (fx, _fy, fw, _fh) = rewo_world::chat_screen::input_rect(gui_w, gui_h);
        // `getScreenX(startPos)` — the field's x plus the width of everything
        // before the word being completed.
        let start = rewo_net::command_format::usage_lines_start(parsed, cursor);
        let value: Vec<u16> = cs.input.value().encode_utf16().collect();
        let prefix = String::from_utf16_lossy(&value[..start.min(value.len())]);
        let box_width = lines.iter().map(|l| width_of(l)).max().unwrap_or(0);
        let position = rewo_net::command_format::usage_position(
            fx + width_of(&prefix),
            fx,
            fw,
            box_width,
        );
        usage_box(
            &lines,
            position,
            gui_h,
            px,
            cs.suggestions.config().fill_color,
            &width_of,
        )
    }

    /// Answer a `/`-command's completion locally where the dispatcher can, and
    /// ask the server only where vanilla would (M116).
    ///
    /// M114 asked about **every** command, because with no dispatcher the
    /// client could not tell a literal from an argument. Now it can:
    /// `dispatcher::parse` walks the tree M113 decodes, and
    /// `completion_suggestions` returns both what the client answered and
    /// whether any candidate child's provider is one Rewo routes to the
    /// server. `/g` therefore completes with **no packet at all**.
    ///
    /// When it does ask, the server's reply REPLACES the local set rather than
    /// merging with it — see `dispatcher`'s module docs — because
    /// `handleCustomCommandSuggestions` runs the server's own dispatcher over
    /// the whole input and returns literals too, so its answer is a superset
    /// at that position.
    fn resolve_command_suggestions(&mut self) {
        let Some(command) = self
            .chat_screen
            .as_mut()
            .and_then(|s| s.take_command_request())
        else {
            return;
        };
        let units: Vec<u16> = command.encode_utf16().collect();
        // The text is the field up to the cursor, INCLUDING the slash, so the
        // cursor is its length and the parse starts at 1 — every range is then
        // an index into the field itself.
        // M118 — the selector parser needs the online names, exactly as
        // `EntityArgument.listSuggestions` takes them from the source.
        let words = self.tab_words();
        let cmd = rewo_net::dispatcher::CommandCtx {
            names: &words,
            blocks: Some(&self.blocks),
            items: Some(&self.items),
        };
        let completion = self.session.as_ref().map(|session| {
            let parsed = rewo_net::dispatcher::parse(&session.commands, &units, 1, cmd);
            rewo_net::dispatcher::completion_suggestions(&session.commands, &parsed, units.len(), cmd)
        });
        let Some(completion) = completion else {
            return;
        };
        if completion.ask_server {
            if let Some(session) = self.session.as_mut() {
                if let Err(e) = session.request_command_suggestions(&command) {
                    log::debug!("chat: suggestion request not sent: {e}");
                }
            }
            return;
        }
        // Counted only when the client actually ANSWERED. An empty command
        // tree parses to no children and would otherwise report a local
        // completion for every keystroke while proving nothing — the witness
        // has to name the suggestions, not the code path.
        if !completion.local.is_empty() {
            // M118 — a selector answered locally is a strictly narrower claim
            // than "a completion was", because it needs the entity argument's
            // own parser rather than a literal match.
            let selector = completion
                .local
                .list
                .iter()
                .any(|s| s.text.starts_with('@'));
            // M119 — a namespaced id. Distinctive by construction: a selector
            // starts with `@`, and neither a literal nor a selector-option
            // name contains a colon.
            let resource = completion
                .local
                .list
                .iter()
                .any(|s| s.text.contains(':'));
            // M120 — a coordinate default. `~` appears in no literal, no
            // selector and no registry id, so the test is disjoint from r33's
            // and r34's by construction.
            let coordinate = completion
                .local
                .list
                .iter()
                .any(|s| s.text.starts_with('~'));
            // M124 — a name from one of the seven literal tables. `DisplaySlot`
            // is the only source of a `sidebar.team.` prefix anywhere in the
            // protocol, so this cannot be satisfied by a literal, a selector, a
            // registry id or a coordinate.
            let literal_table = completion
                .local
                .list
                .iter()
                .any(|s| s.text.starts_with("sidebar.team."));
            if let Some(c) = self.check.as_mut() {
                c.local_command_completions += 1;
                if selector {
                    c.local_selector_completions += 1;
                }
                if resource {
                    c.local_resource_completions += 1;
                }
                if coordinate {
                    c.local_coordinate_completions += 1;
                }
                if literal_table {
                    c.local_literal_table_completions += 1;
                }
            }
        }
        let metrics = self.suggestion_metrics();
        let advance_for_width = self.advance();
        let width_of = move |s: &str| match &advance_for_width {
            Some(a) => rewo_gpu::text::width(s, a),
            None => 0,
        };
        let env = rewo_world::chat_screen::SuggestionEnv {
            metrics,
            width: &width_of,
            tab_words: &[],
            auto_suggestions: true,
        };
        if let Some(s) = self.chat_screen.as_mut() {
            s.accept_suggestions(completion.local, &env);
        }
    }

    /// `T` / `/` — `ChatComponent.openScreen`.
    fn open_chat_screen(&mut self, method: rewo_world::chat_screen::ChatMethod) {
        let recent_len = self
            .session
            .as_ref()
            .map(|s| s.chat.recent_chat().len())
            .unwrap_or(0);
        self.chat_screen = Some(rewo_world::chat_screen::ChatScreen::open(
            method,
            self.chat_draft.as_ref(),
            recent_len,
        ));
        self.grab_for_screen(false);
    }

    /// `Gui.setScreen(null)` — and `ChatScreen.removed`, which is where the
    /// draft is decided and the chat scroll goes back to the bottom.
    fn close_chat_screen(&mut self) {
        use rewo_world::chat_screen::DraftOutcome;
        if let Some(s) = self.chat_screen.take() {
            match s.removed(true) {
                DraftOutcome::Discard => self.chat_draft = None,
                DraftOutcome::Save(d) => self.chat_draft = Some(d),
                DraftOutcome::Keep => {}
            }
            if let Some(session) = self.session.as_mut() {
                session.chat.reset_chat_scroll();
            }
        }
        self.grab_for_screen(true);
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
        // M93m — a beacon button asked to close. Drained here rather than
        // closed from inside the press, so every close goes through the one
        // path that owns the screen.
        //
        // **This closes the CLIENT's screen only.** Vanilla's
        // `player.closeContainer()` also sends a serverbound
        // `container_close`, and Rewo resolves no such packet — `ids.rs` has
        // the clientbound one alone. So the server still believes the menu is
        // open. That gap is older and wider than this milestone (it affects
        // every screen close, not the beacon's), and is recorded rather than
        // half-fixed here.
        if std::mem::take(&mut self.screen.close_beacon) {
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
            // M94 — open the recipe book, which no server does unprompted: a
            // fresh player's `RecipeBookSettings` are all shut, so without this
            // the windowed client never reaches the book's draw and `r23` would
            // be measuring nothing. Injected EARLY so it is live while the
            // inventory (a CRAFTING book menu) is on screen for `r16`.
            //
            // Eight booleans, four `(open, filtering)` pairs in
            // `RecipeBookSettings`' positional order — crafting first.
            if !self.book_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.92 {
                    if let Some(session) = self.session.as_mut() {
                        let id = session.ids.cb_play_recipe_book_settings;
                        let body = [1u8, 0, 0, 0, 0, 0, 0, 0];
                        session.apply_recipe_book(id, &body);
                        if session.recipe_book_settings.crafting.open {
                            self.book_injected = true;
                        }
                    }
                }
            }
            // M110 — force-open the chat screen a fifth of the way in. A
            // windowed run has no keyboard, so without this `T` reaches
            // nothing the gate can see and r27 measures a path no test drives
            // — the M86 shape. It goes in EARLY and stays open: the screen is
            // closed by nothing here, so every later frame carries its bar and
            // the count is unambiguous.
            if !self.chat_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.2 {
                    // M111 — 25 lines, because the scrollbar's guard is
                    // `virtualHeight != chatHeight`: it does not exist until
                    // there is more chat than the focused box's twenty rows,
                    // and a run's own join messages come to about six. Without
                    // these r28 would be a witness over a path the gate cannot
                    // reach, which is worse than no witness.
                    //
                    // Injected as raw `system_chat` bodies through the
                    // production router (M17's rule: injection is the
                    // deterministic proof where a live trigger depends on
                    // timing nothing here controls).
                    if let Some(session) = self.session.as_mut() {
                        let id = session.ids.cb_play_system_chat;
                        for i in 0..25u8 {
                            let text = format!("scrollbar filler {i}");
                            let mut body: Vec<u8> = vec![8];
                            body.extend_from_slice(&(text.len() as u16).to_be_bytes());
                            body.extend_from_slice(text.as_bytes());
                            body.push(0); // overlay = false
                            if let Some(pid) = id {
                                session.inject_packet(pid, &body);
                            }
                        }
                    }
                    // M126d — one message carrying legacy codes, so the
                    // drawn chat has more than one colour and at least one
                    // non-plain flag. Injected as a raw `system_chat` body
                    // through the production router (M17's rule) rather than
                    // sent as chat, because a server owns what it echoes back
                    // and `§` in a player message is not something the gate
                    // can rely on surviving the round trip.
                    //
                    // Every stage has to work for this to score: the NBT
                    // string reaches `parse_component`, `push_legacy` resolves
                    // the codes into five spans, `FlatComponents` carries them
                    // through the wrap as separate parts, and `chat_lines`
                    // emits one text line each. A flatten anywhere in that
                    // chain drops both counters to zero.
                    if let Some(session) = self.session.as_mut() {
                        if let Some(pid) = session.ids.cb_play_system_chat {
                            let text = concat!(
                                "\u{00a7}crewored \u{00a7}9blue ",
                                "\u{00a7}oital \u{00a7}nunder \u{00a7}mstrike"
                            );
                            let mut body: Vec<u8> = vec![8];
                            body.extend_from_slice(&(text.len() as u16).to_be_bytes());
                            body.extend_from_slice(text.as_bytes());
                            body.push(0); // overlay = false
                            session.inject_packet(pid, &body);
                        }
                    }
                    // M115 — a completion word, then a keystroke, so r29
                    // measures the whole production chain rather than a
                    // hand-built `Suggestions`: the packet reaches
                    // `SuggestionProviderState`, `tab_words()` unions it with
                    // the online players, `on_edited` matches the typed prefix
                    // against it, and `auto_show` opens the list. A break
                    // anywhere in that drops the count to zero.
                    //
                    // The word is deliberately nothing a server would send, so
                    // it cannot be confused with a real player's name, and it
                    // begins with the character typed below.
                    if let Some(session) = self.session.as_mut() {
                        if let Some(pid) = Some(session.ids.cb_play_custom_chat_completions) {
                            let words = ["rewopopupwitness", "rewopopupsecond"];
                            let mut body: Vec<u8> = Vec::new();
                            body.push(2); // Action.SET
                            body.push(words.len() as u8);
                            for w in words {
                                body.push(w.len() as u8);
                                body.extend_from_slice(w.as_bytes());
                            }
                            session.inject_packet(pid, &body);
                        }
                    }
                    self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Message);
                    // `onEdited` is what turns suggestions on and asks for
                    // them; nothing else in a windowed run types.
                    self.chat_char('r');
                    self.chat_injected = true;
                }
            }
            // M116 — later, and as its own screen, so r29's frames stay
            // unambiguously the message popup's. Reopening with `Command`
            // seeds the field with `/`; one letter then reaches the top-level
            // literals, which the dispatcher answers without a packet.
            if self.chat_injected && !self.command_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.55 {
                    self.close_chat_screen();
                    self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Command);
                    self.chat_char('g');
                    // …and then on to `/give `, which is where an ARGUMENT is
                    // expected: the integer child suggests nothing, so no
                    // popup opens and the USAGE box takes its place. r30 has
                    // already counted the literal completion `/g` produced;
                    // this reaches r31 and r32.
                    // …then on to `/give @s `, which is two arguments deep.
                    // M118 changed what `/give ` alone shows: its first
                    // argument is `minecraft:entity`, so a SELECTOR POPUP
                    // opens there and the usage box is suppressed by the
                    // mutual exclusion — which is what vanilla does too. The
                    // box needs an argument that suggests nothing, and the
                    // item after the targets is one.
                    // …then on to `/give @s dirt `, which is three arguments
                    // deep. **r32's precondition recedes by one word for every
                    // argument type transcribed**: M118 made `/give ` open a
                    // selector popup and M119 made `/give @s ` open an item
                    // one, each time suppressing the usage box by the mutual
                    // exclusion. The count after the item is a plain integer
                    // and suggests nothing, which is what the box needs.
                    for ch in "ive @s dirt ".chars() {
                        self.chat_char(ch);
                    }
                    self.command_injected = true;
                }
            }
            // M120 — and a coordinate. Injected LAST, at 0.8, so r32 has
            // already banked its frames against `/give @s dirt `: a
            // coordinate popup suppresses the box, which is the receding
            // precondition M118 and M119 both hit.
            if self.command_injected && !self.coords_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.8 {
                    self.close_chat_screen();
                    // **Drop the draft first.** `close_chat_screen` saves one,
                    // and `ChatMethod::Command` restores a COMMAND draft — so
                    // reopening handed the field back `/give @s dirt ` and the
                    // typing below appended to it. That is M110's
                    // `isDraftRestorable` working exactly as documented, and
                    // the gate read as "the coordinate family offers nothing"
                    // when what it had actually typed was
                    // `/give @s dirt setblock `.
                    self.chat_draft = None;
                    self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Command);
                    for ch in "setblock ".chars() {
                        self.chat_char(ch);
                    }
                    self.coords_injected = true;
                }
            }
            // M124 — a literal table, injected LAST at 0.9 for the reason
            // every one of these has been: whichever popup is open last
            // suppresses the usage box, and r32 banks its frames earlier.
            if self.coords_injected && !self.literal_table_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.9 {
                    self.close_chat_screen();
                    // The draft drop M120 needed, for M110's reason.
                    self.chat_draft = None;
                    self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Command);
                    for ch in "scoreboard objectives setdisplay ".chars() {
                        self.chat_char(ch);
                    }
                    self.literal_table_injected = true;
                }
            }
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
            // M94 — last of all, a CRAFTING TABLE, because the book only draws
            // while a book menu is on screen and neither of the two injections
            // below is one: `book_type_of` answers for the player's own
            // inventory, `crafting`, and the three furnaces, and nothing else.
            // The chest opened at 0.4 holds the screen for the rest of the run,
            // so without this the windowed client never reaches the book's
            // builder however long it runs — the same shape of gap as M92's
            // overlay injection, one screen over.
            //
            // Last (0.90) so `r20` and `r22` have already latched: a crafting
            // table's panel is 176x166, the player's own size, and it draws no
            // overlays.
            if !self.book_menu_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.90 {
                    if let Some(session) = self.session.as_mut() {
                        // Menu type 12. NOT 13 - that is `enchantment`,
                        // which this first got wrong, and which opened a
                        // perfectly valid screen with no book.
                        let mut body: Vec<u8> = vec![11, 12, 8];
                        let title = b"Crafting";
                        body.extend_from_slice(&(title.len() as u16).to_be_bytes());
                        body.extend_from_slice(title);
                        let open_id = session.ids.cb_play_open_screen;
                        if rewo_net::route_menu(open_id, &body, &session.ids, &mut session.menus) {
                            self.book_menu_injected = true;
                        }
                    }
                }
            }
            // M104 — and then the which-of-these overlay, through the SAME
            // `open_overlay` a right-click calls.
            //
            // Injected rather than clicked because a click needs the cursor
            // over a particular cell AND a server that has sent a multi-recipe
            // group, neither of which this gate controls — M17's rule. It must
            // come after the crafting table (0.90), because `set_screen_open`
            // clears the overlay: a snapshot of a page must not survive the
            // screen it was taken on.
            if !self.book_overlay_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.95 && self.book_menu_injected {
                    // Only `filtering` and `furnace_family` are read out of
                    // the view, and both are false for a crafting table's
                    // unfiltered book — the rest is named so the fixture reads
                    // as a whole `BookView` rather than a partial one.
                    let view = rewo_world::recipe_book_screen::BookView {
                        tabs: rewo_world::recipe_book_screen::CRAFTING_TABS.len(),
                        selected_tab: 0,
                        page: 0,
                        total_pages: 1,
                        shown: 1,
                        filtering: false,
                        furnace_family: false,
                    };
                    let collection = (0..3)
                        .map(|i| rewo_world::recipe_overlay::Button {
                            recipe: i,
                            craftable: i == 0,
                            slots: Vec::new(),
                        })
                        .collect();
                    self.screen.book_overlay = Some(open_overlay(collection, 0, view));
                    self.book_overlay_injected = true;
                }
            }
            // M92 — near the end, replace it with a BREWING STAND and give it
            // data, so the frame loop has to reach the overlay builder.
            //
            // A chest has no overlays at all, so the injection above cannot
            // exercise this path however long it runs. `containershot` grades
            // the overlays offscreen; without this, nothing says the windowed
            // client ever draws one — the gap M88 closed for the panel and M86
            // for nine features before that.
            //
            // Late (0.85) and after `r20` has latched its height, because a
            // brewing stand's panel is 166 tall — the same as the player's —
            // and so cannot serve `r20`'s discrimination.
            if !self.brewing_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.85 {
                    if let Some(session) = self.session.as_mut() {
                        let mut body: Vec<u8> = vec![9, 11, 8];
                        let title = b"Brewing Stand";
                        body.extend_from_slice(&(title.len() as u16).to_be_bytes());
                        body.extend_from_slice(title);
                        let open_id = session.ids.cb_play_open_screen;
                        if rewo_net::route_menu(open_id, &body, &session.ids, &mut session.menus) {
                            // ...and its data: 200 ticks left of a brew, 20
                            // charges of fuel. Both packets through the
                            // production router (M17's precedent), so the
                            // decode, the id gate and the data-slot write are
                            // all the shipped ones.
                            let data_id = session.ids.cb_play_container_set_data;
                            for (slot, value) in [(0i16, 200i16), (1, 20)] {
                                // VarInt container id, then two BE i16s —
                                // fixed-width shorts among the var-ints (M87).
                                let mut d = vec![9u8];
                                d.extend_from_slice(&slot.to_be_bytes());
                                d.extend_from_slice(&value.to_be_bytes());
                                rewo_net::route_menu(data_id, &d, &session.ids, &mut session.menus);
                            }
                            self.brewing_injected = true;
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
            // M108 — `--render-check` sends its own chat line rather than
            // making this a THIRD caller requirement beside r14's hotbar and
            // r25's recipe book. The server echoing it back is what drives
            // `player_chat` through the signature cache, the trust level, the
            // wrap and the geometry, so r26 grades the whole chain on an
            // otherwise unstaged run.
            if self.check.is_some() {
                let _ = session.send_chat("rewo render-check");
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
            .then(|| {
                self.screen
                    .hovered(session.shown_menu().layout(), sw, sh, book_visible(session))
            })
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
                let beacon_override = session
                    .menus
                    .open()
                    .filter(|m| m.layout.protocol_id == BEACON_MENU_PROTOCOL_ID)
                    .map(|m| beacon_live(&mut self.screen, m, &self.beacon_effects));
                // M93s — likewise, resolved before the borrow of `state`.
                let cut = session
                    .menus
                    .open()
                    .filter(|m| {
                        m.layout.protocol_id
                            == rewo_world::menu_screen::STONECUTTER_MENU_PROTOCOL_ID
                    })
                    .map(|m| cut_view(&mut self.screen, m, &items));
                // M93t — likewise the anvil's field, seeded here so the render
                // and the key handler see one box.
                // M98 — the book's own selection, from the screen state.
                let book_state = self.screen.book;
                // M99 — and its search text, lowercased once here.
                let book_query =
                    rewo_world::recipe_search::normalize(&self.screen.book_search.value());
                let anvil_field = session
                    .menus
                    .open()
                    .filter(|m| m.layout.protocol_id == ANVIL_MENU_PROTOCOL_ID)
                    .map(|m| anvil_local(&mut self.screen, m, &items).field.clone());
                // M93u — the merchant's trade list, from the session's decoded
                // offers plus the screen's own scroll.
                let merchant = session
                    .menus
                    .open()
                    .filter(|m| {
                        m.layout.protocol_id
                            == rewo_world::merchant_screen::MERCHANT_MENU_PROTOCOL_ID
                    })
                    .zip(session.merchant.as_ref())
                    .map(|(m, offers)| {
                        // The clamp's ceiling is the item's own max stack size,
                        // through the SAME production resolver every other
                        // consumer uses (M93b's rule).
                        let props =
                            |id: i32| item_props(&items, id).map_or(64, |p| p.max_stack);
                        merchant_view(&mut self.screen, m, offers, &props)
                    });
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
                    self.beacon_effects,
                    // M93m — the SCREEN's choice, so a click lights the
                    // button it pressed instead of the render continuing to
                    // paint the server's last word.
                    beacon_override,
                    cut.as_ref(),
                    anvil_field.as_ref(),
                    merchant.as_ref(),
                    book_state,
                    &book_query,
                    &self.screen.book_search,
                    self.started.elapsed().as_millis() as u64,
                    self.screen.book_overlay.as_ref(),
                );
                // M105 — the page counter is a LABEL, so it is counted here
                // rather than among the book's quads. Matched on the
                // model's own geometry (the counter is the only text the book
                // draws on that row) rather than on its content, which is a
                // translation and would tie the gate to a language.
                if let Some(c) = self.check.as_mut() {
                    let (_, bt, sc) = rewo_gpu::container::recipe_book_origin(sw, sh);
                    let row = bt
                        + rewo_world::recipe_book_screen::PAGE_LABEL_Y as f32 * sc;
                    if labels.iter().any(|l| (l.y - row).abs() < 0.5) {
                        c.book_page_label_frames += 1;
                    }
                }
                self.screen_labels = labels;
                // M94 — OUTSIDE the `container_panel_height` guard below: the
                // player's own inventory has no container panel and is the
                // commonest screen with a book, so counting inside that guard
                // measures zero forever. r23's first two red runs were this
                // and the field placement it forced.
                // M104 — split by whether an overlay was up, because the claim
                // one frame over is a DIFFERENCE and not a threshold.
                let overlay_up = self.screen.book_overlay.is_some();
                if let Some(c) = self.check.as_mut() {
                    let q = state.world_renderer.container_panel_book_quads();
                    if overlay_up {
                        c.book_overlay_quads_max = c.book_overlay_quads_max.max(q);
                    } else {
                        c.book_quads_max = c.book_quads_max.max(q);
                    }
                }
                // M88 — read the panel back OUT of the renderer, after the
                // draw path set it. Asking the open menu's layout instead
                // would answer 168 for a chest whether or not the panel
                // builder returned one.
                if let Some(h) = state.world_renderer.container_panel_height() {
                    let forced = self.screen_forced_open;
                    if let Some(c) = self.check.as_mut() {
                        c.container_frames += 1;
                        // FIRST sighting, not the last: M92 injects a
                        // second container late (a brewing stand, 166 tall
                        // like the player's), and r20's question is about the
                        // chest that proved the panel builder runs.
                        c.container_panel_h.get_or_insert(h);
                        c.container_overlays_max = c
                            .container_overlays_max
                            .max(state.world_renderer.container_panel_overlays());
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
        // Drain the frame's chat events into the store *before* building the
        // text, or a message that arrived this frame is a frame late. The
        // store needs the font and the GUI clock, which is why this cannot
        // live where the packets are decoded.
        apply_chat(session, state.world_renderer.font_advance().copied());
        // M117 — both command-line overlays, resolved once before the fills
        // stage so the box and the highlighting read the same cached parse.
        // The names come from the session that is already borrowed here, so
        // they are read directly rather than through `tab_words`, which would
        // need `&self` while the session holds `&mut self`.
        let chat_words = session
            .suggestions
            .tab_suggestions(session.world.entities.all_names());
        let chat_runs = chat_runs(
            &mut self.chat_parse,
            self.chat_screen.as_ref(),
            session,
            rewo_net::dispatcher::CommandCtx {
                names: &chat_words,
                blocks: Some(&self.blocks),
                items: Some(&self.items),
            },
        );
        let (usage_fills, usage_text) = Self::usage_box_parts(
            self.chat_screen.as_ref(),
            session,
            &self.chat_parse,
            state.world_renderer.font_advance().copied(),
            {
                let s = gui_px(extent.width, extent.height);
                (
                    (extent.width as f32 / s) as i32,
                    (extent.height as f32 / s) as i32,
                )
            },
            px,
        );
        // A `command_suggestions` reply that matched the outstanding request
        // (M114). Drained here rather than at decode time for the same reason
        // the chat events are: opening the popup needs the font to measure its
        // widest entry, which `rewo-net` does not have.
        if let Some(reply) = session.suggestion_reply.take() {
            let advance = state.world_renderer.font_advance().copied();
            let width_of = move |t: &str| match &advance {
                Some(a) => rewo_gpu::text::width(t, a),
                None => 0,
            };
            let px = gui_px(extent.width, extent.height);
            let (gui_w, gui_h) = (
                (extent.width as f32 / px) as i32,
                (extent.height as f32 / px) as i32,
            );
            let (ix, _iy, iw, _ih) = rewo_world::chat_screen::input_rect(gui_w, gui_h);
            let env = rewo_world::chat_screen::SuggestionEnv {
                metrics: rewo_world::command_suggestions::InputMetrics {
                    x: ix,
                    inner_width: iw,
                    screen_height: gui_h,
                },
                width: &width_of,
                tab_words: &[],
                auto_suggestions: true,
            };
            if let Some(s) = self.chat_screen.as_mut() {
                s.accept_suggestions(reply, &env);
            }
        }
        // `ChatComponent.isChatFocused()` — `gui.screen() instanceof
        // ChatScreen`. It changes the box height, suppresses the fade, and is
        // what `scrollChat` clamps against, so the two derivations below and
        // the key handler must all read the same answer.
        let chat_focused = self.chat_screen.is_some();
        // M109 — the fills, from the same `visible_lines` the text comes from
        // so a row's backdrop and its glyphs cannot disagree about which rows
        // exist. Set every frame, including when it is empty: a stale backdrop
        // under nothing is a black bar hanging over the world.
        let mut backdrops = hud_fills(
            &session.chat,
            session.ticks as i32,
            px,
            extent.height as f32,
            &rewo_world::chat::ChatOptions::default(),
            chat_focused,
        );
        if chat_focused {
            // The input bar goes on the same list AFTER the rows, so it sits
            // over them — one list, and its order is the order on screen.
            let gw = (extent.width as f32 / px) as i32;
            let gh = (extent.height as f32 / px) as i32;
            backdrops.push(chat_input_backdrop(px, gw, gh));
            // M111 — the scrollbar, which only exists while the screen is up
            // (`isForeground`). It reads the same `visible_lines` count the
            // rows do, because vanilla passes `forEachLine`'s own return here.
            let bar = chat_scrollbar(
                &session.chat,
                session.ticks as i32,
                px,
                extent.height as f32,
                &rewo_world::chat::ChatOptions::default(),
            );
            if !bar.is_empty() {
                if let Some(c) = self.check.as_mut() {
                    c.chat_scrollbar_frames += 1;
                }
            }
            backdrops.extend(bar);
            // M115 — the suggestion popup, last on the list so it sits over
            // the input bar and the rows, which is where
            // `ChatScreen.extractRenderState` hands off to it.
            if let Some(cs) = self.chat_screen.as_ref() {
                if let Some(list) = cs.suggestions.list() {
                    let fills = suggestion_popup_fills(list, cs.suggestions.config(), px);
                    if !fills.is_empty() {
                        if let Some(c) = self.check.as_mut() {
                            c.suggestion_popup_frames += 1;
                        }
                    }
                    backdrops.extend(fills);
                } else {
                    // M117 — `extractRenderState` is
                    // `if (!extractSuggestions(..)) extractUsage(..)`, so the
                    // box exists only when the popup does not. Drawing both
                    // stacks two panels over one field.
                    if !usage_fills.is_empty() {
                        if let Some(c) = self.check.as_mut() {
                            c.usage_box_frames += 1;
                        }
                    }
                    backdrops.extend(usage_fills.clone());
                }
            }
            if let Some(c) = self.check.as_mut() {
                c.chat_screen_frames += 1;
            }
        }
        state.world_renderer.set_hud_fills(backdrops);
        // The input bar goes on the same list, after the rows, so it sits over
        // them the way `ChatScreen.extractRenderState` draws its fill before
        // handing off to the chat component — one list, and the order in it is
        // the order on screen.

        let fps = (!self.cpu.is_empty()).then(|| 1000.0 / self.cpu.average().max(0.001));
        let (mut text, chat_rows, chat_range) = build_text(
            session,
            px,
            extent.height as f32,
            fps,
            self.debug,
            chat_focused,
            state.world_renderer.font_advance().copied(),
        );
        if let Some(cs) = self.chat_screen.as_ref() {
            let (gw, gh) = ((extent.width as f32 / px) as i32, (extent.height as f32 / px) as i32);
            let advance = state.world_renderer.font_advance().copied();
            let width_of = move |s: &str| match &advance {
                Some(a) => rewo_gpu::text::width(s, a),
                None => 0,
            };
            if chat_runs.is_some() {
                if let Some(c) = self.check.as_mut() {
                    c.highlighted_command_frames += 1;
                }
            }
            text.extend(chat_input_lines(
                cs,
                px,
                gw,
                gh,
                self.started.elapsed().as_millis() as u64,
                chat_runs.as_deref(),
                &width_of,
            ));
            if let Some(list) = cs.suggestions.list() {
                text.extend(suggestion_popup_text(list, cs.suggestions.config(), px));
            }
        }
        if self
            .chat_screen
            .as_ref()
            .is_some_and(|cs| cs.suggestions.list().is_none())
        {
            text.extend(usage_text);
        }
        if !chat_rows.is_empty() {
            if let Some(c) = self.check.as_mut() {
                c.chat_line_frames += 1;
            }
        }
        // M125 — read off the DRAWN lines rather than the chat store, so a
        // resolution that happened and then failed to reach the frame is not
        // counted.
        //
        // **M126b split the two scans, and the reason is asymmetric.** A chat
        // row is now one text line per SPAN, and a resolved template is
        // several spans ("Gave ", "1", " ", "[", "Diamond Sword", "]", …) — so
        // scanning `text` for the whole sentence finds nothing and the witness
        // would read zero with the feature working. `chat_rows` is that same
        // drawn row re-concatenated by `chat_lines`, from the same spans it
        // emitted, so it keeps M125's "off the drawn lines" property.
        //
        // The raw-key scan does NOT need it: an unresolved translatable falls
        // back to its key as a SINGLE span, by construction, so it still lands
        // in one `text` line — and scanning the wider list keeps the check that
        // no key leaked into some other surface.
        if let Some(c) = self.check.as_mut() {
            // M126d — over the chat lines this frame actually drew, and
            // **within one row**. Across the whole box is a much weaker claim
            // that a client with no span pipeline satisfies for free: the
            // section-sign message's first span is red while the filler rows
            // are white, so a truncate-to-one-span mutation left the
            // across-the-box version green. The battery caught that; this is
            // the corrected witness.
            //
            // `color` is an `[f32; 3]`, so distinctness is by bits — two spans
            // that resolved to the same colour ARE the same colour, and a
            // tolerance would only blur the claim.
            let drawn = &text[chat_range.clone()];
            let mut at = 0usize;
            let mut multi = false;
            for (_, n) in &chat_rows {
                let row = &drawn[at.min(drawn.len())..(at + n).min(drawn.len())];
                let mut colors: Vec<[u32; 3]> = row
                    .iter()
                    .map(|l| {
                        [
                            l.color_linear[0].to_bits(),
                            l.color_linear[1].to_bits(),
                            l.color_linear[2].to_bits(),
                        ]
                    })
                    .collect();
                colors.sort_unstable();
                colors.dedup();
                multi |= colors.len() > 1;
                at += n;
            }
            if multi {
                c.styled_chat_frames += 1;
            }
            if drawn
                .iter()
                .any(|l| l.style != rewo_gpu::text::TextStyle::PLAIN)
            {
                c.flagged_chat_frames += 1;
            }
            if chat_rows
                .iter()
                .any(|(r, _)| r.contains("Gave 1 [Diamond Sword]"))
            {
                c.translated_chat_frames += 1;
            }
            // The three keys of that one message, one per nesting level. Named
            // exactly rather than detected generally: a "looks like a key"
            // test would fire on the F3 block's coordinates and on any player
            // whose name has a dot in it.
            const RAW_KEYS: [&str; 3] = [
                "commands.give.success",
                "chat.square_brackets",
                "item.minecraft.diamond_sword",
            ];
            if text
                .iter()
                .any(|l| RAW_KEYS.iter().any(|k| l.text.contains(k)))
            {
                c.unresolved_key_frames += 1;
            }
        }
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
                // M125 — the same handover `experience_level_lines` above
                // takes, so `/title {"translate":...}` resolves.
                self.baked.as_ref().map(|b| &b.lang),
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
    blocks: std::sync::Arc<rewo_data::blocks::Blocks>,
    beacon_effects: BeaconEffectIds,
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

/// Drain the session's chat events into its [`rewo_world::chat::ChatComponent`].
///
/// The seam exists because the store needs two things the wire cannot supply:
/// the font to wrap against (`addMessageToDisplayQueue` calls
/// `message.splitLines(font, maxWidth)`) and the GUI tick to stamp
/// `addedTime` with. `session.ticks` is the 20 Hz session tick, which is the
/// same rate and the same purpose as vanilla's `Gui.getGuiTicks()`.
///
/// **With no font the events are still drained**, and a zero width is the
/// right measurement rather than a placeholder. `LineBreakFinder`'s
/// `hadNonZeroWidthChar` guard only lets the overflow test fire *after* a
/// character of non-zero width has been accepted, so a font that measures
/// everything at 0 never overflows and the message stays whole — one line per
/// `\n`, no wrapping. Dropping the events instead would lose them
/// permanently and queueing them would grow without bound, so keeping them
/// unwrapped is what a caller with no font can honestly render.
fn apply_chat(session: &mut PlaySession, advance: Option<[u8; 256]>) {
    let width_of = move |t: &str, st: rewo_world::chat_style::ChatStyle| match &advance {
        Some(a) => rewo_gpu::text::width_styled(t, a, st.bold),
        None => 0,
    };
    let ctx = rewo_world::chat::WrapContext {
        options: rewo_world::chat::ChatOptions::default(),
        focused: false,
        width_of: &width_of,
        deleted_marker_text: DELETED_CHAT_MESSAGE,
    };
    let tick = session.ticks as i32;
    session.apply_chat_events(tick, &ctx);
}

/// `-2039584` — `EditBox.textColor`'s default, and `DebugScreenOverlay`'s
/// literal for every F3 line.
///
/// One constant because vanilla writes the same number in both places, not
/// because Rewo chose to share it: `EditBox` declares
/// `private int textColor = -2039584;` and `DebugScreenOverlay.renderLines`
/// calls `graphics.text(this.font, line, left, top, -2039584, false)`.
///
/// It is **not** white. Every text surface that wanted "the off-white vanilla
/// uses" and reached for a literal instead got a different wrong number.
pub(crate) const EDIT_BOX_TEXT_COLOR: u32 = 0xE0_E0E0;

/// `chat.deleted_marker`'s English value.
///
/// **The literal, not a `baked.lang` lookup**, and that is a divergence rather
/// than a shortcut: vanilla resolves the key and also styles the marker
/// `GRAY, ITALIC`, neither of which this path does. The style is unreachable
/// (the HUD's text producer takes one colour per line and no italic), and the
/// lookup is reachable — `rewo_data`'s language map is loaded — but would give
/// the same string on the English-only build Rewo ships. Named here so it is a
/// known one-line change when the HUD grows spans, not an oversight.
const DELETED_CHAT_MESSAGE: &str = "This chat message has been deleted by the server.";

/// Build this frame's overlay text: an F3-style debug block (top-left, when
/// `debug`) and the chat box (above the hotbar). GUI scale
/// `px`; `fps` is shown in the header when known (windowed only).
fn build_text(
    session: &PlaySession,
    px: f32,
    screen_h: f32,
    fps: Option<f32>,
    debug: bool,
    chat_focused: bool,
    advance: Option<[u8; 256]>,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(String, usize)>,
    std::ops::Range<usize>,
) {
    use rewo_gpu::world::OwnedTextLine;
    // `DebugScreenOverlay.renderLines` — `graphics.text(font, line, left, top,
    // -2039584, false)`, the same [`EDIT_BOX_TEXT_COLOR`] the chat input uses.
    //
    // It was `[0.93; 3]` from the first screen-space-text commit until M130 —
    // an invented near-white that is neither the byte (`224/255 = 0.878`) nor
    // its linear form (`0.745`). Two independent errors in one literal, which
    // is what a value nobody derived looks like.
    let white = srgb_bytes_to_linear(EDIT_BOX_TEXT_COLOR);
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
                color_linear: white,
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text,
            });
        }
    }
    let (chat, chat_rows) = chat_lines(
        &session.chat,
        session.ticks as i32,
        px,
        screen_h,
        &rewo_world::chat::ChatOptions::default(),
        advance.as_ref(),
        chat_focused,
    );
    // The range the chat occupies, so a witness can read the lines that were
    // DRAWN rather than re-deriving `chat_lines` (M93q). Chat is appended last
    // here; the caller extends further afterwards, which is why this is a
    // range and not "the tail".
    let chat_range = lines.len()..lines.len() + chat.len();
    lines.extend(chat);
    (lines, chat_rows, chat_range)
}

/// The chat screen's input line and caret (M110), in screen pixels.
///
/// `EditBox(font, 4, height - 12, width - 4, 12)` with `setBordered(false)`,
/// so there is no widget chrome — the only fill is the screen's own bar, which
/// [`chat_input_backdrop`] emits, and the text sits at the box's own inset.
///
/// **An untouched restored draft renders grey and italic**
/// (`formatChat` returns `Style.EMPTY.withColor(GRAY).withItalic(true)` while
/// `isDraft`), which is the visual half of the rule that backspace clears the
/// whole field. Rewo's bitmap text pass carries one colour per line and no
/// slant, so the colour is reproduced and the italic is not — named here
/// rather than silently dropped.
fn chat_input_lines(
    screen: &rewo_world::chat_screen::ChatScreen,
    px: f32,
    gui_w: i32,
    gui_h: i32,
    now_ms: u64,
    runs: Option<&[rewo_net::command_format::Run]>,
    width_of: &dyn Fn(&str) -> i32,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_gpu::world::OwnedTextLine;
    let (x, y, _w, h) = rewo_world::chat_screen::input_rect(gui_w, gui_h);
    // `EditBox.renderWidget`'s unbordered text origin: the box's own x, and
    // vertically centred by `(height - 8) / 2` — 2 here, not 0 and not 3.
    let text_x = x as f32 * px;
    let text_y = (y + (h - 8) / 2) as f32 * px;
    // `ChatScreen.formatChat` — a FORMATTER, not a field colour:
    //
    // ```java
    // return this.isDraft
    //    ? FormattedCharSequence.forward(text, Style.EMPTY.withColor(ChatFormatting.GRAY).withItalic(true))
    //    : null;
    // ```
    //
    // so a draft is GRAY **and italic**, and null means "no formatter", i.e.
    // the field's own `textColor` — not white.
    //
    // **The draft grey does NOT reach the caret**, and the mechanism is why:
    // `graphics.text(font, applyFormat(half, …), drawX, textY, color, …)`
    // hands the formatter's style a chance to override `color` per character,
    // where `TextCursorUtils.extractAppendCursor` draws a bare `"_"` **String**
    // with `color` itself. So on a draft the text greys and the caret stays
    // off-white. Two bindings, not one, for exactly that reason.
    let field_color = srgb_bytes_to_linear(EDIT_BOX_TEXT_COLOR);
    let (color, style) = if screen.is_draft() {
        (
            srgb_bytes_to_linear(0xAA_AAAA),
            rewo_gpu::text::TextStyle {
                italic: true,
                ..rewo_gpu::text::TextStyle::PLAIN
            },
        )
    } else {
        (field_color, rewo_gpu::text::TextStyle::PLAIN)
    };
    let value = screen.input.value();
    // M117 — one line per coloured run, laid out with the FONT's advances
    // rather than a fixed six pixels, because the runs must butt up against
    // each other exactly: a wrong width is a visible gap or an overlap, where
    // for the caret it was only ever a pixel or two of drift.
    let mut out: Vec<OwnedTextLine> = Vec::new();
    match runs {
        Some(runs) if !runs.is_empty() => {
            let mut x = text_x;
            for run in runs {
                out.push(OwnedTextLine {
                    x,
                    y: text_y,
                    px,
                    color_linear: srgb_bytes_to_linear(run.color),
                    alpha: 1.0,
                    shadow: true,
                    style: rewo_gpu::text::TextStyle::PLAIN,
                    text: run.text.clone(),
                });
                x += width_of(&run.text) as f32 * px;
            }
        }
        _ => out.push(OwnedTextLine {
            x: text_x,
            y: text_y,
            px,
            color_linear: color,
            alpha: 1.0,
            shadow: true,
            style,
            text: value.clone(),
        }),
    }
    let before: String = value.chars().take(screen.input.cursor_position()).collect();
    // Measured, not counted: the six-pixel approximation put the caret adrift
    // on any line with a narrow glyph in it (`i` is 2 wide, `l` 3), and the
    // ghost hangs off the caret so it drifted too.
    let caret_x = text_x + width_of(&before) as f32 * px;
    // The greyed ghost after the caret (M115), gated on `!insert` —
    // `cursorPos < value.length() || value.length() >= maxLength`. So it shows
    // only with the caret at the END of an under-length field: a ghost drawn
    // mid-string would sit on top of the text after the cursor. Vanilla puts
    // it at `cursorX - 1`, one pixel left of where the caret glyph goes.
    let insert = screen.input.cursor_position() < screen.input.len()
        || screen.input.len() >= screen.input.max_length();
    if !insert {
        if let Some(ghost) = screen.input.suggestion() {
            out.push(OwnedTextLine {
                x: caret_x - px,
                y: text_y,
                px,
                // `-8355712` — 0x808080, converted rather than written out:
                // the literal it replaces WAS the right number (0.216 is
                // `srgb_to_linear(128/255)` to three places), but a magic
                // constant is indistinguishable from a `/255` at a glance,
                // which is how the two colours above it stayed wrong.
                color_linear: srgb_bytes_to_linear(0x80_8080),
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: ghost.to_string(),
            });
        }
    }
    // The caret, as the anvil's field draws it: a `_` at the cursor when the
    // blink says so. `setCanLoseFocus(false)` means it never stops.
    if screen.input.cursor_visible(now_ms) {
        out.push(OwnedTextLine {
            x: caret_x,
            y: text_y,
            px,
            // `extractAppendCursor(…, color, …)` — the FIELD colour, so the
            // caret does not follow the draft's grey (see the binding above).
            color_linear: field_color,
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text: "_".to_string(),
        });
    }
    out
}

/// The suggestion popup's fills (M115), in screen pixels.
///
/// `SuggestionsList.extractRenderState`, fill half. Three kinds of rect and
/// the order is the order on screen:
///
/// 1. When the list is longer than its window, a **1 px bar above and below**
///    in the popup's own fill colour. Both are drawn whenever *either* end is
///    truncated — `limited` is `hasPrevious || hasNext` and gates the pair —
///    so a list scrolled to its top still gets a bar above it.
/// 2. The **dashes**, white, one pixel wide every two, on whichever end has
///    more entries. These are per-pixel `fill` calls in vanilla and stay
///    per-pixel here; merging them into one rect would draw a solid line.
/// 3. One **row fill** per visible line.
fn suggestion_popup_fills(
    list: &rewo_world::command_suggestions::SuggestionsList,
    cfg: rewo_world::command_suggestions::SuggestionsConfig,
    px: f32,
) -> Vec<rewo_gpu::hud::HudFill> {
    use rewo_world::command_suggestions::{INDICATOR_COLOR, LINE_HEIGHT};
    let rect = list.rect;
    let limit = list.shown(cfg) as i32;
    let fill = |x: i32, y: i32, w: i32, h: i32, argb: u32| rewo_gpu::hud::HudFill {
        x: x as f32 * px,
        y: y as f32 * px,
        w: w as f32 * px,
        h: h as f32 * px,
        alpha: ((argb >> 24) & 0xFF) as f32 / 255.0,
        rgb: srgb_bytes_to_linear(argb & 0x00FF_FFFF),
    };
    let mut out = Vec::new();
    let has_previous = list.offset() > 0;
    let has_next = list.entries().len() > list.offset() + limit as usize;
    if has_previous || has_next {
        out.push(fill(rect.x, rect.y - 1, rect.w, 1, cfg.fill_color));
        out.push(fill(rect.x, rect.y + rect.h, rect.w, 1, cfg.fill_color));
        for (edge, present) in [(rect.y - 1, has_previous), (rect.y + rect.h, has_next)] {
            if !present {
                continue;
            }
            let mut x = 0;
            while x < rect.w {
                out.push(fill(rect.x + x, edge, 1, 1, INDICATOR_COLOR));
                x += 2;
            }
        }
    }
    for i in 0..limit {
        out.push(fill(
            rect.x,
            rect.y + LINE_HEIGHT * i,
            rect.w,
            LINE_HEIGHT,
            cfg.fill_color,
        ));
    }
    out
}

/// The suggestion popup's text (M115), in screen pixels.
///
/// One line per visible row at `rect.x + 1`, `rect.y + 2 + 12 * i`, yellow for
/// the selected entry and grey for the rest. `graphics.text`'s five-argument
/// form drops a shadow (M105), so these do too.
fn suggestion_popup_text(
    list: &rewo_world::command_suggestions::SuggestionsList,
    cfg: rewo_world::command_suggestions::SuggestionsConfig,
    px: f32,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_world::command_suggestions::{LINE_HEIGHT, SELECTED_COLOR, UNSELECTED_COLOR};
    let rect = list.rect;
    let limit = list.shown(cfg);
    (0..limit)
        .filter_map(|i| {
            let index = i + list.offset();
            let entry = list.entries().get(index)?;
            let argb = if index == list.current() {
                SELECTED_COLOR
            } else {
                UNSELECTED_COLOR
            };
            Some(rewo_gpu::world::OwnedTextLine {
                x: (rect.x + 1) as f32 * px,
                y: (rect.y + 2 + LINE_HEIGHT * i as i32) as f32 * px,
                px,
                color_linear: srgb_bytes_to_linear(argb & 0x00FF_FFFF),
                alpha: ((argb >> 24) & 0xFF) as f32 / 255.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: entry.text.clone(),
            })
        })
        .collect()
}

/// The coloured runs for the chat field, or `None` when there is nothing to
/// colour (M117).
///
/// `formatChat` returns null while `currentParse` is null, and
/// `updateCommandInfo` only ever builds one for a `/`-command — so an ordinary
/// chat message is drawn in the field's own colour, which is a state vanilla
/// passes through too.
///
/// A free function over the three pieces it needs rather than a method,
/// because the frame already holds the session borrowed and `&mut self` here
/// would take the whole app with it.
fn chat_runs(
    cache: &mut Option<(String, rewo_net::dispatcher::ParseResults)>,
    screen: Option<&rewo_world::chat_screen::ChatScreen>,
    session: &PlaySession,
    cmd: rewo_net::dispatcher::CommandCtx<'_>,
) -> Option<Vec<rewo_net::command_format::Run>> {
    let value = screen?.input.value();
    if !value.starts_with('/') {
        *cache = None;
        return None;
    }
    if cache.as_ref().map(|(t, _)| t.as_str()) != Some(value.as_str()) {
        let units: Vec<u16> = value.encode_utf16().collect();
        let parsed = rewo_net::dispatcher::parse(&session.commands, &units, 1, cmd);
        *cache = Some((value.clone(), parsed));
    }
    let (_, parsed) = cache.as_ref()?;
    let units: Vec<u16> = value.encode_utf16().collect();
    // `offset` is `displayPos` in vanilla, because `EditBox` renders only the
    // visible substring. Rewo's chat field draws the whole value, so the
    // visible text starts at 0 — this must become `display_pos()` the day the
    // render honours the horizontal scroll, or every colour lands one scroll
    // to the left.
    Some(rewo_net::command_format::format_text(parsed, &units, 0))
}

/// The usage box under the chat field (M117), in screen pixels.
///
/// `CommandSuggestions.extractUsage`. Two things read backwards:
///
/// * The list grows **upward**. `lineY = height - 27 - 12 * y`, so entry 0 is
///   the LOWEST line and each later one sits twelve pixels higher. Laying it
///   out downward from a top puts a two-line box over the field it belongs to.
/// * The fill is one pixel wider than the text **on each side**
///   (`position - 1` to `position + width + 1`), so the box has a hairline of
///   padding the text does not.
///
/// The box and the suggestion popup are **mutually exclusive**:
/// `extractRenderState` is `if (!extractSuggestions(..)) extractUsage(..)`.
fn usage_box(
    lines: &[String],
    position: i32,
    gui_h: i32,
    px: f32,
    fill_color: u32,
    width_of: &dyn Fn(&str) -> i32,
) -> (Vec<rewo_gpu::hud::HudFill>, Vec<rewo_gpu::world::OwnedTextLine>) {
    let box_width = lines.iter().map(|l| width_of(l)).max().unwrap_or(0);
    let mut fills = Vec::new();
    let mut text = Vec::new();
    for (y, line) in lines.iter().enumerate() {
        let line_y = gui_h - rewo_world::command_suggestions::USAGE_OFFSET_FROM_BOTTOM
            - rewo_world::command_suggestions::LINE_HEIGHT * y as i32;
        fills.push(rewo_gpu::hud::HudFill {
            x: (position - 1) as f32 * px,
            y: line_y as f32 * px,
            w: (box_width + 2) as f32 * px,
            h: rewo_world::command_suggestions::LINE_HEIGHT as f32 * px,
            alpha: ((fill_color >> 24) & 0xFF) as f32 / 255.0,
            rgb: srgb_bytes_to_linear(fill_color & 0x00FF_FFFF),
        });
        text.push(rewo_gpu::world::OwnedTextLine {
            x: position as f32 * px,
            y: (line_y + 2) as f32 * px,
            px,
            // `-1` — white.
            color_linear: [1.0, 1.0, 1.0],
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text: line.clone(),
        });
    }
    (fills, text)
}

/// The chat scrollbar's two rects (M111), in screen pixels.
///
/// Reachable only while the chat screen is open, so the caller gates it — and
/// the visible-line count is `forEachLine`'s own return, which is why it comes
/// from the same `visible_lines` the rows and their fills read rather than
/// from `linesPerPage`: a page that is not full gives a shorter thumb.
fn chat_scrollbar(
    chat: &rewo_world::chat::ChatComponent,
    gui_tick: i32,
    px: f32,
    screen_h: f32,
    opts: &rewo_world::chat::ChatOptions,
) -> Vec<rewo_gpu::hud::HudFill> {
    let chat_px = px * opts.scale as f32;
    let chat_bottom = ((screen_h / chat_px) - rewo_world::chat::BOTTOM_MARGIN as f32).floor();
    let max_width = (opts.width() as f32 / opts.scale as f32).ceil();
    let visible = chat.visible_lines(gui_tick, true, opts).len() as i32;
    chat.scrollbar(visible, chat_bottom as i32, max_width as i32, opts)
        .into_iter()
        .flatten()
        .map(|r| rewo_gpu::hud::HudFill {
            // The pose is translated by `MESSAGE_INDENT` before these, exactly
            // as it is before the rows' fills.
            x: (r.x + rewo_world::chat::MESSAGE_INDENT) as f32 * chat_px,
            y: r.y as f32 * chat_px,
            w: r.w as f32 * chat_px,
            h: r.h as f32 * chat_px,
            alpha: r.alpha as f32 / 255.0,
            rgb: srgb_bytes_to_linear(r.rgb),
        })
        .collect()
}

/// The bar behind the input, as a [`rewo_gpu::hud::HudFill`].
///
/// `fill(2, height - 14, width - 2, height - 2, getBackgroundColor(Integer.MIN_VALUE))`
/// — a fixed alpha 128, which does **not** follow the text-background slider
/// the chat rows' own fills read. See
/// [`rewo_world::chat_screen::INPUT_BACKDROP_ALPHA`].
fn chat_input_backdrop(px: f32, gui_w: i32, gui_h: i32) -> rewo_gpu::hud::HudFill {
    let (x, y, w, h) = rewo_world::chat_screen::input_backdrop_rect(gui_w, gui_h);
    rewo_gpu::hud::HudFill {
        x: x as f32 * px,
        y: y as f32 * px,
        w: w as f32 * px,
        h: h as f32 * px,
        alpha: rewo_world::chat_screen::INPUT_BACKDROP_ALPHA,
        // `getBackgroundColor` builds it with `colorFromFloat(_, 0, 0, 0)` —
        // black, like the rows'.
        rgb: [0.0; 3],
    }
}

/// The chat box's text, from `ChatComponent.extractRenderState`.
///
/// ```java
/// float scale = (float)this.getScale();
/// int chatBottom = Mth.floor((screenHeight - 40) / scale);
/// pose.scale(scale, scale); pose.translate(4.0F, 0.0F);
/// int entryBottom = chatBottom - lineIndex * entryHeight;
/// int textTop     = entryBottom - entryBottomToMessageY;
/// ```
///
/// Lifted out of [`build_text`] rather than written inline because
/// `build_text` takes a `&PlaySession`, which owns a socket and cannot be
/// constructed in a test — M97's lesson, and the arithmetic here is exactly
/// what a witness needs to reach.
///
/// **Two things this deliberately does not draw**, named rather than quietly
/// missing:
///
/// * **The backdrop fills.** `graphics.fill(-4, entryTop, maxWidth + 4 + 4,
///   entryBottom, ARGB.black(alpha * backgroundOpacity))` needs a per-quad
///   alpha, and [`rewo_gpu::hud`]'s vertex is `vec2 pos + vec2 uv` with **no
///   colour channel** — the cooldown overlay gets its tint from a texel baked
///   into the atlas, which cannot carry a varying fade. Adding one is a
///   vertex-format change: stride 16 → 32, both `hud` shaders, and the
///   `v.len() * 16` hardcode sitting beside `VERTEX_STRIDE`, which is the
///   shape M21 found in the entity pass and which produced a silently
///   truncated upload there.
/// * **The scrollbar**, which needs the same plus a second colour, and which
///   nothing can reach anyway until there is a chat screen to scroll from.
///
/// The *text* half is complete, and the fade works today only because
/// `OwnedTextLine::alpha` already exists — its doc comment says "chat fades
/// old lines", written before there was a chat that faded.
fn chat_lines(
    chat: &rewo_world::chat::ChatComponent,
    gui_tick: i32,
    px: f32,
    screen_h: f32,
    opts: &rewo_world::chat::ChatOptions,
    advance: Option<&[u8; 256]>,
    focused: bool,
) -> (Vec<rewo_gpu::world::OwnedTextLine>, Vec<(String, usize)>) {
    use rewo_gpu::world::OwnedTextLine;
    // `getScale()` is a second multiplier on top of the GUI scale, so a chat
    // pixel is `px * scale` screen pixels and every offset below is in chat
    // pixels.
    let chat_px = px * opts.scale as f32;
    let chat_bottom = ((screen_h / chat_px) - rewo_world::chat::BOTTOM_MARGIN as f32).floor();
    let entry_height = opts.entry_height() as f32;
    let to_message_y = opts.entry_bottom_to_message_y() as f32;
    let text_opacity = opts.text_opacity();
    // Focused chat is a taller box with no fade. Supplied by the caller
    // (M110) rather than hardcoded: it is a fact about whether a `ChatScreen`
    // is open, which is exactly what `ChatComponent.isChatFocused` asks.
    let mut out: Vec<OwnedTextLine> = Vec::new();
    // Per row: its characters, and how many text lines it emitted — the
    // second is what lets a witness ask about ONE row rather than about the
    // whole box, which is a different and much weaker claim.
    let mut rows: Vec<(String, usize)> = Vec::new();
    for line in chat.visible_lines(gui_tick, focused, opts) {
        let entry_bottom = chat_bottom - line.index as f32 * entry_height;
        let y = (entry_bottom - to_message_y) * chat_px;
        // `pose.translate(4.0F, 0.0F)` — `MESSAGE_INDENT`.
        let mut pen = rewo_world::chat::MESSAGE_INDENT as f32 * chat_px;
        let mut row = String::new();
        let row_start = out.len();
        for span in line.text {
            let w = advance
                .map(|a| rewo_gpu::text::width_styled(&span.text, a, span.bold))
                .unwrap_or(0);
            if !span.text.is_empty() {
                out.push(OwnedTextLine {
                    x: pen,
                    y,
                    px: chat_px,
                    // The span's own colour, in the LINEAR space the pass
                    // writes into an sRGB attachment. M117's coloured command
                    // runs are the precedent; the death screen and the XP
                    // level still hand over `/255` bytes and are a hair bright
                    // (named in the plan, not fixed here).
                    color_linear: srgb_bytes_to_linear_f(span.color),
                    // `alpha * textOpacity`, where `textOpacity` is
                    // `chatOpacity * 0.9 + 0.1` and so never reaches 0.
                    alpha: line.alpha * text_opacity,
                    shadow: true,
                    style: rewo_gpu::text::TextStyle {
                        bold: span.bold,
                        italic: span.italic,
                        underlined: span.underlined,
                        strikethrough: span.strikethrough,
                        obfuscated: span.obfuscated,
                    },
                    text: span.text.clone(),
                });
            }
            row.push_str(&span.text);
            pen += w as f32 * chat_px;
        }
        rows.push((row, out.len() - row_start));
    }
    (out, rows)
}

/// A span's already-unpacked `[f32; 3]` (sRGB, `chat_style::rgb_f32`'s plain
/// `/255`) into linear.
///
/// Beside [`srgb_bytes_to_linear`] rather than folded into it because the
/// input is a triple that has already been divided, not a packed `u32` — and
/// both go through `rewo_gpu`'s one transfer function for M111's reason.
pub(crate) fn srgb_bytes_to_linear_f(rgb: [f32; 3]) -> [f32; 3] {
    [
        srgb_to_linear(rgb[0]),
        srgb_to_linear(rgb[1]),
        srgb_to_linear(rgb[2]),
    ]
}

/// An 0xRRGGBB byte triple into the LINEAR space the HUD's vertex tint
/// multiplies in.
///
/// The attachment is SRGB and the atlas is an SRGB image, so `texture()` has
/// already decoded by the time the tint applies — a caller handing over the
/// stored byte would be a third of a stop too bright. Black hid this for two
/// milestones because it is 0 in both spaces; the scrollbar's `0x3333AA` does
/// not. Same finding as M50's glint, one pass over.
/// **Built on `rewo_gpu`'s per-channel `srgb_to_linear`** rather than a second
/// transfer function: two copies of a curve are two chances to differ by a
/// hundredth, and this one is already the renderer's.
pub(crate) fn srgb_bytes_to_linear(rgb: u32) -> [f32; 3] {
    let ch = |shift: u32| srgb_to_linear(((rgb >> shift) & 0xFF) as f32 / 255.0);
    [ch(16), ch(8), ch(0)]
}

/// The chat box's backdrop fills (M109), in GUI pixels.
///
/// ```java
/// int count = this.forEachLine(alphaCalculator, (line, lineIndex, alpha) -> {
///    int entryBottom = chatBottom - lineIndex * entryHeight;
///    int entryTop = entryBottom - entryHeight;
///    graphics.fill(-4, entryTop, maxWidth + 4 + 4, entryBottom, ARGB.black(alpha * backgroundOpacity));
/// });
/// ```
///
/// Beside [`chat_lines`] and reading the same `visible_lines`, so a row's fill
/// and its text cannot disagree about which rows exist or where they are.
///
/// Three things that are not the obvious reading:
///
/// * **The rect is asymmetric about the text.** The pose is translated by
///   `MESSAGE_INDENT` (4) before the fill, so `-4` lands at absolute 0 — four
///   pixels of padding left of the text — while `maxWidth + 4 + 4` lands at
///   `maxWidth + 12`, eight past where a full-width line can reach. Centring it
///   is the tidy reading and is not vanilla's.
/// * **`maxWidth` is `Mth.ceil(getWidth() / scale)`, a CEIL**, where
///   `addMessageToDisplayQueue`'s wrap budget is `Mth.floor` of the same
///   expression. They differ by one whenever the division is not exact, and the
///   difference is deliberate in the sense that vanilla wrote both: the box is
///   never narrower than the text it was wrapped to.
/// * **The alpha is `alpha * backgroundOpacity`, NOT `alpha * textOpacity`.**
///   The text's multiplier has a 0.1 floor (`chatOpacity * 0.9 + 0.1`) and the
///   background's has none, so at "Text Background: 0" the fill vanishes
///   entirely while the text stays faintly visible.
fn hud_fills(
    chat: &rewo_world::chat::ChatComponent,
    gui_tick: i32,
    px: f32,
    screen_h: f32,
    opts: &rewo_world::chat::ChatOptions,
    focused: bool,
) -> Vec<rewo_gpu::hud::HudFill> {
    let chat_px = px * opts.scale as f32;
    let chat_bottom = ((screen_h / chat_px) - rewo_world::chat::BOTTOM_MARGIN as f32).floor();
    let entry_height = opts.entry_height() as f32;
    // `Mth.ceil(this.getWidth() / scale)`.
    let max_width = (opts.width() as f32 / opts.scale as f32).ceil();
    let bg = opts.text_background_opacity as f32;
    chat.visible_lines(gui_tick, focused, opts)
        .into_iter()
        .map(|line| {
            let entry_bottom = chat_bottom - line.index as f32 * entry_height;
            let entry_top = entry_bottom - entry_height;
            rewo_gpu::hud::HudFill {
                // `fill(-4, …)` under a pose translated by +4.
                x: (rewo_world::chat::MESSAGE_INDENT as f32 - 4.0) * chat_px,
                y: entry_top * chat_px,
                // right - left = (maxWidth + 4 + 4) - (-4).
                w: (max_width + 12.0) * chat_px,
                h: entry_height * chat_px,
                alpha: line.alpha * bg,
                // `ARGB.black(a)` sets only the alpha byte.
                rgb: [0.0; 3],
            }
        })
        .collect()
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
        // `rarity_color` yields vanilla's byte `/255`, and three of its five
        // callers feed the tooltip and Velvet passes rather than this one — so
        // the conversion belongs here, at the line, not inside the helper.
        color_linear: srgb_bytes_to_linear_f(color),
        alpha: alpha as f32 / 255.0,
        shadow: true,
        style: rewo_gpu::text::TextStyle::PLAIN,
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
            color_linear: srgb_bytes_to_linear(color & 0x00FF_FFFF),
            alpha: 1.0,
            shadow: false,
            style: rewo_gpu::text::TextStyle::PLAIN,
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

/// A parsed span's five renderable `Style` flags, for the bitmap text pass.
///
/// `Font.PreparedTextBuilder.accept` reads all five off the `Style` the
/// `FormattedCharSequence` carries per character — there is no surface at
/// which vanilla honours a component's colour and drops its bold. Rewo had
/// exactly that asymmetry on the title and the death screen until M130,
/// because both were written before `TextPass` could draw a flag (M126c) and
/// neither was revisited when it could.
pub(crate) fn text_style_of(span: &rewo_world::chat_style::ChatSpan) -> rewo_gpu::text::TextStyle {
    rewo_gpu::text::TextStyle {
        bold: span.bold,
        italic: span.italic,
        underlined: span.underlined,
        strikethrough: span.strikethrough,
        obfuscated: span.obfuscated,
    }
}

/// `Font.width(FormattedText)` over a parsed line — the per-span widths summed
/// with each span's OWN bold flag.
///
/// **Not `width(plain_text(line))`.** `getBoldOffset()` is 1.0 charged per
/// character (M126b), so flattening a line to measure it undercounts a bold
/// run by its length — and every consumer here divides the result by two to
/// centre, so the error lands as a visible half-width offset rather than as a
/// pixel. Vanilla's `font.width(this.title)` is style-aware for the same
/// reason: its splitter's width provider takes the style.
pub(crate) fn styled_line_width(
    line: &rewo_world::chat_style::ChatLine,
    advance: &[u8; 256],
) -> i32 {
    line.iter()
        .map(|s| rewo_gpu::text::width_styled(&s.text, advance, s.bold))
        .sum()
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
    lang: Option<&rewo_data::lang::Language>,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_net::chat_style::{self, ChatStyle};
    let (gw, gh) = ((screen_w / px) as i32, (screen_h / px) as i32);
    let mut out = Vec::new();
    // A run of spans laid end to end from a top-left in GUI pixels, at a
    // whole-number scale. `scale` multiplies the *font* pixel, which is why
    // the title is 4× and the subtitle 2× rather than being pre-scaled
    // strings.
    let run = |out: &mut Vec<rewo_gpu::world::OwnedTextLine>,
               line: &chat_style::ChatLine,
               x: i32,
               y: i32,
               scale: i32,
               alpha: f32| {
        let mut pen = x;
        for span in line {
            let w = rewo_gpu::text::width_styled(&span.text, advance, span.bold);
            if !span.text.is_empty() {
                out.push(rewo_gpu::world::OwnedTextLine {
                    x: pen as f32 * px,
                    y: y as f32 * px,
                    px: px * scale as f32,
                    // The span's own colour, in the LINEAR space the pass
                    // writes into an sRGB attachment. M117's coloured
                    // command runs are the precedent.
                    color_linear: srgb_bytes_to_linear_f(span.color),
                    alpha,
                    shadow: true,
                    style: text_style_of(span),
                    text: span.text.clone(),
                });
            }
            pen += w * scale;
        }
    };

    // `if (this.title != null && this.titleTime > 0)`.
    if let Some(title) = t.title.as_ref().filter(|_| t.title_time > 0) {
        let alpha =
            rewo_gpu::hud::title_alpha(t.title_time, t.fade_in, t.stay, t.fade_out, partial);
        // `if (alpha > 0)` — the draw's own guard, so a fully-faded frame
        // emits nothing rather than a transparent quad.
        if alpha > 0 {
            let a = alpha as f32 / 255.0;
            let line = chat_style::parse_component(title, ChatStyle::WHITE, lang);
            let width = styled_line_width(&line, advance);
            let (x, y) = rewo_gpu::hud::title_pos(gw, gh, width);
            run(&mut out, &line, x, y, rewo_gpu::hud::TITLE_SCALE, a);
            // The subtitle is drawn *inside* the title's block, at the title's
            // alpha — it has no ramp of its own.
            if let Some(subtitle) = &t.subtitle {
                let line = chat_style::parse_component(subtitle, ChatStyle::WHITE, lang);
                let width = styled_line_width(&line, advance);
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
            let line = chat_style::parse_component(message, ChatStyle::WHITE, lang);
            let width = styled_line_width(&line, advance);
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
    // -- the ghost recipe (M103) ---------------------------------------------

    // -- the chat box (M108) -------------------------------------------------

    /// Six lines of chat and the geometry they land on. Shared by the
    /// witnesses below so each asserts one property against one fixture.
    fn chat_fixture(count: usize, added_time: i32) -> rewo_world::chat::ChatComponent {
        let w6 = |s: &str, st: rewo_world::chat_style::ChatStyle| {
            s.chars().count() as i32 * (6 + i32::from(st.bold))
        };
        let ctx = rewo_world::chat::WrapContext {
            options: rewo_world::chat::ChatOptions::default(),
            focused: false,
            width_of: &w6,
            deleted_marker_text: "deleted",
        };
        let mut c = rewo_world::chat::ChatComponent::new();
        for i in 0..count {
            c.add_message(
                rewo_world::chat::GuiMessage {
                    added_time,
                    content: vec![
                        rewo_world::chat_style::ChatStyle::WHITE.span(format!("m{i}")),
                    ],
                    signature: None,
                    source: rewo_world::chat::MessageSource::SystemServer,
                    tag: None,
                },
                &ctx,
            );
        }
        c
    }

    /// The bottom row sits `entryBottomToMessageY` above `chatBottom`, and
    /// `chatBottom` is `floor((screenHeight - 40) / scale)` — measured in
    /// **chat** pixels, then scaled back to screen pixels.
    ///
    /// At the defaults (GUI px 1, chat scale 1, 720 px tall) that is
    /// `720 - 40 = 680`, minus 8, so the bottom line's top edge is 672.
    #[test]
    fn the_bottom_chat_row_sits_forty_pixels_off_the_bottom_less_the_baseline() {
        let opts = rewo_world::chat::ChatOptions::default();
        let lines = super::chat_lines(&chat_fixture(1, 0), 0, 1.0, 720.0, &opts, None, false).0;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].y, 672.0);
        // `pose.translate(4, 0)` — not 0, and not the F3 block's 3.
        assert_eq!(lines[0].x, 4.0);
    }

    /// Rows are one `entryHeight` apart and stack **upward**, newest at the
    /// bottom. Both ends are asserted: the top row alone cannot see a
    /// reversed stack, and the count alone cannot see a wrong pitch.
    #[test]
    fn chat_rows_stack_upward_one_entry_height_apart() {
        let opts = rewo_world::chat::ChatOptions::default();
        let lines = super::chat_lines(&chat_fixture(3, 0), 0, 1.0, 720.0, &opts, None, false).0;
        assert_eq!(lines.len(), 3);
        // `visible_lines` emits top-first, so index 0 here is the oldest and
        // highest.
        assert_eq!((lines[0].text.as_str(), lines[0].y), ("m0", 672.0 - 18.0));
        assert_eq!((lines[1].text.as_str(), lines[1].y), ("m1", 672.0 - 9.0));
        assert_eq!((lines[2].text.as_str(), lines[2].y), ("m2", 672.0));
    }

    /// The baseline offset is `entryBottomToMessageY`, whose two terms move in
    /// opposite directions with the line spacing.
    ///
    /// **This needs a non-default spacing to say anything.** At spacing 0 the
    /// row is 9 tall and the offset is 8, so `entryHeight - 1` — the obvious
    /// wrong reading — gives the same answer; every other witness here uses
    /// the defaults and a mutation to it survived them all. At spacing 1 the
    /// row is 18 and the offset is `round(8*2 - 4*1) = 12`, where
    /// `entryHeight - 1` would be 17.
    #[test]
    fn the_baseline_offset_is_not_the_row_height_less_one() {
        let mut opts = rewo_world::chat::ChatOptions::default();
        opts.line_spacing = 1.0;
        assert_eq!(opts.entry_height(), 18);
        let lines = super::chat_lines(&chat_fixture(2, 0), 0, 1.0, 720.0, &opts, None, false).0;
        // chatBottom 680, minus 12.
        assert_eq!(lines[1].y, 668.0);
        // …and the pitch is the row height, so the two readings cannot be
        // confused by the gap either.
        assert_eq!(lines[0].y, 668.0 - 18.0);
    }

    /// The backdrop's rect is asymmetric about the text: the pose is
    /// translated by 4 before `fill(-4, …, maxWidth + 4 + 4, …)`, so the left
    /// edge lands at absolute 0 and the right at `maxWidth + 12`.
    #[test]
    fn the_backdrop_is_asymmetric_about_the_text() {
        let opts = rewo_world::chat::ChatOptions::default();
        let b = super::hud_fills(&chat_fixture(1, 0), 0, 1.0, 720.0, &opts, false);
        assert_eq!(b.len(), 1);
        // Four pixels left of the text, which sits at 4.
        assert_eq!(b[0].x, 0.0);
        // …and eight past where a full-width line can reach, not four.
        assert_eq!(b[0].w, 320.0 + 12.0);
        // Centring it — the tidy reading — would put the left edge at -6.
        assert_ne!(b[0].x, -6.0);
    }

    /// A row's fill spans its whole `entryHeight` and sits directly under the
    /// row's text, so the two derivations agree about where a row is.
    #[test]
    fn each_fill_covers_its_own_row_and_meets_the_next() {
        let opts = rewo_world::chat::ChatOptions::default();
        let c = chat_fixture(3, 0);
        let b = super::hud_fills(&c, 0, 1.0, 720.0, &opts, false);
        let l = super::chat_lines(&c, 0, 1.0, 720.0, &opts, None, false).0;
        assert_eq!(b.len(), l.len());
        for i in 0..b.len() {
            assert_eq!(b[i].h, 9.0);
            // The text's top edge is inside its own fill.
            assert!(b[i].y <= l[i].y && l[i].y < b[i].y + b[i].h, "row {i}");
        }
        // Rows tile with no gap: each fill's bottom is the next one's top.
        for i in 1..b.len() {
            assert_eq!(b[i - 1].y + b[i - 1].h, b[i].y);
        }
    }

    /// The fill's alpha is `alpha * backgroundOpacity`, and the text's is
    /// `alpha * (chatOpacity * 0.9 + 0.1)` — **different multipliers**. At
    /// "Text Background: 0" the fill vanishes while the text stays visible.
    #[test]
    fn the_fill_and_the_text_use_different_opacity_multipliers() {
        let mut opts = rewo_world::chat::ChatOptions::default();
        let c = chat_fixture(1, 0);
        // Default: background 0.5, text 1.0.
        assert_eq!(super::hud_fills(&c, 0, 1.0, 720.0, &opts, false)[0].alpha, 0.5);
        assert_eq!(super::chat_lines(&c, 0, 1.0, 720.0, &opts, None, false).0[0].alpha, 1.0);
        opts.text_background_opacity = 0.0;
        assert_eq!(super::hud_fills(&c, 0, 1.0, 720.0, &opts, false)[0].alpha, 0.0);
        // The text is untouched by it — a shared multiplier would zero both.
        assert_eq!(super::chat_lines(&c, 0, 1.0, 720.0, &opts, None, false).0[0].alpha, 1.0);
    }

    /// The fade reaches the fill too, so a message dims its own backdrop with
    /// it rather than leaving a black bar behind.
    #[test]
    fn the_fade_reaches_the_backdrop() {
        let opts = rewo_world::chat::ChatOptions::default();
        let c = chat_fixture(1, 0);
        // 190 ticks: half through the fade, squared -> 0.25, times bg 0.5.
        let b = super::hud_fills(&c, 190, 1.0, 720.0, &opts, false);
        assert!((b[0].alpha - 0.125).abs() < 1e-5);
        // Fully faded rows are not emitted at all.
        assert!(super::hud_fills(&c, 200, 1.0, 720.0, &opts, false).is_empty());
    }

    /// `maxWidth` is a CEIL here and a FLOOR in the wrap budget. They agree at
    /// scale 1 and differ the moment the division is not exact.
    #[test]
    fn the_backdrops_max_width_is_a_ceil_not_the_wraps_floor() {
        let mut opts = rewo_world::chat::ChatOptions::default();
        opts.scale = 0.75;
        // 320 / 0.75 = 426.66… -> ceil 427, floor 426.
        let b = super::hud_fills(&chat_fixture(1, 0), 0, 1.0, 720.0, &opts, false);
        let chat_px = 0.75_f32;
        assert!(((b[0].w / chat_px) - (427.0 + 12.0)).abs() < 1e-3, "{}", b[0].w / chat_px);
    }

    /// The GUI scale multiplies every offset, including the 40-px margin —
    /// which is why `chatBottom` divides by the scale before the subtraction
    /// rather than after. At px 2 the box is 40 *chat* pixels off the bottom,
    /// i.e. 80 screen pixels, not 40.
    #[test]
    fn the_gui_scale_multiplies_the_whole_box() {
        let opts = rewo_world::chat::ChatOptions::default();
        let lines = super::chat_lines(&chat_fixture(1, 0), 0, 2.0, 720.0, &opts, None, false).0;
        assert_eq!(lines[0].px, 2.0);
        assert_eq!(lines[0].x, 8.0);
        // floor(720/2 - 40) = 320 chat px, minus 8, times 2.
        assert_eq!(lines[0].y, 624.0);
    }

    /// The fade reaches the line's alpha, and is multiplied by
    /// `chatOpacity * 0.9 + 0.1` — so a fully faded line is gone but a fresh
    /// one is not dimmed at the default opacity of 1.
    #[test]
    fn the_fade_and_the_text_opacity_both_reach_the_line() {
        let opts = rewo_world::chat::ChatOptions::default();
        let c = chat_fixture(1, 0);
        assert_eq!(super::chat_lines(&c, 0, 1.0, 720.0, &opts, None, false).0[0].alpha, 1.0);
        // 190 ticks: half way through the 20-tick fade, squared -> 0.25.
        let faded = super::chat_lines(&c, 190, 1.0, 720.0, &opts, None, false).0;
        assert!((faded[0].alpha - 0.25).abs() < 1e-5);
        // Past 200 the line is not emitted at all, rather than emitted at 0.
        assert!(super::chat_lines(&c, 200, 1.0, 720.0, &opts, None, false).0.is_empty());

        let mut dim = rewo_world::chat::ChatOptions::default();
        dim.opacity = 0.0;
        let lines = super::chat_lines(&c, 0, 1.0, 720.0, &dim, None, false).0;
        assert!((lines[0].alpha - 0.1).abs() < 1e-6, "the floor is 0.1, not 0");
    }

    /// The unfocused box holds ten rows (90 / 9), so an eleventh message
    /// pushes the oldest off the top rather than growing the box.
    #[test]
    fn the_unfocused_box_holds_ten_rows() {
        let opts = rewo_world::chat::ChatOptions::default();
        let lines = super::chat_lines(&chat_fixture(30, 0), 0, 1.0, 720.0, &opts, None, false).0;
        assert_eq!(lines.len(), 10);
        assert_eq!(lines.last().unwrap().text, "m29");
        assert_eq!(lines.first().unwrap().text, "m20");
    }

    /// The whole point of threading `focused` through (M110): with the chat
    /// screen open the box is TALLER and the fade is off.
    ///
    /// Hardcoding `false` — which is what these two derivations did until the
    /// screen existed — leaves the focused view showing ten rows of twenty and
    /// fading messages out from under someone who is reading them.
    #[test]
    fn the_focused_box_is_taller_and_does_not_fade() {
        let opts = rewo_world::chat::ChatOptions::default();
        let c = chat_fixture(30, 0);
        assert_eq!(super::chat_lines(&c, 0, 1.0, 720.0, &opts, None, false).0.len(), 10);
        assert_eq!(super::chat_lines(&c, 0, 1.0, 720.0, &opts, None, true).0.len(), 20);
        // …and the fills follow the text, so a taller box does not draw ten
        // rows of glyphs over twenty rows of backdrop.
        assert_eq!(super::hud_fills(&c, 0, 1.0, 720.0, &opts, false).len(), 10);
        assert_eq!(super::hud_fills(&c, 0, 1.0, 720.0, &opts, true).len(), 20);

        // At 300 ticks every message is long faded, and the focused view shows
        // them anyway — `AlphaCalculator.FULLY_VISIBLE` rather than
        // `timeBased`.
        assert!(super::chat_lines(&c, 300, 1.0, 720.0, &opts, None, false).0.is_empty());
        let focused = super::chat_lines(&c, 300, 1.0, 720.0, &opts, None, true).0;
        assert_eq!(focused.len(), 20);
        assert!(focused.iter().all(|l| l.alpha == 1.0));
    }

    /// **The bug M112 fixes**: `ScreenState::hovered` converted through
    /// `Placement::centred` while the render converts through
    /// `Placement::with_book`, so with the book open the two disagree by the
    /// 77 GUI px `updateScreenPosition` moves the panel.
    ///
    /// Written before the fix and left as the regression guard. M89 and M106b
    /// each recorded "a per-call-site choice is how they come to disagree";
    /// this is the third time, and it reached the CLICK rather than a tooltip.
    #[test]
    fn the_hover_and_the_render_agree_about_where_a_slot_is() {
        let layout = &rewo_world::menu_layout::PLAYER;
        // A window wide enough for the book to displace the panel: 1280 at GUI
        // scale 4 is 320 GUI px, which is NARROW, so pick one that is not.
        let (w, h) = (1920.0f32, 1080.0f32);
        let scale = rewo_gpu::hud::gui_scale(w, h);
        let gui_w = w / scale;
        assert!(
            gui_w >= rewo_world::recipe_book_screen::WIDTH_TOO_NARROW_BELOW as f32,
            "the fixture must be a WIDE window or the displacement is 0 and              this test cannot see anything ({gui_w} GUI px)"
        );
        // The cursor over the centre of the panel as the book-open render
        // places it.
        let (ox, _oy, _s) = rewo_gpu::container::gui_origin_placed(
            w,
            h,
            rewo_gpu::container::Placement::with_book(
                layout.image_w as f32,
                layout.image_h as f32,
                true,
            ),
        );
        // Slot 9's own position, converted forward to a screen point.
        let (sx, sy) = layout.position(9).unwrap();
        let mouse = (
            (ox + (sx as f32 + 8.0) * scale) as f64,
            {
                let (_, oy2, _) = rewo_gpu::container::gui_origin_placed(
                    w,
                    h,
                    rewo_gpu::container::Placement::with_book(
                        layout.image_w as f32,
                        layout.image_h as f32,
                        true,
                    ),
                );
                (oy2 + (sy as f32 + 8.0) * scale) as f64
            },
        );
        let mut screen = super::ScreenState::default();
        screen.mouse = mouse;
        assert_eq!(
            screen.hovered(layout, w, h, true),
            Some(9),
            "the hover must resolve the slot the render drew under the cursor"
        );
        // …and with the book SHUT the same screen point is a different slot,
        // which is what makes the parameter load-bearing rather than cosmetic.
        assert_ne!(screen.hovered(layout, w, h, false), Some(9));
    }

    /// A menu with no recipe book is never displaced and never suppressed.
    ///
    /// The witness that did not exist while this rule lived inside a
    /// `PlaySession`-taking function: a mutation answering `true` for a
    /// bookless menu survived the whole suite, and it would shift a chest's
    /// panel 77 px and blank its hover on a narrow window.
    #[test]
    fn a_menu_with_no_book_is_never_displaced() {
        use rewo_world::recipe_book_screen::BookType;
        let mut open = rewo_net::recipe_book::BookSettings::default();
        open.crafting.open = true;
        open.furnace.open = true;
        open.blast_furnace.open = true;
        open.smoker.open = true;
        assert!(!super::book_visible_for(None, &open), "a chest has no book");
        // …and every type that DOES have one reads its own flag, so this is
        // not "always false".
        for b in [
            BookType::Crafting,
            BookType::Furnace,
            BookType::BlastFurnace,
            BookType::Smoker,
        ] {
            assert!(super::book_visible_for(Some(b), &open), "{b:?}");
            assert!(
                !super::book_visible_for(
                    Some(b),
                    &rewo_net::recipe_book::BookSettings::default()
                ),
                "{b:?} shut"
            );
        }
    }

    /// Each book type reads **its own** flag, not another's — four settings
    /// that a single shared bool would collapse.
    #[test]
    fn each_book_type_reads_its_own_flag() {
        use rewo_world::recipe_book_screen::BookType;
        let mut only_furnace = rewo_net::recipe_book::BookSettings::default();
        only_furnace.furnace.open = true;
        assert!(super::book_visible_for(Some(BookType::Furnace), &only_furnace));
        assert!(!super::book_visible_for(Some(BookType::Crafting), &only_furnace));
        assert!(!super::book_visible_for(Some(BookType::Smoker), &only_furnace));
        assert!(!super::book_visible_for(
            Some(BookType::BlastFurnace),
            &only_furnace
        ));
    }

    /// `isHovering`'s narrow-window override: under 379 GUI px the book covers
    /// the menu, and vanilla answers "no slot" for every slot rather than
    /// letting a click reach through the panel on top of it.
    #[test]
    fn a_narrow_window_with_the_book_open_hovers_nothing() {
        let layout = &rewo_world::menu_layout::PLAYER;
        // 1280x720 at GUI scale 3 is 426 GUI px — wide. 1280x480 gives scale 2
        // and 640, also wide. A genuinely narrow one needs a small window.
        let (w, h) = (1024.0f32, 768.0f32);
        let scale = rewo_gpu::hud::gui_scale(w, h);
        let gui_w = (w / scale) as i32;
        assert!(
            rewo_world::recipe_book_screen::width_too_narrow(gui_w),
            "the fixture must be NARROW or this test cannot see the override              ({gui_w} GUI px against {})",
            rewo_world::recipe_book_screen::WIDTH_TOO_NARROW_BELOW
        );
        // Dead centre of the panel — unambiguously over a slot with the book
        // shut, which is the control that makes the `None` mean the override
        // and not a cursor that simply missed.
        let mouse = (w as f64 / 2.0, h as f64 / 2.0);
        assert!(
            super::hovered_menu_slot(layout, mouse, w, h, false).is_some(),
            "the control: with the book shut this point IS over a slot"
        );
        assert_eq!(
            super::hovered_menu_slot(layout, mouse, w, h, true),
            None,
            "and with it open on a narrow window, nothing is hovered"
        );
    }

    /// …and on a WIDE window the book displaces the panel instead of covering
    /// it, so slots stay hoverable. Without this partner the override could be
    /// "the book always suppresses the hover", which is a different rule.
    #[test]
    fn a_wide_window_with_the_book_open_still_hovers() {
        let layout = &rewo_world::menu_layout::PLAYER;
        let (w, h) = (1920.0f32, 1080.0f32);
        let scale = rewo_gpu::hud::gui_scale(w, h);
        assert!(!rewo_world::recipe_book_screen::width_too_narrow((w / scale) as i32));
        let (ox, oy, _) = rewo_gpu::container::gui_origin_placed(
            w,
            h,
            rewo_gpu::container::Placement::with_book(
                layout.image_w as f32,
                layout.image_h as f32,
                true,
            ),
        );
        let (sx, sy) = layout.position(9).unwrap();
        let mouse = (
            (ox + (sx as f32 + 8.0) * scale) as f64,
            (oy + (sy as f32 + 8.0) * scale) as f64,
        );
        assert_eq!(super::hovered_menu_slot(layout, mouse, w, h, true), Some(9));
    }

    /// The scrollbar reaches the frame's fill list, in screen pixels, and its
    /// colour is converted OUT of sRGB.
    ///
    /// The conversion is the half that hides: black is 0 in both spaces, so
    /// M109 and M110's fills could pass their bytes straight through and this
    /// is the first fill whose colour is not black.
    #[test]
    fn the_scrollbar_reaches_the_frame_in_linear_colour() {
        let opts = rewo_world::chat::ChatOptions::default();
        let c = chat_fixture(30, 0);
        let bars = super::chat_scrollbar(&c, 0, 1.0, 720.0, &opts);
        assert_eq!(bars.len(), 2, "a body and a highlight");
        // `scrollBarStartX = maxWidth + 4`, plus the pose's MESSAGE_INDENT.
        assert_eq!(bars[0].x, 328.0);
        assert_eq!((bars[0].w, bars[1].w), (2.0, 1.0));
        assert_eq!(bars[1].x, 329.0);
        // 96/255.
        assert!((bars[0].alpha - 96.0 / 255.0).abs() < 1e-6);
        // 0x3333AA's blue channel decoded: (0xAA/255 + 0.055)/1.055 ^ 2.4.
        let expect_b = ((0xAA as f32 / 255.0 + 0.055) / 1.055).powf(2.4);
        assert!((bars[0].rgb[2] - expect_b).abs() < 1e-5);
        // …and NOT the raw byte, which is what a pass-through would give.
        assert!((bars[0].rgb[2] - 0xAA as f32 / 255.0).abs() > 0.1);
        // The highlight is a light grey, brighter than the body in every
        // channel — a swap would be invisible to a single-channel check.
        for i in 0..3 {
            assert!(bars[1].rgb[i] > bars[0].rgb[i], "channel {i}");
        }
    }

    /// Nothing to scroll, nothing drawn — and the fills list is otherwise
    /// unchanged, so a short chat does not gain a stray rect.
    #[test]
    fn a_chat_that_fits_draws_no_scrollbar() {
        let opts = rewo_world::chat::ChatOptions::default();
        assert!(super::chat_scrollbar(&chat_fixture(5, 0), 0, 1.0, 720.0, &opts).is_empty());
        assert!(super::chat_scrollbar(
            &rewo_world::chat::ChatComponent::new(),
            0,
            1.0,
            720.0,
            &opts
        )
        .is_empty());
    }

    /// A suggestion environment with no completion words, so typing into a
    /// chat screen in these tests never opens a popup that would swallow the
    /// next key. The popup has its own tests in `rewo_world`.
    fn chat_env() -> rewo_world::chat_screen::SuggestionEnv<'static> {
        fn zero(_: &str) -> i32 {
            0
        }
        const NO_WORDS: &[String] = &[];
        rewo_world::chat_screen::SuggestionEnv {
            metrics: rewo_world::command_suggestions::InputMetrics {
                x: 4,
                inner_width: 316,
                screen_height: 240,
            },
            width: &zero,
            tab_words: NO_WORDS,
            auto_suggestions: true,
        }
    }

    /// A popup over `n` entries, opened through the production path.
    fn popup(n: usize) -> (rewo_world::chat_screen::ChatScreen, Vec<String>) {
        use rewo_world::chat_screen::{ChatMethod, ChatScreen};
        let words: Vec<String> = (0..n).map(|i| format!("rewo{i:02}")).collect();
        let mut s = ChatScreen::open(ChatMethod::Message, None, 0);
        fn six(t: &str) -> i32 {
            t.encode_utf16().count() as i32 * 6
        }
        let env = rewo_world::chat_screen::SuggestionEnv {
            metrics: rewo_world::command_suggestions::InputMetrics {
                x: 4,
                inner_width: 316,
                screen_height: 240,
            },
            width: &six,
            tab_words: &words,
            auto_suggestions: true,
        };
        s.char_typed('r', &env);
        (s, words)
    }

    #[test]
    fn the_popup_fills_one_rect_per_visible_row() {
        let (s, _) = popup(3);
        let cs = &s.suggestions;
        let list = cs.list().expect("the popup opened");
        let fills = super::suggestion_popup_fills(list, cs.config(), 1.0);
        // Three rows, no truncation bars.
        assert_eq!(fills.len(), 3);
        for (i, f) in fills.iter().enumerate() {
            assert_eq!(f.x, list.rect.x as f32);
            assert_eq!(f.y, (list.rect.y + 12 * i as i32) as f32);
            assert_eq!((f.w, f.h), (list.rect.w as f32, 12.0));
            // `-805306368` — alpha 208 over black.
            assert!((f.alpha - 208.0 / 255.0).abs() < 1e-6);
            assert_eq!(f.rgb, [0.0, 0.0, 0.0]);
        }
    }

    #[test]
    fn a_list_longer_than_its_window_gains_two_bars_and_one_row_of_dashes() {
        // `limited` is `hasPrevious || hasNext` and gates BOTH bars, so a list
        // scrolled to its top still gets one above it — while the dashes are
        // per-end and only the bottom has any here.
        let (s, _) = popup(25);
        let cs = &s.suggestions;
        let list = cs.list().expect("the popup opened");
        assert_eq!(list.offset(), 0, "fixture precondition: at the top");
        let fills = super::suggestion_popup_fills(list, cs.config(), 1.0);
        let rows = 10;
        let dashes = (list.rect.w as usize).div_ceil(2);
        assert_eq!(fills.len(), 2 + dashes + rows);
        // The two bars are one pixel tall and hug the rect.
        assert_eq!((fills[0].y, fills[0].h), ((list.rect.y - 1) as f32, 1.0));
        assert_eq!(
            (fills[1].y, fills[1].h),
            ((list.rect.y + list.rect.h) as f32, 1.0)
        );
        // Every dash is white, one pixel wide, two apart.
        for (k, f) in fills[2..2 + dashes].iter().enumerate() {
            assert_eq!(f.x, (list.rect.x + 2 * k as i32) as f32);
            assert_eq!((f.w, f.h), (1.0, 1.0));
            assert_eq!(f.rgb, [1.0, 1.0, 1.0]);
            assert_eq!(f.y, (list.rect.y + list.rect.h) as f32);
        }
    }

    #[test]
    fn a_short_list_gets_no_bars_at_all() {
        let (s, _) = popup(4);
        let cs = &s.suggestions;
        let list = cs.list().unwrap();
        let fills = super::suggestion_popup_fills(list, cs.config(), 1.0);
        assert!(fills.iter().all(|f| f.h == 12.0), "rows only");
    }

    #[test]
    fn the_selected_row_is_yellow_and_the_rest_are_grey() {
        let (s, _) = popup(3);
        let cs = &s.suggestions;
        let list = cs.list().unwrap();
        let text = super::suggestion_popup_text(list, cs.config(), 1.0);
        assert_eq!(text.len(), 3);
        for (i, t) in text.iter().enumerate() {
            assert_eq!(t.x, (list.rect.x + 1) as f32);
            assert_eq!(t.y, (list.rect.y + 2 + 12 * i as i32) as f32);
            assert!(t.shadow, "the five-argument graphics.text drops one");
        }
        // `-256` is 0xFFFF00: red and green at full, blue at nothing.
        assert_eq!(text[0].color_linear[0], 1.0);
        assert_eq!(text[0].color_linear[1], 1.0);
        assert_eq!(text[0].color_linear[2], 0.0);
        // `-5592406` is 0xAAAAAA: neutral, and not full.
        assert_eq!(text[1].color_linear[0], text[1].color_linear[1]);
        assert_eq!(text[1].color_linear[1], text[1].color_linear[2]);
        assert!(text[1].color_linear[0] > 0.0 && text[1].color_linear[0] < 1.0);
    }

    #[test]
    fn the_rows_follow_the_scroll_offset() {
        let (mut s, _) = popup(25);
        let first = super::suggestion_popup_text(
            s.suggestions.list().unwrap(),
            s.suggestions.config(),
            1.0,
        )[0]
        .text
        .clone();
        s.suggestions.mouse_scrolled(-1.0, {
            let r = s.suggestions.list().unwrap().rect;
            (r.x + 2, r.y + 2)
        });
        let scrolled = super::suggestion_popup_text(
            s.suggestions.list().unwrap(),
            s.suggestions.config(),
            1.0,
        )[0]
        .text
        .clone();
        assert_ne!(first, scrolled, "the top row moved with the offset");
    }

    #[test]
    fn the_coloured_runs_butt_up_against_each_other_at_measured_widths() {
        // The runs must abut exactly: a wrong width is a visible gap or an
        // overlap, where for the caret a fixed six pixels was only ever a
        // pixel or two of drift. The width function here is deliberately NOT
        // six-per-character, so a run laid out on the constant lands
        // somewhere else.
        use rewo_net::command_format::{Run, ARGUMENT_COLORS, GRAY};
        use rewo_world::chat_screen::{ChatMethod, ChatScreen};
        let mut s = ChatScreen::open(ChatMethod::Command, None, 0);
        s.input.set_value("/give 5");
        let runs = vec![
            Run { text: "/give ".into(), color: GRAY },
            Run { text: "5".into(), color: ARGUMENT_COLORS[0] },
        ];
        let width_of = |t: &str| t.chars().count() as i32 * 4;
        let lines = super::chat_input_lines(&s, 1.0, 320, 240, 0, Some(&runs), &width_of);
        assert_eq!(lines[0].text, "/give ");
        assert_eq!(lines[1].text, "5");
        assert_eq!(lines[1].x - lines[0].x, 6.0 * 4.0);
        // The two runs carry different colours, which is the whole point.
        assert_ne!(lines[0].color_linear, lines[1].color_linear);
        // And the caret sits after the WHOLE value, measured the same way.
        let caret = lines.iter().find(|l| l.text == "_").expect("a caret");
        assert_eq!(caret.x - lines[0].x, 7.0 * 4.0);
    }

    #[test]
    fn with_no_runs_the_field_is_one_flat_line() {
        // `formatChat` returns null before there is a parse, and an ordinary
        // chat message never gets one.
        use rewo_world::chat_screen::{ChatMethod, ChatScreen};
        let mut s = ChatScreen::open(ChatMethod::Message, None, 0);
        s.input.set_value("hello there");
        let lines = super::chat_input_lines(&s, 1.0, 320, 240, 0, None, &|t: &str| {
            t.chars().count() as i32 * 6
        });
        assert_eq!(lines[0].text, "hello there");
    }

    #[test]
    fn the_usage_box_grows_upward_from_the_bottom() {
        // `lineY = height - 27 - 12 * y`, so entry 0 is the LOWEST. Laying it
        // out downward from a top puts a two-line box over the field.
        let lines = vec!["<count>".to_string(), "<flag>".to_string()];
        let (fills, text) = super::usage_box(&lines, 40, 240, 1.0, 0xD000_0000, &|t| {
            t.chars().count() as i32 * 6
        });
        assert_eq!(fills.len(), 2);
        assert_eq!(fills[0].y, (240 - 27) as f32);
        assert_eq!(fills[1].y, (240 - 27 - 12) as f32);
        assert!(fills[1].y < fills[0].y, "later lines sit HIGHER");
        // The text sits two pixels down inside its fill.
        assert_eq!(text[0].y, fills[0].y + 2.0);
        assert_eq!(text[0].x, 40.0);
    }

    #[test]
    fn the_usage_fill_is_one_pixel_wider_than_its_text_on_each_side() {
        // `position - 1` to `position + width + 1`.
        let lines = vec!["<count>".to_string()];
        let (fills, text) = super::usage_box(&lines, 40, 240, 1.0, 0xD000_0000, &|t| {
            t.chars().count() as i32 * 6
        });
        assert_eq!(fills[0].x, 39.0);
        assert_eq!(fills[0].w, (7 * 6 + 2) as f32);
        assert_eq!(text[0].x - fills[0].x, 1.0);
    }

    #[test]
    fn the_usage_box_is_as_wide_as_its_widest_line() {
        let lines = vec!["<a>".to_string(), "<a much longer one>".to_string()];
        let (fills, _) = super::usage_box(&lines, 0, 240, 1.0, 0xD000_0000, &|t| {
            t.chars().count() as i32 * 6
        });
        let widest = "<a much longer one>".chars().count() as i32 * 6 + 2;
        assert!(fills.iter().all(|f| f.w == widest as f32));
    }

    /// The input field's caret sits after the text before the cursor, and the
    /// bar behind it is the screen's own rather than a chat row's.
    #[test]
    fn the_input_line_carries_its_text_and_a_caret() {
        use rewo_world::chat_screen::{ChatMethod, ChatScreen};
        let mut s = ChatScreen::open(ChatMethod::Message, None, 0);
        s.char_typed('h', &chat_env());
        s.char_typed('i', &chat_env());
        // `focused_time_ms` is unset until `set_focused_at`, so the blink's
        // clock starts at 0 and the caret is visible at t = 0.
        let lines = super::chat_input_lines(&s, 1.0, 320, 240, 0, None, &|t: &str| t.chars().count() as i32 * 6);
        assert_eq!(lines[0].text, "hi");
        // `EditBox(font, 4, height - 12, …)` with `(height - 8) / 2` centring.
        assert_eq!((lines[0].x, lines[0].y), (4.0, 230.0));
        assert_eq!(lines[1].text, "_");
        // Two characters at 6 px each.
        assert_eq!(lines[1].x, 4.0 + 12.0);

        let bar = super::chat_input_backdrop(1.0, 320, 240);
        assert_eq!((bar.x, bar.y, bar.w, bar.h), (2.0, 226.0, 316.0, 12.0));
        // A fixed 128/255 — NOT the text-background slider the rows read.
        assert!((bar.alpha - 128.0 / 255.0).abs() < 1e-6);
    }

    /// An untouched restored draft renders grey; an edited one does not.
    #[test]
    fn a_restored_draft_renders_grey_until_it_is_edited() {
        use rewo_world::chat_screen::{ChatMethod, ChatScreen, Draft};
        let draft = Draft::of("remembered");
        let mut s = ChatScreen::open(ChatMethod::Message, Some(&draft), 0);
        let grey = super::chat_input_lines(&s, 1.0, 320, 240, 0, None, &|t: &str| {
            t.chars().count() as i32 * 6
        })[0]
            .color_linear;
        // `ChatFormatting.GRAY` is `0xAAAAAA`, and the pass wants LINEAR.
        // 0.4020 is `((170/255 + 0.055) / 1.055)^2.4` — written out rather
        // than called, so this pins the number against the sRGB transfer
        // function itself and not against `srgb_bytes_to_linear`, which the
        // renderer also calls (M93q's self-calibration trap).
        assert!(
            grey.iter().all(|c| (c - 0.401_977_8).abs() < 1e-5),
            "ChatFormatting.GRAY in linear, got {grey:?}"
        );
        // The pre-M130 value, and the exact signature of the bug: handing the
        // byte over `/255` stores 213 where vanilla stores 170.
        assert!(
            (grey[0] - 2.0 / 3.0).abs() > 0.1,
            "the /255 byte is what M130 removed"
        );
        s.char_typed('!', &chat_env());
        let normal = super::chat_input_lines(&s, 1.0, 320, 240, 0, None, &|t: &str| {
            t.chars().count() as i32 * 6
        })[0]
            .color_linear;
        assert_ne!(normal, grey);
    }

    /// A restored draft is grey **and italic**, and its caret is neither.
    ///
    /// `ChatScreen.formatChat` returns
    /// `Style.EMPTY.withColor(GRAY).withItalic(true)`, and the draft field on
    /// `rewo_world::chat_screen` has said "drives the grey **italic**
    /// rendering" in its own doc since M110 while the renderer drew
    /// `TextStyle::PLAIN` — the fifth comment in this project to describe
    /// behaviour its code did not have.
    ///
    /// The caret half is the part a plausible implementation gets wrong.
    /// `graphics.text(font, applyFormat(half, …), …, color, …)` lets the
    /// formatter's `Style` override `color` per character, but
    /// `TextCursorUtils.extractAppendCursor` draws a bare `"_"` **String** with
    /// `color` itself — so the draft's grey and italic reach the text and stop
    /// at the caret.
    #[test]
    fn a_draft_is_italic_and_its_caret_is_not() {
        use rewo_world::chat_screen::{ChatMethod, ChatScreen, Draft};
        let draft = Draft::of("remembered");
        // t=0 is inside `isCursorVisible`'s first 300 ms window, so the caret
        // is emitted; without it this test would silently assert over one line.
        let lines = super::chat_input_lines(
            &ChatScreen::open(ChatMethod::Message, Some(&draft), 0),
            1.0,
            320,
            240,
            0,
            None,
            &|t: &str| t.chars().count() as i32 * 6,
        );
        let text = lines
            .iter()
            .find(|l| l.text == "remembered")
            .expect("the draft text");
        let caret = lines.iter().find(|l| l.text == "_").expect("the caret");
        assert!(text.style.italic, "withItalic(true)");
        assert_eq!(
            text.style,
            rewo_gpu::text::TextStyle {
                italic: true,
                ..rewo_gpu::text::TextStyle::PLAIN
            },
            "italic and ONLY italic — the style is a patch, not a preset"
        );
        assert_eq!(
            caret.style,
            rewo_gpu::text::TextStyle::PLAIN,
            "a bare String never meets the formatter"
        );
        assert_ne!(
            caret.color_linear, text.color_linear,
            "and it keeps the field's textColor rather than the draft's grey"
        );
        assert_eq!(caret.color_linear, srgb_bytes_to_linear(EDIT_BOX_TEXT_COLOR));
    }

    /// An empty chat produces no lines at all — not one blank row.
    #[test]
    fn an_empty_chat_draws_nothing() {
        let opts = rewo_world::chat::ChatOptions::default();
        assert!(super::chat_lines(
            &rewo_world::chat::ChatComponent::new(),
            0,
            1.0,
            720.0,
            &opts,
            None,
            false,
        )
        .0
        .is_empty());
    }


    /// The washes are two lists, and the veil is NOT widened for a big result
    /// slot — only the wash beneath it is.
    #[test]
    fn the_ghosts_two_washes_go_in_different_halves_and_only_one_widens() {
        use rewo_world::ghost_slots as gs;
        let layout = &rewo_world::menu_layout::PLAYER;
        let g = [gs::Ghost { slot: 0, items: vec![1], is_result: true }];
        // A crafting table screen: the result's wash is big.
        let (under, over) = super::ghost_washes(&g, layout, false);
        assert_eq!(under.len(), 1);
        assert_eq!(over.len(), 1, "one of each, per ghost");
        assert_eq!((under[0].1.w, under[0].1.h), (24.0, 24.0), "the big wash");
        assert_eq!((over[0].1.w, over[0].1.h), (16.0, 16.0), "the veil stays 16");
        // …and the big one starts 4 px up and left of the veil.
        assert_eq!(under[0].1.dx, over[0].1.dx - 4.0);
        assert_eq!(under[0].1.dy, over[0].1.dy - 4.0);

        // The player's own inventory: not big, so the two coincide.
        let (u2, o2) = super::ghost_washes(&g, layout, true);
        assert_eq!((u2[0].1.w, u2[0].1.h), (16.0, 16.0));
        assert_eq!((u2[0].1.dx, u2[0].1.dy), (o2[0].1.dx, o2[0].1.dy));
    }

    /// The two washes are DIFFERENT colours — red under, white over.
    #[test]
    fn the_wash_under_is_red_and_the_veil_is_white() {
        use rewo_world::ghost_slots as gs;
        let g = [gs::Ghost { slot: 1, items: vec![1], is_result: false }];
        let (under, over) = super::ghost_washes(&g, &rewo_world::menu_layout::PLAYER, false);
        // alpha 48/255 on both.
        assert!((under[0].1.tint[3] - 48.0 / 255.0).abs() < 1e-6);
        assert!((over[0].1.tint[3] - 48.0 / 255.0).abs() < 1e-6);
        // Red: full red, no green or blue.
        assert_eq!(under[0].1.tint[0], 1.0);
        assert_eq!((under[0].1.tint[1], under[0].1.tint[2]), (0.0, 0.0));
        // White: all three.
        assert_eq!(
            (over[0].1.tint[0], over[0].1.tint[1], over[0].1.tint[2]),
            (1.0, 1.0, 1.0)
        );
        assert_ne!(under[0].1.tint, over[0].1.tint);
    }

    /// Both washes are FILL quads, so they carry no sprite — the container pass
    /// reads a negative `u` as untextured.
    #[test]
    fn the_washes_are_untextured_fills() {
        use rewo_world::ghost_slots as gs;
        let g = [gs::Ghost { slot: 1, items: vec![1], is_result: false }];
        let (under, over) = super::ghost_washes(&g, &rewo_world::menu_layout::PLAYER, false);
        assert_eq!(under[0].0, rewo_gpu::container::FILL_SPRITE);
        assert_eq!(over[0].0, rewo_gpu::container::FILL_SPRITE);
    }

    /// A ghost on a slot the layout does not have is dropped rather than drawn
    /// at the origin.
    #[test]
    fn a_ghost_on_a_slot_that_does_not_exist_is_dropped() {
        use rewo_world::ghost_slots as gs;
        let g = [gs::Ghost { slot: 999, items: vec![1], is_result: false }];
        let (under, over) = super::ghost_washes(&g, &rewo_world::menu_layout::PLAYER, false);
        assert!(under.is_empty() && over.is_empty());
    }

    /// Each ghost lands on its own slot's position, not all at one place.
    #[test]
    fn each_ghost_lands_on_its_own_slot() {
        use rewo_world::ghost_slots as gs;
        let g = [
            gs::Ghost { slot: 1, items: vec![1], is_result: false },
            gs::Ghost { slot: 2, items: vec![2], is_result: false },
        ];
        let (under, _) = super::ghost_washes(&g, &rewo_world::menu_layout::PLAYER, false);
        assert_eq!(under.len(), 2);
        assert_ne!((under[0].1.dx, under[0].1.dy), (under[1].1.dx, under[1].1.dy));
        // And they match the layout's own positions.
        for (n, slot) in [1usize, 2].into_iter().enumerate() {
            let (sx, sy) = rewo_world::menu_layout::PLAYER.position(slot).unwrap();
            assert_eq!((under[n].1.dx, under[n].1.dy), (sx as f32, sy as f32));
        }
    }

    // -- the two crafting fills (M102) ---------------------------------------

    /// A player menu holding one stack in a given slot.
    fn inv_with(slots: &[(usize, rewo_world::inventory::ItemSlot)]) -> rewo_world::inventory::Inventory {
        let mut inv = rewo_world::inventory::Inventory::default();
        for &(slot, st) in slots {
            inv.set_slot(0, slot as i32, Some(st));
        }
        inv
    }

    fn dirt() -> rewo_world::inventory::ItemSlot {
        rewo_world::inventory::ItemSlot::plain(1, 1)
    }

    /// Both fills run, and they are disjoint: a stack on the player's 2x2 grid
    /// counts EXACTLY once, through the craft-slot half.
    #[test]
    fn the_grid_is_counted_once_through_the_craft_slot_fill() {
        let max = |_: i32| 64;
        let ing = [Ingredient::of(&[1])];
        // Slot 1 is the player's crafting grid — outside PLAYER_ITEM_SLOTS.
        let mut c = super::crafting_contents(
            &inv_with(&[(1, dirt())]),
            None,
            rb96::BookType::Crafting,
            &max,
        );
        assert!(c.try_pick(&ing, 1), "one on the grid satisfies one ingredient");
        // …and only once: two ingredients need two items, so one is not enough.
        let mut c2 = super::crafting_contents(
            &inv_with(&[(1, dirt())]),
            None,
            rb96::BookType::Crafting,
            &max,
        );
        assert!(
            !c2.try_pick(&[Ingredient::of(&[1]), Ingredient::of(&[1])], 1),
            "double-counting the grid would make this pass"
        );
    }

    /// The craft RESULT (slot 0) counts for nothing — a recipe must not read as
    /// craftable off its own output.
    #[test]
    fn the_craft_result_counts_for_nothing() {
        let max = |_: i32| 64;
        let mut c = super::crafting_contents(
            &inv_with(&[(0, dirt())]),
            None,
            rb96::BookType::Crafting,
            &max,
        );
        assert!(!c.try_pick(&[Ingredient::of(&[1])], 1));
    }

    /// A container's craft slots are added on top of the player's items, which
    /// is what makes a part-finished craft readable — the M96 gap.
    #[test]
    fn an_open_menus_craft_slots_are_added_to_the_players_items() {
        let max = |_: i32| 64;
        let ing = [Ingredient::of(&[1]), Ingredient::of(&[1])];
        // One in the hotbar, one on the open table's grid.
        let player = inv_with(&[(36, dirt())]);
        let table = inv_with(&[(1, dirt())]);
        let mut both = super::crafting_contents(&player, Some(&table), rb96::BookType::Crafting, &max);
        assert!(both.try_pick(&ing, 1), "one from each half");
        // Without the table's half, one short.
        let mut alone = super::crafting_contents(&player, None, rb96::BookType::Crafting, &max);
        assert!(!alone.try_pick(&ing, 1));
    }

    /// A furnace contributes its whole container, RESULT INCLUDED, and
    /// ungated — where a crafting grid excludes its result and is gated.
    #[test]
    fn a_furnaces_result_slot_counts_where_a_crafting_tables_does_not() {
        let max = |_: i32| 64;
        let ing = [Ingredient::of(&[1])];
        // Slot 2 is the furnace's result.
        let furnace = inv_with(&[(2, dirt())]);
        let mut f = super::crafting_contents(
            &rewo_world::inventory::Inventory::default(),
            Some(&furnace),
            rb96::BookType::Furnace,
            &max,
        );
        assert!(f.try_pick(&ing, 1), "a furnace's output counts");
        // The same slot under a crafting table's range (1..10) is a grid cell,
        // so it counts there too — the discriminating slot is 0, which the
        // crafting range excludes and the furnace range includes.
        let mut zero = super::crafting_contents(
            &rewo_world::inventory::Inventory::default(),
            Some(&inv_with(&[(0, dirt())])),
            rb96::BookType::Furnace,
            &max,
        );
        assert!(zero.try_pick(&ing, 1), "furnace slot 0 is its ingredient slot");
    }

    /// The furnace half is UNGATED: a damaged stack in a furnace counts, and the
    /// same stack on a crafting grid does not.
    #[test]
    fn the_furnace_half_ignores_isUsableForCrafting_and_the_crafting_half_does_not() {
        let max = |_: i32| 64;
        let ing = [Ingredient::of(&[1])];
        let chipped = rewo_world::inventory::ItemSlot {
            damage: Some(3),
            max_damage: Some(59),
            ..dirt()
        };
        let mut f = super::crafting_contents(
            &rewo_world::inventory::Inventory::default(),
            Some(&inv_with(&[(1, chipped)])),
            rb96::BookType::Furnace,
            &max,
        );
        assert!(f.try_pick(&ing, 1), "a furnace takes a damaged stack");
        let mut t = super::crafting_contents(
            &rewo_world::inventory::Inventory::default(),
            Some(&inv_with(&[(1, chipped)])),
            rb96::BookType::Crafting,
            &max,
        );
        assert!(!t.try_pick(&ing, 1), "a crafting grid does not");
    }

    // -- the recipe book's derivation (M97) ---------------------------------
    //
    // M96 graded this at its two ends - the solver's own tests and the gate's
    // chrome witness - and not in between. These drive `book_render_from`,
    // which is the whole arithmetic: grouping, tab membership, paging, the
    // display cycle, and `hasCraftable`.

    use rewo_world::recipe_book_screen as rb96;
    use rewo_world::stacked_contents::{Ingredient, StackedContents};

    fn entry<'a>(
        id: i32,
        group: Option<i32>,
        category: &'a str,
        results: &[i32],
        ingredients: Option<&[&[i32]]>,
    ) -> super::BookEntry<'a> {
        super::BookEntry {
            id,
            group,
            category,
            results: results.to_vec(),
            ingredients: ingredients
                .map(|v| v.iter().map(|ids| Ingredient::of(ids)).collect()),
            // M99 — no searchable text, which every witness below relies on
            // being inert: `matches` skips the stage on an empty query.
            search: Default::default(),
            // M104 — a shapeless recipe whose ingredients are the crafting
            // requirements, so a collection built from these has a grid the
            // which-of-these overlay can draw.
            shape: rewo_world::recipe_overlay::Shape::Shapeless {
                ingredients: ingredients.map_or(0, |v| v.len()),
            },
            grid_items: ingredients
                .map(|v| v.iter().map(|ids| ids.to_vec()).collect())
                .unwrap_or_default(),
        }
    }

    /// [`entry`] with searchable text, for the M99 witnesses.
    fn searchable<'a>(id: i32, category: &'a str, name: &str, item: &str) -> super::BookEntry<'a> {
        let (ns, path) = item.split_once(':').unwrap();
        super::BookEntry {
            search: rewo_world::recipe_search::SearchEntry {
                names: vec![name.to_lowercase()],
                ids: vec![(ns.to_string(), path.to_string())],
            },
            ..entry(id, None, category, &[70], None)
        }
    }

    /// `book_render_from` with the two inputs no M97 witness varies — the
    /// book's own selection (default) and the cursor (absent, so no hover).
    /// M98 added both; funnelling through one helper keeps those witnesses
    /// about what they were written for.
    fn render(
        book: rb96::BookType,
        filtering: bool,
        entries: &[super::BookEntry<'_>],
        held: &mut StackedContents,
        cycle: i32,
    ) -> super::BookRender {
        super::book_render_from(
            book,
            filtering,
            entries,
            held,
            cycle,
            rb96::BookState::default(),
            None,
            "",
        )
    }

    fn held(items: &[(i32, i32)]) -> StackedContents {
        let mut c = StackedContents::new();
        for &(item, count) in items {
            c.account(item, count);
        }
        c
    }

    const EQUIP: &str = "minecraft:crafting_equipment";
    const MISC: &str = "minecraft:crafting_misc";

    /// The whole point of M96, end to end: a slot's craftable flag follows
    /// what the player is holding.
    #[test]
    fn a_slot_is_craftable_exactly_when_the_held_items_satisfy_it() {
        let e = [entry(1, None, EQUIP, &[7], Some(&[&[10], &[11]]))];
        let got = |items: &[(i32, i32)]| {
            render(rb96::BookType::Crafting, false, &e, &mut held(items), 0)
                .slots[0]
                .0
        };
        assert!(got(&[(10, 1), (11, 1)]), "both ingredients held");
        assert!(!got(&[(10, 1)]), "one missing");
        assert!(!got(&[]), "nothing held");
        // Holding MORE than enough is still craftable.
        assert!(got(&[(10, 64), (11, 64)]));
    }

    /// `canCraft` opens with `craftingRequirements.isEmpty() ? false`, so an
    /// entry that carried none is never craftable however much you hold — the
    /// state the solver alone cannot express, because it never sees the entry.
    #[test]
    fn an_entry_with_no_requirements_is_never_craftable() {
        let e = [entry(1, None, EQUIP, &[7], None)];
        let r = render(
            rb96::BookType::Crafting,
            false,
            &e,
            &mut held(&[(10, 64), (11, 64)]),
            0,
        );
        assert!(!r.slots[0].0);
        // ...while the same entry declaring an EMPTY list is craftable, since
        // there is nothing to satisfy. The two states are distinct.
        let e2 = [entry(1, None, EQUIP, &[7], Some(&[]))];
        let r2 =
            render(rb96::BookType::Crafting, false, &e2, &mut held(&[]), 0);
        assert!(r2.slots[0].0);
    }

    /// `hasCraftable()` is ANY of the collection's recipes, so a group with one
    /// affordable member lights up.
    #[test]
    fn a_collection_is_craftable_if_ANY_of_its_recipes_is() {
        // Two recipes in one group: the first needs an item we lack.
        let e = [
            entry(1, Some(5), EQUIP, &[7], Some(&[&[99]])),
            entry(2, Some(5), EQUIP, &[7], Some(&[&[10]])),
        ];
        let r =
            render(rb96::BookType::Crafting, false, &e, &mut held(&[(10, 1)]), 0);
        assert_eq!(r.slots.len(), 1, "one grouped collection");
        assert!(r.slots[0].0, "the second recipe is affordable");
        assert!(r.slots[0].1, "and it has several recipes");
    }

    /// The solver is asked once per recipe and RESTORES what it took, so two
    /// collections needing the same item do not starve each other. A solver
    /// that consumed would light the first and grey the second.
    #[test]
    fn asking_about_one_collection_does_not_spend_anothers_items() {
        let e = [
            entry(1, None, EQUIP, &[7], Some(&[&[10]])),
            entry(2, None, EQUIP, &[8], Some(&[&[10]])),
        ];
        let r =
            render(rb96::BookType::Crafting, false, &e, &mut held(&[(10, 1)]), 0);
        assert_eq!(r.slots.len(), 2);
        assert!(r.slots[0].0 && r.slots[1].0, "both, from one item");
    }

    /// The search tab shows every category the book has; a category from
    /// ANOTHER book does not appear at all.
    #[test]
    fn the_page_holds_this_books_categories_and_no_others() {
        let e = [
            entry(1, None, EQUIP, &[7], None),
            entry(2, None, MISC, &[8], None),
            entry(3, None, "minecraft:smoker_food", &[9], None),
            entry(4, None, "minecraft:stonecutter", &[9], None),
        ];
        let r = render(rb96::BookType::Crafting, false, &e, &mut held(&[]), 0);
        assert_eq!(r.slots.len(), 2, "equipment and misc, not smoker or stonecutter");
        let smoker =
            render(rb96::BookType::Smoker, false, &e, &mut held(&[]), 0);
        assert_eq!(smoker.slots.len(), 1);
    }

    /// The cycle reaches the rendered item, and it is the SHARED clock — every
    /// slot advances together.
    #[test]
    fn the_display_cycle_reaches_the_slot_items() {
        let e = [
            entry(1, Some(5), EQUIP, &[70], Some(&[&[10]])),
            entry(2, Some(5), EQUIP, &[71], Some(&[&[10]])),
        ];
        let at = |cycle: i32| {
            render(rb96::BookType::Crafting, false, &e, &mut held(&[]), cycle)
                .slot_items[0]
        };
        assert_eq!(at(0), Some(70));
        assert_eq!(at(1), Some(71));
        assert_eq!(at(2), Some(70), "and it wraps");
    }

    /// The shadow copy needs BOTH conditions — several recipes and one shared
    /// result. Two recipes yielding different items draw one icon.
    #[test]
    fn the_shadow_needs_several_recipes_AND_one_result() {
        let same = [
            entry(1, Some(5), EQUIP, &[70], None),
            entry(2, Some(5), EQUIP, &[70], None),
        ];
        let differ = [
            entry(1, Some(5), EQUIP, &[70], None),
            entry(2, Some(5), EQUIP, &[71], None),
        ];
        let one = [entry(1, None, EQUIP, &[70], None)];
        let shadowed = |e: &[super::BookEntry<'_>]| {
            render(rb96::BookType::Crafting, false, e, &mut held(&[]), 0)
                .slot_shadowed[0]
        };
        assert!(shadowed(&same));
        assert!(!shadowed(&differ), "several recipes, different results");
        assert!(!shadowed(&one), "one recipe");
    }

    /// The view's `shown` is what the page actually holds, and its
    /// `total_pages` counts every collection — so a 45-recipe book pages.
    #[test]
    fn a_long_book_pages_and_the_first_page_is_full() {
        let cats: Vec<String> = (0..45).map(|_| EQUIP.to_string()).collect();
        let e: Vec<_> = (0..45)
            .map(|i| entry(i, None, &cats[i as usize], &[70], None))
            .collect();
        let r = render(rb96::BookType::Crafting, false, &e, &mut held(&[]), 0);
        let v = r.view.unwrap();
        assert_eq!(v.total_pages, 3);
        assert_eq!(v.shown, rb96::ITEMS_PER_PAGE);
        assert_eq!(r.slots.len(), rb96::ITEMS_PER_PAGE);
    }

    /// The filter flag reaches the view, which is what picks the toggle's art.
    #[test]
    fn the_filter_flag_reaches_the_view() {
        let e = [entry(1, None, EQUIP, &[7], None)];
        for filtering in [false, true] {
            let r =
                render(rb96::BookType::Crafting, filtering, &e, &mut held(&[]), 0);
            assert_eq!(r.view.unwrap().filtering, filtering);
        }
    }

    /// The book's selection reaches the view (M98) — and is CLAMPED, because a
    /// tab index outlives the book it was chosen in.
    #[test]
    fn a_tab_index_too_big_for_this_book_clamps_rather_than_panicking() {
        let e = [entry(1, None, "minecraft:smoker_food", &[7], None)];
        // Tab 4 exists on a crafting book (five tabs) and not on a smoker (two).
        let st = rb96::BookState { selected_tab: 4, page: 0 };
        let r = super::book_render_from(
            rb96::BookType::Smoker,
            false,
            &e,
            &mut held(&[]),
            0,
            st,
            None,
            "",
        );
        assert_eq!(r.view.unwrap().selected_tab, 1, "clamped to the last tab");
    }

    /// A page index that outlived its list resets to the FRONT, not to the new
    /// last page — `clamp_page`'s rule, reached through the real derivation.
    ///
    /// The fixture needs more than one page left for the two readings to
    /// differ, which is M93z's lesson a second time: the first draft used five
    /// collections (one page), where reset-to-front and clamp-to-last are both
    /// 0, and a mutation replacing the reset with a clamp survived it.
    #[test]
    fn a_page_index_that_outlived_its_list_resets_to_the_FRONT() {
        let three_pages: Vec<_> = (0..45)
            .map(|i| entry(i, None, EQUIP, &[7], None))
            .collect();
        let page_of = |page: usize, e: &[super::BookEntry<'_>]| {
            let st = rb96::BookState { selected_tab: 0, page };
            super::book_render_from(
                rb96::BookType::Crafting,
                false,
                e,
                &mut held(&[]),
                0,
                st,
                None,
                "",
            )
                .view
                .unwrap()
                .page
        };
        assert_eq!(page_of(2, &three_pages), 2, "still in range");
        // Out of range by one: `totalPages <= currentPage`, so page 3 of three
        // pages resets — and to 0, where a clamp would give 2.
        assert_eq!(page_of(3, &three_pages), 0);
        assert_eq!(page_of(9, &three_pages), 0, "and far out of range too");
        // A one-page list, where the two readings coincide and so prove
        // nothing on their own.
        let one_page: Vec<_> = (0..5).map(|i| entry(i, None, EQUIP, &[7], None)).collect();
        assert_eq!(page_of(3, &one_page), 0);
    }

    /// The hover comes from the SAME `book_hit` a press uses, so what lights up
    /// is what a click would take.
    #[test]
    fn the_hover_lights_what_a_click_would_take() {
        use rewo_world::recipe_book_screen as rb;
        // 45 collections, so there is a forward arrow to hover.
        let e: Vec<_> = (0..45)
            .map(|i| entry(i, None, EQUIP, &[7], None))
            .collect();
        let at = |bx: i32, by: i32| {
            super::book_render_from(
                rb96::BookType::Crafting,
                false,
                &e,
                &mut held(&[]),
                0,
                rb96::BookState::default(),
                Some((bx, by)),
                "",
            )
            .hover
        };
        let f = at(rb::PAGE_FORWARD_X + 6, rb::PAGE_ARROW_Y + 8);
        assert!(f.page_forward && !f.page_backward && !f.filter);
        let fl = at(rb::FILTER_X + 5, rb::FILTER_Y + 5);
        assert!(fl.filter && !fl.page_forward);
        // On page 0 the BACK arrow is not drawn, so it cannot be hovered
        // either — one gate, both.
        let b = at(rb::PAGE_BACK_X + 6, rb::PAGE_ARROW_Y + 8);
        assert!(!b.page_backward);
        // Bare panel hovers nothing.
        let none = at(70, 29);
        assert!(!none.filter && !none.page_forward && !none.page_backward);
        // And no cursor at all hovers nothing.
        let absent = super::book_render_from(
            rb96::BookType::Crafting,
            false,
            &e,
            &mut held(&[]),
            0,
            rb96::BookState::default(),
            None,
                "",
            )
        .hover;
        assert!(!absent.filter && !absent.page_forward && !absent.page_backward);
    }

    /// A left-click places the recipe the CYCLE is on, not the collection's
    /// first — `getCurrentRecipe`, the same index `getDisplayStack` reads.
    #[test]
    fn a_click_places_the_recipe_the_cycle_is_showing() {
        let e = [
            entry(11, Some(5), EQUIP, &[70], Some(&[&[1]])),
            entry(22, Some(5), EQUIP, &[71], Some(&[&[1]])),
        ];
        let at = |cycle: i32| {
            render(rb96::BookType::Crafting, false, &e, &mut held(&[]), cycle).slot_recipes[0]
        };
        assert_eq!(at(0), Some(11));
        assert_eq!(at(1), Some(22), "the second recipe, matching the shown item");
        assert_eq!(at(2), Some(11), "and it wraps with the display");
    }

    // -- the search (M99) ----------------------------------------------------

    fn search_render(entries: &[super::BookEntry<'_>], query: &str) -> super::BookRender {
        super::book_render_from(
            rb96::BookType::Crafting,
            false,
            entries,
            &mut held(&[]),
            0,
            rb96::BookState::default(),
            None,
            query,
        )
    }

    /// The search narrows the page, and an EMPTY query narrows nothing —
    /// vanilla skips the stage rather than running it with an empty string.
    #[test]
    fn the_search_narrows_the_page_and_an_empty_query_does_not() {
        let e = [
            searchable(1, EQUIP, "Diamond Sword", "minecraft:diamond_sword"),
            searchable(2, EQUIP, "Golden Apple", "minecraft:golden_apple"),
            searchable(3, MISC, "Iron Ingot", "minecraft:iron_ingot"),
        ];
        assert_eq!(search_render(&e, "").slots.len(), 3, "no query, no filter");
        assert_eq!(search_render(&e, "sword").slots.len(), 1);
        assert_eq!(search_render(&e, "gold").slots.len(), 1);
        assert_eq!(search_render(&e, "o").slots.len(), 3, "a substring of all three");
        assert_eq!(search_render(&e, "zzz").slots.len(), 0);
    }

    /// A colon-less query searches names only — the id is not consulted.
    #[test]
    fn a_query_without_a_colon_does_not_reach_the_ids() {
        // The name says Plank and the id says oak.
        let e = [searchable(1, EQUIP, "Wooden Plank", "minecraft:oak_planks")];
        assert_eq!(search_render(&e, "plank").slots.len(), 1);
        assert_eq!(search_render(&e, "oak").slots.len(), 0, "the id is not searched");
        assert_eq!(search_render(&e, "minecraft:oak").slots.len(), 1, "with a colon it is");
    }

    /// The page's total re-counts against the FILTERED list, so a search that
    /// leaves one page removes the arrows.
    #[test]
    fn a_search_recounts_the_pages() {
        let mut e: Vec<_> = (0..45)
            .map(|i| searchable(i, EQUIP, "Cobblestone", "minecraft:cobblestone"))
            .collect();
        e.push(searchable(99, EQUIP, "Diamond Sword", "minecraft:diamond_sword"));
        assert_eq!(search_render(&e, "").view.unwrap().total_pages, 3);
        let one = search_render(&e, "sword");
        assert_eq!(one.view.unwrap().total_pages, 1);
        assert_eq!(one.slots.len(), 1);
    }

    /// A collection's searchable text is the UNION of its recipes', so a group
    /// survives if any member matches — `flatMap` over `getRecipes()`.
    #[test]
    fn a_grouped_collection_matches_on_any_of_its_recipes() {
        let mut a = searchable(1, EQUIP, "Diamond Sword", "minecraft:diamond_sword");
        let mut b = searchable(2, EQUIP, "Golden Apple", "minecraft:golden_apple");
        a.group = Some(5);
        b.group = Some(5);
        let e = [a, b];
        assert_eq!(search_render(&e, "").slots.len(), 1, "one grouped collection");
        assert_eq!(search_render(&e, "sword").slots.len(), 1);
        assert_eq!(search_render(&e, "apple").slots.len(), 1, "the OTHER member");
        assert_eq!(search_render(&e, "iron").slots.len(), 0);
    }

    // -- the which-of-these overlay's derivation (M104) ---------------------
    //
    // Between the model (which knows the arithmetic and no recipes) and the
    // pixel gate (which is handed a finished `Open`) sits the step that turns
    // a page cell into a placed popup. That step is what M92 found is untested
    // by construction whenever a gate supplies what production derives.

    fn book_view(furnace_family: bool, filtering: bool) -> rb96::BookView {
        rb96::BookView {
            tabs: 5,
            selected_tab: 0,
            page: 0,
            total_pages: 1,
            shown: 1,
            filtering,
            furnace_family,
        }
    }

    fn button(recipe: i32, craftable: bool) -> rewo_world::recipe_overlay::Button {
        rewo_world::recipe_overlay::Button { recipe, craftable, slots: Vec::new() }
    }

    /// A collection reaches the overlay craftable-first, and the flag travels
    /// with the recipe rather than being recomputed against a later inventory.
    #[test]
    fn opening_the_overlay_promotes_the_craftable_recipes() {
        let open = super::open_overlay(
            vec![button(7, false), button(8, true), button(9, false)],
            0,
            book_view(false, false),
        );
        assert_eq!(
            open.buttons.iter().map(|b| b.recipe).collect::<Vec<_>>(),
            vec![8, 7, 9],
            "craftable first, then the collection's own order"
        );
        assert_eq!(open.craftable_flags(), vec![true, false, false]);
    }

    /// Filtering drops the uncraftable half instead of greying it — so the
    /// overlay a filtered book opens is SHORTER, not differently coloured.
    #[test]
    fn a_filtered_book_opens_an_overlay_of_craftable_recipes_only() {
        let all = vec![button(7, false), button(8, true)];
        let unfiltered = super::open_overlay(all.clone(), 0, book_view(false, false));
        let filtered = super::open_overlay(all, 0, book_view(false, true));
        assert_eq!(unfiltered.buttons.len(), 2);
        assert_eq!(filtered.buttons.len(), 1);
        assert_eq!(filtered.buttons[0].recipe, 8);
    }

    /// The panel is placed from the clicked CELL, through the clamps — and the
    /// right-hand column is where the clamps show.
    #[test]
    fn the_overlay_is_placed_from_the_cell_that_opened_it() {
        let two = vec![button(1, true), button(2, true)];
        // Cell 0 (column 0) is well inside every bound, so the panel's origin
        // IS the cell's corner.
        let left = super::open_overlay(two.clone(), 0, book_view(false, false));
        assert_eq!(left.origin, rb96::grid_slot(0));
        // Cell 4 (column 4) overhangs, and the horizontal clamp moves it one
        // button width — one, not two, because that clamp truncates.
        let right = super::open_overlay(two, 4, book_view(false, false));
        assert_eq!(right.origin.0, rb96::grid_slot(4).0 - 25);
        assert_eq!(right.origin.1, rb96::grid_slot(4).1, "the row is untouched");
    }

    /// The button family follows the MENU, which is what `view` carries.
    #[test]
    fn the_overlays_family_comes_from_the_book_not_the_recipes() {
        let one = vec![button(1, true), button(2, true)];
        assert!(!super::open_overlay(one.clone(), 0, book_view(false, false)).furnace);
        assert!(super::open_overlay(one, 0, book_view(true, false)).furnace);
    }

    /// `slot_collections` carries per-recipe affordability, where `slots`
    /// carries only whether ANY of them is — and the two must not disagree.
    #[test]
    fn a_collections_recipes_are_graded_one_by_one_for_the_overlay() {
        // Two recipes in one group: the first needs an item held, the second
        // does not exist in the inventory at all.
        let mut a = entry(1, Some(9), EQUIP, &[7], Some(&[&[10]]));
        let mut b = entry(2, Some(9), EQUIP, &[7], Some(&[&[99]]));
        a.group = Some(9);
        b.group = Some(9);
        let got = render(rb96::BookType::Crafting, false, &[a, b], &mut held(&[(10, 1)]), 0);
        assert_eq!(got.slots[0].0, true, "the CELL is craftable — any of them");
        let per = &got.slot_collections[0];
        assert_eq!(
            per.iter().map(|x| (x.recipe, x.craftable)).collect::<Vec<_>>(),
            vec![(1, true), (2, false)],
            "and the overlay sees them one by one, in the collection's order"
        );
    }

    /// An ingredient that resolves to nothing contributes no grid position,
    /// and because neither placement arm counts, the rest do not shift.
    #[test]
    fn an_unresolvable_ingredient_drops_its_position_and_moves_no_other() {
        let full = entry(1, None, EQUIP, &[7], Some(&[&[10], &[11], &[12]]));
        let holey = entry(2, None, MISC, &[7], Some(&[&[10], &[], &[12]]));
        let got = render(rb96::BookType::Crafting, false, &[full, holey], &mut held(&[]), 0);
        let slots_of = |n: usize| got.slot_collections[n][0].slots.clone();
        let a = slots_of(0);
        let b = slots_of(1);
        assert_eq!(a.len(), 3);
        assert_eq!(b.len(), 2, "the empty one is skipped");
        // The survivors keep the positions they had, rather than closing up.
        assert_eq!(b[0].0, a[0].0);
        assert_eq!(b[1].0, a[2].0, "ingredient 2 did NOT slide into slot 1");
    }

    /// The field is built with the BOOK's maximum (50), not `EditBox`'s default
    /// (32) — a long search would otherwise be silently truncated, and the
    /// difference is invisible until someone types past 32.
    #[test]
    fn the_search_field_carries_the_books_own_maximum() {
        let st = super::ScreenState::default();
        let mut field = st.book_search;
        // `char_typed` is gated on `can_consume_input`, which needs focus — the
        // coupling `book_press` mirrors from `BookState::search_focused`.
        field.set_focused(true);
        for _ in 0..60 {
            field.char_typed('a');
        }
        assert_eq!(
            field.value().chars().count(),
            rewo_world::recipe_book_screen::SEARCH_MAX_LENGTH
        );
        assert_eq!(rewo_world::recipe_book_screen::SEARCH_MAX_LENGTH, 50);
        assert_ne!(rewo_world::edit_box::EditBox::default().max_length(), 50);
    }

    // -- the search field's render (M100) -------------------------------------

    fn field_of(text: &str, focused: bool) -> rewo_world::edit_box::EditBox {
        use rewo_world::recipe_book_screen as rb;
        let mut f = rewo_world::edit_box::EditBox::new(rb::SEARCH_MAX_LENGTH);
        f.set_focused(true);
        for ch in text.chars() {
            f.char_typed(ch);
        }
        f.set_focused(focused);
        f
    }

    /// A stub advance table: every glyph 6 wide, which is the vanilla default
    /// and enough to make the caret's x a checkable number.
    fn advances() -> [u8; 256] {
        [6u8; 256]
    }

    /// The field's text is inset FOUR px and centred vertically — the bordered
    /// case, which is the book's, and not `getY()`.
    #[test]
    fn the_fields_text_geometry_is_the_BORDERED_one() {
        use rewo_world::recipe_book_screen as rb;
        assert_eq!(rb::SEARCH_TEXT_X, rb::SEARCH_X + 4);
        assert_eq!(rb::SEARCH_TEXT_Y, rb::SEARCH_Y + 3, "(14 - 8) / 2");
        assert_eq!(rb::SEARCH_INNER_W, 73, "81 - 8, taken off BOTH ends");
        assert_ne!(rb::SEARCH_INNER_W, rb::SEARCH_W);
        assert_ne!(rb::SEARCH_TEXT_Y, rb::SEARCH_Y, "the unbordered case");
    }

    /// The background is TWO blits, and the second is inset one pixel on every
    /// side — which is what leaves the border showing.
    #[test]
    fn the_fields_background_is_two_blits_and_the_inner_one_is_inset() {
        use rewo_world::recipe_book_screen as rb;
        let (_, fills) = super::book_field_render(&field_of("", false), &advances(), 1280.0, 720.0, 0);
        assert!(fills.len() >= 2);
        let (outer, inner) = (fills[0].1, fills[1].1);
        assert_eq!((outer.dx, outer.dy), (rb::SEARCH_X as f32, rb::SEARCH_Y as f32));
        assert_eq!((outer.w, outer.h), (rb::SEARCH_W as f32, rb::SEARCH_H as f32));
        assert_eq!((inner.dx, inner.dy), (outer.dx + 1.0, outer.dy + 1.0));
        assert_eq!((inner.w, inner.h), (outer.w - 2.0, outer.h - 2.0));
        // Both sample a 1x1 source, which is only exact because every region of
        // the sprite is uniform — a 1-bit paletted image of two colours.
        assert_eq!((outer.sw, outer.sh), (1.0, 1.0));
        assert_eq!((inner.sw, inner.sh), (1.0, 1.0));
        // …from DIFFERENT texels: the border's and the interior's.
        assert_ne!((outer.sx, outer.sy), (inner.sx, inner.sy));
    }

    /// Focus swaps the background sprite, and this is the one use of
    /// `WidgetSprites::get` on the book that means what its names say.
    #[test]
    fn focus_swaps_the_fields_background_sprite() {
        let plain = super::book_field_render(&field_of("", false), &advances(), 1280.0, 720.0, 0).1;
        let lit = super::book_field_render(&field_of("", true), &advances(), 1280.0, 720.0, 0).1;
        assert_ne!(plain[0].0, lit[0].0, "a different sprite index");
        assert_eq!(lit[0].0, plain[0].0 + 1, "highlighted is the pair's second");
    }

    /// The hint goes when the field takes FOCUS, not when the first character
    /// arrives — `displayed.isEmpty() && !isFocused()`.
    #[test]
    fn the_hint_goes_on_focus_not_on_the_first_character() {
        use rewo_world::recipe_book_screen as rb;
        let hint_of = |text: &str, focused: bool| {
            super::book_field_render(&field_of(text, focused), &advances(), 1280.0, 720.0, 0)
                .0
                .into_iter()
                .find(|l| l.text == rb::SEARCH_HINT)
        };
        assert!(hint_of("", false).is_some(), "empty and unfocused");
        assert!(hint_of("", true).is_none(), "FOCUSED and still empty");
        assert!(hint_of("iron", false).is_none(), "unfocused but not empty");
        assert!(hint_of("iron", true).is_none());
        // Its colour is the hint style's grey, not the field's white.
        let h = hint_of("", false).unwrap();
        // `SEARCH_HINT_STYLE` is `ChatFormatting.GRAY` — `0xAAAAAA` — and the
        // text pass takes LINEAR, so the constant converts on the way in
        // rather than being handed over as the byte.
        assert!(
            h.color_linear
                .iter()
                .all(|c| (c - 0.401_977_8).abs() < 1e-5),
            "GRAY in linear, got {:?}",
            h.color_linear
        );
        assert_ne!(h.color_linear, rb::SEARCH_HINT_COLOR, "not the raw /255");
        assert_ne!(h.color_linear, [1.0, 1.0, 1.0]);
    }

    /// The typed text is drawn at the field's text origin, in white.
    #[test]
    fn the_typed_text_is_drawn_where_the_field_says() {
        use rewo_world::recipe_book_screen as rb;
        let (labels, _) =
            super::book_field_render(&field_of("iron", true), &advances(), 1280.0, 720.0, 0);
        let (bl, bt, scale) = rewo_gpu::container::recipe_book_origin(1280.0, 720.0);
        let text = labels.iter().find(|l| l.text == "iron").expect("the text");
        assert_eq!(text.x, bl + rb::SEARCH_TEXT_X as f32 * scale);
        assert_eq!(text.y, bt + rb::SEARCH_TEXT_Y as f32 * scale);
        assert_eq!(text.color_linear, [1.0, 1.0, 1.0], "setTextColor(-1)");
    }

    // -- the page counter (M105) --------------------------------------------

    /// A language map holding the real `en_us.json` entry, so these read what
    /// the client shows.
    fn page_lang() -> rewo_data::lang::Language {
        let mut m = std::collections::HashMap::new();
        m.insert(
            rewo_world::recipe_book_screen::PAGE_LABEL_KEY.to_string(),
            "%s/%s".to_string(),
        );
        rewo_data::lang::Language::from_map(m)
    }

    fn book_of(page: usize, total: usize) -> super::BookRender {
        use rewo_world::recipe_book_screen as rb;
        super::BookRender {
            view: Some(rb::BookView {
                tabs: rb::CRAFTING_TABS.len(),
                selected_tab: 0,
                page,
                total_pages: total,
                shown: 0,
                filtering: false,
                furnace_family: false,
            }),
            book: rb::BookType::Crafting,
            ..Default::default()
        }
    }

    fn counter(page: usize, total: usize) -> Option<rewo_gpu::world::OwnedTextLine> {
        let (labels, _) = super::book_labels(
            &book_of(page, total),
            &field_of("", false),
            &page_lang(),
            &advances(),
            1280.0,
            720.0,
            0,
        );
        // By text rather than by index: the field contributes a hint label on
        // this fixture, so "the last one" would name whichever happened to be
        // pushed last and would keep passing if the two swapped.
        labels.into_iter().find(|l| l.text.contains('/'))
    }

    /// The counter reaches the composed label list at all — the step
    /// `apply_screen` cannot be asked about, since it needs a `PlaySession`.
    #[test]
    fn the_page_counter_reaches_the_books_labels() {
        assert_eq!(counter(0, 3).map(|l| l.text), Some("1/3".to_string()));
        // …and the field's own text is still there beside it, so composing the
        // two did not replace one with the other.
        let (labels, _) = super::book_labels(
            &book_of(0, 3),
            &field_of("iron", true),
            &page_lang(),
            &advances(),
            1280.0,
            720.0,
            0,
        );
        assert!(labels.iter().any(|l| l.text == "iron"), "the search text");
        assert!(labels.iter().any(|l| l.text == "1/3"), "the counter");
    }

    /// `if (this.totalPages > 1)` — a single-page book draws no counter, and a
    /// shut book contributes none because it has no view.
    #[test]
    fn a_single_page_book_draws_no_counter() {
        assert!(counter(0, 1).is_none(), "one page");
        assert!(counter(0, 0).is_none(), "an empty book");
        assert!(counter(0, 2).is_some(), "two pages");
        let (labels, _) = super::book_labels(
            &super::BookRender::default(),
            &field_of("", false),
            &page_lang(),
            &advances(),
            1280.0,
            720.0,
            0,
        );
        assert!(
            !labels.iter().any(|l| l.text.contains('/')),
            "no view, no counter"
        );
    }

    /// The x is `73 - width / 2` in BOOK pixels, and the width is MEASURED.
    ///
    /// The two fixtures differ in label length by one character, so a build
    /// that centred on a constant — or that measured the wrong string — puts
    /// them at the same x. With the stub table's 6 px glyphs the difference is
    /// exactly 3 book pixels.
    #[test]
    fn the_counter_is_placed_by_its_measured_width() {
        use rewo_world::recipe_book_screen as rb;
        let (bl, bt, scale) = rewo_gpu::container::recipe_book_origin(1280.0, 720.0);
        let short = counter(0, 3).expect("1/3");
        let long = counter(0, 10).expect("1/10");
        assert_eq!(short.text, "1/3");
        assert_eq!(long.text, "1/10");
        assert_eq!(short.x, bl + rb::page_label_x(3 * 6) as f32 * scale);
        assert_eq!(long.x, bl + rb::page_label_x(4 * 6) as f32 * scale);
        assert_ne!(short.x, long.x, "a constant x would agree here");
        assert_eq!(short.x - long.x, 3.0 * scale, "half the extra glyph");
        // The row is the same for both — only the x tracks the width.
        assert_eq!(short.y, bt + rb::PAGE_LABEL_Y as f32 * scale);
        assert_eq!(long.y, short.y);
    }

    /// Colour `-1` is opaque white, and the five-argument `graphics.text`
    /// delegates with `dropShadow = true`.
    #[test]
    fn the_counter_is_white_and_shadowed() {
        let c = counter(0, 3).unwrap();
        assert_eq!(c.color_linear, [1.0, 1.0, 1.0]);
        assert_eq!(c.alpha, 1.0, "ARGB.alpha(-1) — and text() skips alpha 0");
        assert!(c.shadow, "the 5-arg overload passes true");
    }

    /// The FIRST tooltip of a frame wins, not the last (M106c).
    ///
    /// `setTooltipForNextFrameInternal`'s body is
    /// `if (this.deferredTooltip == null || replaceExisting)` — so the
    /// container's, set before the book's two, is the one that survives. The
    /// call order reads the other way, which is what makes this worth pinning.
    #[test]
    fn the_first_tooltip_of_a_frame_wins() {
        fn pick(
            m: Option<&'static str>,
            b: Option<&'static str>,
            g: Option<&'static str>,
        ) -> Option<&'static str> {
            super::frame_tooltip(&mut (), |_| m, |_| b, |_| g)
        }
        assert_eq!(pick(Some("menu"), Some("book"), Some("ghost")), Some("menu"));
        assert_eq!(pick(None, Some("book"), Some("ghost")), Some("book"));
        assert_eq!(pick(None, None, Some("ghost")), Some("ghost"));
        assert_eq!(pick(None, None, None), None);
        // The menu's beats the ghost's with no page tooltip in play, which is
        // the pair that is actually reachable together: a ghost sits ON a menu
        // slot, while a page cell and a menu slot can never both be hovered.
        assert_eq!(pick(Some("menu"), None, Some("ghost")), Some("menu"));
        // And the later producers are not even evaluated once one has spoken —
        // `deferredTooltip` is assigned, not compared.
        let mut ran = 0;
        assert_eq!(
            super::frame_tooltip(
                &mut ran,
                |_| Some("menu"),
                |n| {
                    *n += 1;
                    Some("book")
                },
                |n| {
                    *n += 1;
                    Some("ghost")
                },
            ),
            Some("menu")
        );
        assert_eq!(ran, 0, "neither later producer ran");
    }

    /// The hover highlight resolves through the placement the book MOVED
    /// (M106b).
    ///
    /// The cursor is put at the true centre of a slot in each case, so a
    /// conversion that ignored the book would miss it by 77 GUI px — four
    /// columns on an 18 px pitch, and often off the panel entirely.
    #[test]
    fn the_hover_highlight_follows_the_panel_the_book_pushed() {
        // The crafting table: one of the four menus that has a book.
        let craft = rewo_world::menu_layout::layout_of(12).unwrap();
        let (w, h) = (1280.0f32, 720.0f32);
        let at = |slot: usize, book_open: bool| {
            let (l, t, sc) = rewo_gpu::container::gui_origin_placed(
                w,
                h,
                rewo_gpu::container::Placement::with_book(
                    craft.image_w as f32,
                    craft.image_h as f32,
                    book_open,
                ),
            );
            let (sx, sy) = craft.position(slot).unwrap();
            super::hovered_slot_position(
                craft,
                (
                    (l + (sx as f32 + 8.0) * sc) as f64,
                    (t + (sy as f32 + 8.0) * sc) as f64,
                ),
                w,
                h,
                book_open,
            )
        };
        let want = craft.position(1).map(|(x, y)| (x as i32, y as i32));
        assert_eq!(at(1, false), want, "book shut");
        assert_eq!(at(1, true), want, "book OPEN — the panel moved with it");
        // And the shift is real at this size, so the two cases are not the
        // same test written twice.
        let left = |book_open: bool| {
            rewo_gpu::container::gui_origin_placed(
                w,
                h,
                rewo_gpu::container::Placement::with_book(
                    craft.image_w as f32,
                    craft.image_h as f32,
                    book_open,
                ),
            )
            .0
        };
        assert_ne!(left(true), left(false));
        // Off the panel entirely is `None`, not slot 0 — otherwise every miss
        // would light the top-left slot.
        assert_eq!(
            super::hovered_slot_position(craft, (0.0, 0.0), w, h, false),
            None
        );
    }

    /// `if (!lines.isEmpty())` in `setTooltipForNextFrameInternal` — an empty
    /// list sets NO tooltip, which is not the same as an empty box (M106).
    ///
    /// **No current caller can reach this**: `screen_tooltip`'s two producers
    /// both start with a name line, and so does `book_tooltip`. A mutation
    /// deleting the guard therefore survived every behavioural witness, and
    /// the choice was to drop the guard or to pin it. It is pinned, because it
    /// is vanilla's rule for a shared entry point rather than a property of
    /// today's two producers — the third one to arrive gets it for free. This
    /// test names its own unreachability so the next reader does not go looking
    /// for the path that exercises it.
    #[test]
    fn an_empty_line_list_sets_no_tooltip_rather_than_an_empty_box() {
        assert!(
            super::tooltip_layout(Vec::new(), &advances(), None, (100.0, 100.0), (1280.0, 720.0))
                .is_none()
        );
        // …and one line still does, so the guard is not simply "never".
        assert!(super::tooltip_layout(
            vec![vec![rewo_gpu::tooltip::Span::new("x".to_string(), [1.0; 3])]],
            &advances(),
            None,
            (100.0, 100.0),
            (1280.0, 720.0),
        )
        .is_some());
    }

    /// A missing translation renders the bare key — `getOrDefault` returns it
    /// and a template with no specifiers survives substitution unchanged.
    #[test]
    fn a_missing_translation_shows_the_key() {
        use rewo_world::recipe_book_screen as rb;
        let empty = rewo_data::lang::Language::from_map(Default::default());
        let (labels, _) = super::book_labels(
            &book_of(0, 3),
            &field_of("", false),
            &empty,
            &advances(),
            1280.0,
            720.0,
            0,
        );
        assert!(labels.iter().any(|l| l.text == rb::PAGE_LABEL_KEY));
    }

    /// The blink reaches the RENDER: the same focused field draws a caret in
    /// one 300 ms window and none in the next (M101).
    #[test]
    fn the_caret_blinks_in_the_rendered_output() {
        use rewo_world::recipe_book_screen as rb;
        let mut f = rewo_world::edit_box::EditBox::new(rb::SEARCH_MAX_LENGTH);
        f.set_focused_at(true, 1_000);
        for ch in "iron".chars() {
            f.char_typed(ch);
        }
        let drawn = |now: u64| {
            let (labels, fills) = super::book_field_render(&f, &advances(), 1280.0, 720.0, now);
            labels.iter().any(|l| l.text == "_")
                || fills.iter().skip(2).any(|(_, b)| b.w == 1.0 && b.h == 11.0)
        };
        assert!(drawn(1_000), "on, the instant it was focused");
        assert!(drawn(1_299));
        assert!(!drawn(1_300), "off for the next 300 ms");
        assert!(drawn(1_600), "and on again");
    }

    /// The anvil's field is pinned focused, so a click elsewhere cannot take
    /// its caret away — `setCanLoseFocus(false)`.
    #[test]
    fn the_anvils_field_is_pinned_focused() {
        let mut f = super::anvil_field_new();
        assert!(f.is_focused(), "focused from the moment it is built");
        f.set_focused(false);
        assert!(f.is_focused(), "and it cannot lose it");
        assert_eq!(f.max_length(), rewo_world::anvil::MAX_NAME_LENGTH);
    }

    /// A caret outside the visible run is NOT drawn — `cursorOnScreen`, the
    /// third of `showCursor`'s conditions, which M93t's renderer omitted.
    ///
    /// Reachable only without `follow_cursor`: with it, every input keeps the
    /// cursor inside the run, which is the point of M101's other half. So the
    /// fixture types without following — the state the field was permanently in
    /// before M101, and the state a programmatic cursor move still produces.
    #[test]
    fn a_caret_outside_the_visible_run_is_not_drawn() {
        use rewo_world::recipe_book_screen as rb;
        let mut f = rewo_world::edit_box::EditBox::new(rb::SEARCH_MAX_LENGTH);
        f.set_focused_at(true, 0);
        // 26 glyphs at 6 px is 156, well past the 73 px inner width.
        for ch in "abcdefghijklmnopqrstuvwxyz".chars() {
            f.char_typed(ch);
        }
        let caret = |field: &rewo_world::edit_box::EditBox| {
            let (labels, fills) = super::book_field_render(field, &advances(), 1280.0, 720.0, 0);
            labels.iter().any(|l| l.text == "_")
                || fills.iter().skip(2).any(|(_, b)| b.w == 1.0 && b.h == 11.0)
        };
        assert!(!caret(&f), "the cursor is past the visible run");
        // Follow it, and the caret comes back — the two halves of M101 meeting.
        let advance = advances();
        let width = move |u: &[u16]| rewo_gpu::text::width(&String::from_utf16_lossy(u), &advance);
        f.follow_cursor(rb::SEARCH_INNER_W, &width);
        assert!(caret(&f), "and following the cursor restores it");
    }

    /// A focused field draws a caret and an unfocused one does not.
    #[test]
    fn only_a_focused_field_draws_a_caret() {
        let caret = |focused: bool| {
            let (labels, fills) =
                super::book_field_render(&field_of("iron", focused), &advances(), 1280.0, 720.0, 0);
            // The append caret is the character "_"; the insert caret is a
            // 1-px fill. Either counts.
            labels.iter().any(|l| l.text == "_")
                || fills.iter().skip(2).any(|(_, b)| b.w == 1.0 && b.h == 11.0)
        };
        assert!(caret(true));
        assert!(!caret(false));
    }

    /// `rewo-gpu` restates the recipe book's geometry rather than importing it,
    /// because the renderer deliberately does not depend on `rewo-world` (M94).
    /// This is the crate that sees both, so this is where the copy is paid for.
    ///
    /// Without it a drift draws the book a pixel off, or against the wrong
    /// sheet, with nothing failing anywhere.
    #[test]
    fn the_renderers_copy_of_the_books_geometry_matches_the_model() {
        use rewo_world::recipe_book_screen as rb;
        let (w, h, off, narrow, sheet) =
            rewo_gpu::container::book_constants_for_cross_check();
        assert_eq!(w, rb::IMAGE_W as f32);
        assert_eq!(h, rb::IMAGE_H as f32);
        assert_eq!(off, rb::OFFSET_X as f32);
        assert_eq!(narrow, rb::WIDTH_TOO_NARROW_BELOW as f32);
        assert_eq!(
            rewo_data::assets::MENU_BACKGROUND_TEXTURES[sheet],
            "gui/recipe_book.png",
            "the renderer's sheet index must name the book's own sheet"
        );
        assert_eq!(sheet, rewo_data::assets::RECIPE_BOOK_SHEET);
    }

    /// The draw and the hit test resolve their origin through the same
    /// `Placement`, so an open book must move BOTH or neither.
    #[test]
    fn an_open_book_moves_the_hit_test_exactly_as_far_as_the_panel() {
        use rewo_gpu::container::{gui_origin_placed, screen_to_gui_placed, Placement};
        let (w, h) = (1920.0f32, 1080.0f32);
        let shut = Placement::with_book(176.0, 166.0, false);
        let open = Placement::with_book(176.0, 166.0, true);
        let (l0, t0, sc) = gui_origin_placed(w, h, shut);
        let (l1, t1, _) = gui_origin_placed(w, h, open);
        assert_ne!(l0, l1, "the panel moves");
        assert_eq!(t0, t1, "but only horizontally");

        let m = (900.0, 500.0);
        let g0 = screen_to_gui_placed(m, w, h, shut);
        let g1 = screen_to_gui_placed(m, w, h, open);
        // The cursor's GUI-space x shifts by exactly the panel's own shift, so
        // a slot under the cursor before the book opened is not under it after.
        let panel_shift = ((l1 - l0) / sc) as f64;
        assert!(((g0.0 - g1.0) - panel_shift).abs() < 1e-6);
        assert_eq!(g0.1, g1.1);
    }

    /// A window under 379 GUI px keeps the menu centred, and the book covers it.
    #[test]
    fn a_narrow_window_moves_neither() {
        use rewo_gpu::container::{gui_origin_placed, Placement};
        // 640x480 at the GUI scale this picks is under the threshold.
        let (w, h) = (640.0f32, 480.0f32);
        let scale = rewo_gpu::hud::gui_scale(w, h);
        assert!(w / scale < 379.0, "the fixture has to actually be narrow");
        assert_eq!(
            gui_origin_placed(w, h, Placement::with_book(176.0, 166.0, true)),
            gui_origin_placed(w, h, Placement::with_book(176.0, 166.0, false))
        );
    }

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
        assert!(container_panel(&rewo_world::menu_layout::PLAYER, None, EnchantPlayer::default(), None).is_none());
    }

    #[test]
    fn a_lectern_paints_no_panel_rather_than_someone_elses() {
        // LecternScreen is a BookViewScreen. Falling through to a default
        // would paint some other menu's sheet behind a book.
        assert!(container_panel(layout(17), None, EnchantPlayer::default(), None).is_none());
    }

    #[test]
    fn every_other_menu_resolves_to_a_sheet_in_the_atlas() {
        for id in 0..25 {
            let l = layout(id);
            if id == 17 {
                continue;
            }
            let p = container_panel(l, None, EnchantPlayer::default(), None).unwrap_or_else(|| panic!("{} has no panel", l.name));
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
        let p = container_panel(layout(2), None, EnchantPlayer::default(), None).unwrap();
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
        let p = container_panel(layout(19), None, EnchantPlayer::default(), None).unwrap();
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
        let cr = menu_slot_rects(&chest, w, h, false);
        let pr = menu_slot_rects(&player, w, h, false);
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
pub struct ScreenState {
    pub screens: rewo_world::screen::Screens,
    /// The recipe book's own selection (M98) — the tab, the page and whether
    /// the search field has focus. **Screen state, not menu state**: the
    /// server's `RecipeBookSettings` carries open and filtering and nothing
    /// else, so these three exist only here.
    pub book: rewo_world::recipe_book_screen::BookState,
    /// The book's search field (M99), beside `book` rather than inside it
    /// because an `EditBox` is not `Copy` and `BookState` is passed by value
    /// through the render path.
    ///
    /// **Constructed with the book's own maximum, not `EditBox::default()`'s.**
    /// `initVisuals` calls `setMaxLength(50)` while the default is 32, so
    /// deriving `Default` for this field would silently truncate a long search
    /// at 32 characters — and the difference is invisible until someone types
    /// past it. That is why `ScreenState`'s `Default` is written out below
    /// rather than derived.
    pub book_search: rewo_world::edit_box::EditBox,
    /// The open which-of-these overlay (M104), beside `book` for the same
    /// reason `book_search` is: it holds a `Vec` and `BookState` is `Copy`.
    ///
    /// **A snapshot taken when the right-click opened it**, not a view — see
    /// `recipe_overlay::Open`. Its lifetime is exactly "until the next click",
    /// because every click while it is up either selects in it or shuts it.
    pub book_overlay: Option<rewo_world::recipe_overlay::Open>,
    /// `RecipeBookComponent.lastPlacedRecipe` (M107) — which recipe the last
    /// click sent, so a repeat of an UNCRAFTABLE one can be suppressed.
    ///
    /// Screen state rather than session state, like `book` and `book_search`:
    /// nothing on the wire carries it, and vanilla keeps it on the component.
    pub place_guard: rewo_world::recipe_book_screen::PlaceGuard,
    /// Cursor position in screen pixels. Only tracked while a screen is
    /// open; the rest of the time the cursor is grabbed and its position is
    /// meaningless.
    pub mouse: (f64, f64),
    /// How many `container_close` packets had arrived last time the frame
    /// looked (M74). A watermark rather than a flag so a close that lands in
    /// the same frame as another cannot be swallowed, and so nothing has to
    /// reach into the session to clear state it does not own.
    pub close_requests_seen: u64,
    /// The beacon screen's own `primary` / `secondary` (M93m).
    ///
    /// **Vanilla's screen owns these, not the menu.** A click moves them
    /// locally and only `set_beacon` on confirm tells the server, so they
    /// cannot be re-read from the data slots each frame — that was M92's
    /// stated shortcut, and it is what this replaces.
    ///
    /// Seeded from the menu whenever *either* watermark moves: the container
    /// id, because a new menu is a new beacon, and `data_writes`, because
    /// `ContainerListener.dataChanged` re-reads both effects on ANY slot
    /// change — so a pyramid growing under you discards an unconfirmed pick,
    /// which is vanilla's behaviour and not a bug to design around.
    pub beacon: Option<BeaconLocal>,
    /// The stonecutter's screen-local scroll (M93s). Reset by `cut_local`
    /// whenever the container or its input slot changes, which is what
    /// `containerChanged` does.
    pub cut: Option<CutLocal>,
    /// The anvil's name field (M93t). Screen-local: no packet carries the text
    /// being typed, only the `rename_item` it produces.
    pub anvil: Option<AnvilLocal>,
    /// The merchant's screen-local scroll (M93u).
    pub merchant: Option<MerchantLocal>,
    /// Set when a beacon button asks for the screen to close (M93m). Drained
    /// by the frame loop rather than closing from inside the press, so the
    /// close goes through the one path that owns the screen.
    pub close_beacon: bool,
    /// The container id whose screen is currently up, if a container's (M89).
    ///
    /// A watermark on the *menu*, not a mirror of the screen's open flag: a
    /// server re-opening the same slot gets a fresh menu, and comparing
    /// against the screen state would miss it.
    pub container_shown: Option<i32>,
}

impl Default for ScreenState {
    fn default() -> Self {
        Self {
            // M99 — `initVisuals`' own `setMaxLength(50)`. Everything else is
            // its type's default; only this one field has a value the type
            // cannot know.
            book_search: rewo_world::edit_box::EditBox::new(
                rewo_world::recipe_book_screen::SEARCH_MAX_LENGTH,
            ),
            screens: Default::default(),
            book: Default::default(),
            book_overlay: Default::default(),
            place_guard: Default::default(),
            mouse: Default::default(),
            close_requests_seen: Default::default(),
            beacon: Default::default(),
            cut: Default::default(),
            anvil: Default::default(),
            merchant: Default::default(),
            close_beacon: Default::default(),
            container_shown: Default::default(),
        }
    }
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
        layout: &'static rewo_world::menu_layout::MenuLayout,
        w: f32,
        h: f32,
        book_open: bool,
    ) -> Option<usize> {
        // **Through `hovered_menu_slot`, not its own conversion.** This method
        // used `screen_to_gui_for`, which is `Placement::centred` — so with
        // the recipe book open it resolved the cursor against a panel 77 GUI
        // px from the one the render drew, and it feeds the CLICK, the
        // double-click detector and the item-hover highlight. M89 and M106b
        // each fixed a consumer of the same predicate and each recorded that a
        // per-call-site choice is how they come to disagree; this is the third
        // time, and the first to reach an input path rather than a tooltip.
        hovered_menu_slot(layout, self.mouse, w, h, book_open)
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
            cause_of_death: Some(chat_style::plain_text(&chat_style::parse_component(
                &kill.message,
                ChatStyle::WHITE,
                Some(lang),
            ))),
            hardcore,
            score,
        };
        let labels = model.labels(lang);
        let cause = Some(chat_style::parse_component(
            &kill.message,
            ChatStyle::WHITE,
            Some(lang),
        ));
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
    // `color` arrives as vanilla's byte `/255` (`screen::INACTIVE_LABEL` is
    // `0xA0A0A0`); the pass wants linear. One conversion, in the one closure
    // every branch below pushes through.
    let mut push = |text: &str, x: i32, y: i32, color: [f32; 3]| {
        if text.is_empty() {
            return;
        }
        out.push(rewo_gpu::world::OwnedTextLine {
            x: x as f32 * px,
            y: y as f32 * px,
            px,
            color_linear: srgb_bytes_to_linear_f(color),
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
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
    use rewo_net::chat_style::ChatSpan;
    use rewo_world::death_screen as ds;
    let gui_w = (screen_w / px) as i32;
    let mut out = Vec::new();

    // A run of spans laid end to end from a GUI-space top-left, at a
    // whole-number extra scale. `scale` multiplies the *font* pixel, which is
    // how the title comes out double-size without a second font.
    let run = |out: &mut Vec<rewo_gpu::world::OwnedTextLine>,
               spans: &[ChatSpan],
               x: i32,
               y: i32,
               scale: i32| {
        let mut pen = x;
        for span in spans {
            let w = rewo_gpu::text::width_styled(&span.text, advance, span.bold);
            if !span.text.is_empty() {
                out.push(rewo_gpu::world::OwnedTextLine {
                    x: pen as f32 * px,
                    y: y as f32 * px,
                    px: px * scale as f32,
                    // `ActiveTextCollector.accept` builds its `GuiTextRenderState`
                    // with `ARGB.white(opacity)` as the BASE and lets the
                    // component's own `Style` override it per character — so
                    // the colour is the span's, in linear, and the five flags
                    // are the span's too.
                    color_linear: srgb_bytes_to_linear_f(span.color),
                    alpha: 1.0,
                    shadow: true,
                    style: text_style_of(span),
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
        let w = styled_line_width(cause, advance);
        let (x, y) = ds::cause_pos(gui_w, w);
        run(&mut out, cause, x, y, 1);
    }

    // `deathScreen.score.value` — "Score: %s" with the value in YELLOW. Two
    // spans, not one: `Component.translatable(key, scoreValue)` nests a styled
    // literal inside an unstyled template, so the number is yellow and the
    // word is not.
    let score = score_spans(&view.labels.score_template, view.model.score);
    let w = styled_line_width(&score, advance);
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
    // M92 — the beacon's six effect ids, resolved once at startup.
    beacon_effects: BeaconEffectIds,
    // The beacon screen's own choice (M93m), or `None` to read the menu.
    beacon_override: Option<rewo_world::menu_screen::BeaconChoice>,
    // The stonecutter's grid (M93s). Resolved by the caller for the beacon's
    // reason: it needs the screen-local scroll, and `apply_screen` holds no
    // `ScreenState` — which is also what keeps it drivable from a gate.
    cut: Option<&CutView>,
    // M93t — the anvil's name field, resolved by the caller for the reason the
    // beacon's choice and the stonecutter's grid are: it is screen-local state
    // and `apply_screen` holds no `ScreenState`.
    anvil_field: Option<&rewo_world::edit_box::EditBox>,
    // M93u — the merchant's trade list, resolved by the caller for the same
    // reason: it needs the screen-local scroll.
    merchant: Option<&MerchantView>,
    // M98 — the recipe book's own tab and page, for the same reason as the
    // three above: `apply_screen` holds no `ScreenState`, which is what keeps
    // it drivable from a gate.
    book_state: rewo_world::recipe_book_screen::BookState,
    // M99 — and its search field's contents, already lowercased.
    book_query: &str,
    // M100 — and the field itself, for its text, caret and selection.
    book_field: &rewo_world::edit_box::EditBox,
    // M101 — wall-clock milliseconds, for both fields' caret blink.
    now_ms: u64,
    // M104 — and the open which-of-these overlay, supplied for the same reason
    // the four above are: it is screen-local state that `apply_screen` cannot
    // reach, and supplying it is what lets a gate drive the render path the
    // live client takes rather than a copy of it (M45/M93q).
    book_overlay: Option<&rewo_world::recipe_overlay::Open>,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
) {
    // M93t — the anvil's field is rendered ONCE, here, because its text and
    // its cursor share a width measurement: the cursor's x is the width of the
    // run before it. Splitting the two would measure the same string twice
    // with two chances to disagree.
    let (anvil_labels, anvil_fills) = match (baked.font.as_ref(), anvil_field) {
        (Some(f), Some(a)) => {
            let (l, fills, _) = anvil_field_render(a, &f.advance, w, h, now_ms);
            (l, fills)
        }
        _ => (Vec::new(), Vec::new()),
    };
    // M93q — the loom's grid, resolved here because it keys off the pattern
    // slot's item NAME and only this side holds the registry.
    let loom = session.menus.open().and_then(|m| {
        if m.layout.protocol_id != rewo_world::menu_screen::LOOM_MENU_PROTOCOL_ID {
            return None;
        }
        // Slots: banner 0, dye 1, pattern 2.
        let name = |slot: usize| m.menu.menu_slot(slot).and_then(|s| items.name(s.item_id));
        let patterns = rewo_data::loom_pattern_table::selectable_patterns(name(2));
        Some(LoomView {
            patterns,
            // The scrollbar's drag is not wired; see `LoomView::start_row`.
            start_row: 0,
            selected: m.loom_selected_pattern(),
            display: rewo_world::menu_screen::loom_display_patterns(
                m.menu.menu_slot(0).is_some(),
                m.menu.menu_slot(1).is_some(),
                // `hasMaxPatterns` needs the banner's own layer count, which
                // lives in a component Rewo does not read — so the grid stays
                // visible on a full banner where vanilla hides it.
                false,
                patterns.len(),
            ),
        })
    });
    // M94 — the recipe book, if the server says it is open for this menu's
    // book type. It is built here rather than inside `container_panel` because
    // its presence has to reach `screen_to_gui_placed` below as well: an open
    // book MOVES the menu, and a hover resolved against a centred panel while
    // the panel is drawn 77 px right would be wrong by more than four slots.
    // M98 — the cursor in the book's own space, computed ONCE here so the
    // hover and the press cannot read different numbers.
    let book_mouse = {
        let (bl, bt, scale) = rewo_gpu::container::recipe_book_origin(w, h);
        Some((
            ((mouse.0 - bl as f64) / scale as f64).floor() as i32,
            ((mouse.1 - bt as f64) / scale as f64).floor() as i32,
        ))
    };
    // M103 — the ghost recipe, from `place_ghost_recipe`, and its two washes.
    let ghosts = live_ghosts(session);
    let (ghost_under, ghost_over) = ghost_washes(
        &ghosts,
        session.shown_menu().layout(),
        session.menus.open().is_none(),
    );
    let book = live_recipe_book(
        session,
        items,
        book_state,
        book_mouse,
        book_query,
        &baked.item_names,
    );
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
    //
    // The cursor goes in through the SAME panel size the panel itself uses
    // (M87k's rule): the enchanting table's row highlight is measured from the
    // panel's top-left, so converting against the player's 176x166 would offset
    // it wherever the two disagree.
    // M94 — the book, beside the panel. Set unconditionally so a shut book
    // clears last frame's, and set even when `container_panel` returns `None`
    // (the player's own inventory), which is one of the four screens that has
    // one.
    // M100 — the search field's own quads and text. Built only when the book is
    // open, and only when there is a font to measure with.
    // M106b — ONE binding for "the book is open", read by the panel's origin,
    // the slot rects, the hover highlight, the tooltip and the enchanting
    // rows. Each of those used to spell `book_open` at its own call site,
    // which is how the tooltip and the highlight came to be the two that
    // never learnt about the shift (M89's rule: a per-call-site choice is how
    // they come to disagree).
    //
    // This is `apply_screen`'s composition root and no test reaches it — the
    // function needs a `PlaySession`. A mutation pinning this to `false`
    // therefore survives, and is recorded rather than papered over; what the
    // consolidation buys is that the five consumers can no longer disagree
    // with each other, which is the failure that actually happened.
    let book_open = book.is_some();
    // The ghost's items rotate on the SAME 30-tick clock the book's recipe
    // cells use (M95) — `slotSelectTime` is one object shared by the page, the
    // overlay and the ghost. Bound once so the icons and the ghost's tooltip
    // cannot be a frame apart on it.
    let ghost_cycle =
        (session.ticks as f32 / rewo_net::recipe_book::TICKS_TO_SWAP_SLOT).floor() as i32;
    // M105 — one call rather than two, so the composition of the field's text
    // with the page counter is inside a function a test can reach. This
    // function cannot be one: it needs a `PlaySession`.
    let (book_field_labels, book_field_fills) = match (book.as_ref(), baked.font.as_ref()) {
        (Some(b), Some(f)) => {
            book_labels(b, book_field, &baked.lang, &f.advance, w, h, now_ms)
        }
        _ => (Vec::new(), Vec::new()),
    };
    wr.set_recipe_book(book.as_ref().and_then(|b| {
        recipe_book_panel(b, &book_field_fills, book_overlay, book_mouse)
    }));
    wr.set_container_panel(container_panel(
        layout,
        session.menus.open(),
        EnchantPlayer {
            xp_level: session.hud.experience.level,
            creative: session.abilities.instabuild,
            beacon_effects,
            beacon_override,
            loom,
            cut,
            anvil_fills: &anvil_fills,
            merchant,
            ghost_under: &ghost_under,
            ghost_over: &ghost_over,
        },
        Some(rewo_gpu::container::screen_to_gui_placed(
            mouse,
            w,
            h,
            rewo_gpu::container::Placement::with_book(
                layout.image_w as f32,
                layout.image_h as f32,
                book_open,
            ),
        )),
    ));

    let (mut icons, mut labels) =
        screen_icons(
            menu,
            items,
            &session.trim_materials,
            w,
            h,
            session.menus.open(),
            cut,
            merchant,
            book.as_ref(),
            &ghosts,
            ghost_cycle,
            book_overlay,
        );
    if let Some((icon, label)) = carried_icon(menu, items, &session.trim_materials, mouse, w, h) {
        icons.push(icon);
        labels.extend(label);
    }
    // M92 — the enchanting table's three cost numerals, from the SAME row
    // derivation the overlays used.
    if let (Some(rows), Some(font)) = (
        enchant_rows_of(
            layout,
            session.menus.open(),
            EnchantPlayer {
                xp_level: session.hud.experience.level,
                creative: session.abilities.instabuild,
                beacon_effects,
                beacon_override,
                loom,
                cut,
                anvil_fills: &anvil_fills,
                merchant,
                ghost_under: &ghost_under,
                ghost_over: &ghost_over,
            },
            Some(rewo_gpu::container::screen_to_gui_placed(
                mouse,
                w,
                h,
                rewo_gpu::container::Placement::with_book(
                    layout.image_w as f32,
                    layout.image_h as f32,
                    book_open,
                ),
            )),
        ),
        baked.font.as_ref(),
    ) {
        labels.extend(enchant_cost_labels(rows, &font.advance, w, h));
    }
    // The text and the append cursor are labels; the insert cursor and the
    // selection travelled with the panel as solid quads — M93q's `FILL_SPRITE`
    // doing the job it was built for, one screen over.
    labels.extend(anvil_labels);
    // M100 — the book's search field, over the book's own panel.
    labels.extend(book_field_labels);
    apply_gui_icons(wr, gpu, gui, &icons);

    wr.set_container(
        true,
        hovered_slot_position(layout, mouse, w, h, book_open),
    );

    // Every visible slot's durability bar, plus the cursor's. The screen's
    // rects and the hotbar's go through the same builder.
    {
        let rects = menu_slot_rects(menu, w, h, book_open);
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
        // M106c — the frame's three producers, in vanilla's precedence. Named
        // rather than chained with `or_else` here: see `frame_tooltip`.
        frame_tooltip(
        &mut glyphs,
        |glyphs| screen_tooltip(
            menu,
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
            layout,
            session.menus.open(),
            session
                .game_state
                .game_mode()
                .is_some_and(|m| m.is_spectator()),
            book_open,
        ),
        // The book's cell tooltip, SECOND. Vanilla calls it AFTER the
        // container's and it still loses — first-wins, not last-wins.
        |glyphs| {
            book_tooltip(
                book.as_ref()?,
                book_overlay.is_some(),
                book_mouse,
                items,
                &baked.item_names,
                &baked.lang,
                flag,
                &advance,
                glyphs.as_deref_mut(),
                mouse,
                (w, h),
            )
        },
        // And the ghost's, THIRD — `RecipeBookComponent.extractTooltip` runs
        // the page's then the ghost's.
        |glyphs| {
            ghost_tooltip(
                &ghosts,
                ghost_cycle,
                layout,
                book_open,
                items,
                &baked.item_names,
                &advance,
                glyphs.as_deref_mut(),
                mouse,
                (w, h),
            )
        },
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
    player: EnchantPlayer<'_>,
    mouse_gui: Option<(f64, f64)>,
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
            // A background quad samples 1:1 — the panel sheet is drawn at its
            // own scale.
            sw: q.w as f32,
            sh: q.h as f32,
            tint: [1.0; 4],
        })
        .collect();
    Some(rewo_gpu::container::ContainerPanel {
        sheet,
        blits,
        gui_w: screen.image_w as f32,
        gui_h: screen.image_h as f32,
        overlays: {
            let mut o = menu_overlays(layout, open, player, mouse_gui);
            // M103 — the ghost's red wash goes UNDER the icons, so with the
            // screen's own overlays.
            o.extend_from_slice(player.ghost_under);
            o
        },
        front_overlays: player.ghost_over.to_vec(),
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
        // M93p — `None` resolves to the destination size, which every overlay
        // before the loom preview used.
        sw: b.source_size().0 as f32,
        sh: b.source_size().1 as f32,
        // M93q — a sprite blit is untinted; `to_fill` is the tinted form.
        tint: [1.0; 4],
    }
}

/// The scroller thumb's blit — 12x15 at `leftPos + 119`, whose y the caller
/// computes from `scrollOffs` (M93s).
fn cut_scroller_blit(y: i32) -> rewo_world::menu_screen::ProgressBlit {
    use rewo_world::menu_screen as ms;
    rewo_world::menu_screen::ProgressBlit {
        dx: ms::CUT_SCROLLER_X,
        dy: y,
        w: ms::CUT_SCROLLER_W,
        h: ms::CUT_SCROLLER_H,
        sx: 0,
        sy: 0,
        src: None,
    }
}

/// A solid-colour overlay quad (M93q) — the loom's grey banner backing, and
/// anything else vanilla draws with `GuiGraphics.fill`.
///
/// `rgb` is an **sRGB** 0xRRGGBB, which is how vanilla's colour constants are
/// written; the shader linearises it, the same discipline every Rewo UI pass
/// follows.
fn to_fill(
    b: rewo_world::menu_screen::ProgressBlit,
    rgb: u32,
) -> (usize, rewo_gpu::container::PanelBlit) {
    let c = |shift: u32| ((rgb >> shift) & 0xFF) as f32 / 255.0;
    (
        rewo_gpu::container::FILL_SPRITE,
        rewo_gpu::container::PanelBlit {
            tint: [c(16), c(8), c(0), 1.0],
            ..to_blit(b)
        },
    )
}

/// The open menu's enchanting rows, or `None` when it is not that menu.
///
/// **One derivation, two consumers** — the sprite overlays and the cost
/// labels. They must agree about which row is hovered and which is
/// unaffordable, because a row whose background says "available" while its
/// numeral says otherwise is not a state vanilla can produce; deriving it
/// twice gives two chances to disagree (M18's finding, in miniature).
pub(crate) fn enchant_rows_of(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    open: Option<&rewo_world::menu::OpenMenu>,
    player: EnchantPlayer<'_>,
    mouse_gui: Option<(f64, f64)>,
) -> Option<[rewo_world::menu_screen::EnchantRow; 3]> {
    if layout.protocol_id != 13 {
        return None;
    }
    let m = open?;
    Some(rewo_world::menu_screen::enchant_rows(
        m.enchant_costs(),
        m.enchant_lapis(),
        player.xp_level,
        player.creative,
        mouse_gui,
    ))
}

/// An enchanting-row press (M92f). Returns whether the press was taken.
///
/// Mirrors `EnchantmentScreen.mouseClicked`: the row loop runs first, and a
/// row is taken **only if `clickMenuButton` would return true**. A press on a
/// row that fails the gate is *not* consumed — vanilla falls through to
/// `super.mouseClicked`, so the slot logic still gets it.
///
/// The gate is deliberately not the render's: it additionally requires slot 0
/// to hold something, and tests the level against `row + 1` as well as the
/// cost. See `menu_screen::enchant_click_allowed`.
fn enchant_press(
    session: &mut PlaySession,
    screen: &ScreenState,
    w: f32,
    h: f32,
) -> bool {
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != 13 {
        return false;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
        screen.mouse,
        w,
        h,
        open.layout.image_w as f32,
        open.layout.image_h as f32,
    );
    let Some(row) = rewo_world::menu_screen::enchant_click_row(gx, gy) else {
        return false;
    };
    let allowed = rewo_world::menu_screen::enchant_click_allowed(
        row,
        open.enchant_costs(),
        open.enchant_lapis(),
        // `enchantSlots.getItem(0)` — the item being enchanted.
        open.menu.menu_slot(0).is_some(),
        session.hud.experience.level,
        session.abilities.instabuild,
    );
    if !allowed {
        return false;
    }
    if let Err(e) = session.container_button_click(row as i32) {
        log::warn!("enchant button {row}: {e}");
    }
    true
}

/// The beacon's live choice, from its data slots (M92).
///
/// **Vanilla's screen keeps its own `primary`/`secondary` fields**, seeded
/// from the menu by a `ContainerListener` on every `dataChanged` and then
/// moved by clicks *before* the server hears about them — a click updates the
/// screen and only `ServerboundSetBeaconPacket` on confirm tells the server.
/// Rewo has no click path here yet, so this reads the data slots directly:
/// the display is correct for whatever the server last said, and the
/// unconfirmed-choice half arrives with the button clicks.
/// `BeaconScreen`'s button press (M93m).
///
/// Placed beside `enchant_press` and called from the same seam, for the same
/// reason: `BeaconScreen.mouseClicked` is `AbstractContainerScreen`'s with
/// widgets in front of it, so a press on a live button consumes the click and
/// never reaches the slot logic. Returns whether it did.
///
/// The confirm sends and then closes; vanilla's order is the same
/// (`send(...)` then `closeContainer()`), and here the send failing must not
/// close the screen over a beacon the server never heard about.
fn beacon_press(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    effect_ids: &BeaconEffectIds,
    w: f32,
    h: f32,
) -> bool {
    use rewo_world::menu_screen::BeaconPress;
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != BEACON_MENU_PROTOCOL_ID {
        return false;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
        screen.mouse,
        w,
        h,
        open.layout.image_w as f32,
        open.layout.image_h as f32,
    );
    let choice = beacon_live(screen, open, effect_ids);
    let Some(button) = rewo_world::menu_screen::beacon_buttons()
        .into_iter()
        .find(|b| rewo_world::menu_screen::beacon_button_hovered(*b, gx, gy))
    else {
        return false;
    };
    match rewo_world::menu_screen::beacon_press(button, choice) {
        // An inactive or already-selected button is NOT a consumed click:
        // `AbstractWidget.mouseClicked` only returns true when it fires, so a
        // press on a dark button falls through to the slot logic exactly as a
        // disabled enchanting row does (M92f).
        BeaconPress::None => false,
        BeaconPress::Select(next) => {
            if let Some(b) = screen.beacon.as_mut() {
                b.choice = next;
            }
            true
        }
        BeaconPress::Confirm => {
            let id = |e: Option<rewo_world::menu_screen::BeaconEffect>| {
                e.and_then(|e| effect_ids.id_of(e))
            };
            match session.set_beacon(id(choice.primary), id(choice.secondary)) {
                Ok(()) => screen.close_beacon = true,
                Err(e) => log::warn!("set_beacon: {e}"),
            }
            true
        }
        BeaconPress::Cancel => {
            screen.close_beacon = true;
            true
        }
    }
}

/// `minecraft:menu`'s `beacon` id.
const BEACON_MENU_PROTOCOL_ID: i32 = 9;

/// What the loom's pattern grid needs, resolved by the caller (M93q).
///
/// Carried rather than derived in the overlay builder because the pattern list
/// keys off the pattern slot's item NAME, and only the caller holds the
/// registry that turns an id into one — the same reason `ItemProps` is an
/// input to the click arithmetic.
#[derive(Debug, Clone, Copy)]
pub struct LoomView {
    /// `getSelectablePatterns()`.
    pub patterns: &'static [&'static str],
    /// `LoomScreen.startRow` — the first visible row.
    ///
    /// **Always 0 today**: the scrollbar's drag is not wired, so a loom with
    /// more than 16 patterns shows the first sixteen and no more. The field
    /// exists because the index arithmetic needs it and would otherwise have
    /// a 0 baked in where a variable belongs.
    pub start_row: i32,
    /// `getSelectedBannerPatternIndex()`, from data slot 0. `-1` for none.
    pub selected: i32,
    /// `displayPatterns` — the grid is hidden entirely when false.
    pub display: bool,
}

/// `AnvilScreen.keyPressed` and `EditBox.charTyped` (M93t).
///
/// ```java
/// public boolean keyPressed(final KeyEvent event) {
///    if (event.isEscape()) { this.minecraft.player.closeContainer(); return true; }
///    return !this.name.keyPressed(event) && !this.name.canConsumeInput()
///        ? super.keyPressed(event) : true;
/// }
/// ```
///
/// **Read that return carefully.** It falls through to `super` only when the
/// box did *not* handle the key **and** cannot consume input. With the field
/// focused and editable — which it is whenever slot 0 holds something — the
/// second half is false, so **every non-escape key is swallowed**: `E` does not
/// close the anvil, a number key does not swap a hotbar slot, `Q` does not
/// drop. That reads like a bug and is exactly what typing a name requires.
///
/// With slot 0 empty the field is uneditable, `canConsumeInput` is false, and
/// the screen behaves normally again.
///
/// Returns whether the key was consumed.
fn anvil_key(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    items: &rewo_data::items::Items,
    // M101 — for `follow_cursor`, whose width function needs the font.
    baked: Option<&assets::BakedAssets>,
    input: rewo_world::edit_box::Input,
    clipboard: &mut String,
) -> bool {
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != ANVIL_MENU_PROTOCOL_ID {
        return false;
    }
    // Escape is handled by the screen's own close path, before the field.
    if input.key == 256 {
        return false;
    }
    let slot0 = open
        .menu
        .menu_slot(0)
        .and_then(|s| items.name(s.item_id).map(|n| (s, n)));
    let handled = {
        let local = anvil_local(screen, open, items);
        let handled = local.field.key_pressed(input, clipboard);
        rewo_world::anvil::key_consumed(handled, local.field.can_consume_input())
    };
    if let Some(a) = screen.anvil.as_mut() {
        follow_cursor(&mut a.field, baked, ANVIL_FIELD.2);
    }
    anvil_flush(session, screen, slot0);
    handled
}

/// A typed character — `EditBox.charTyped`, which the key path never sees
/// because winit reports text separately from the key (M93t).
fn anvil_char(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    items: &rewo_data::items::Items,
    baked: Option<&assets::BakedAssets>,
    ch: char,
) -> bool {
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != ANVIL_MENU_PROTOCOL_ID {
        return false;
    }
    let slot0 = open
        .menu
        .menu_slot(0)
        .and_then(|s| items.name(s.item_id).map(|n| (s, n)));
    let handled = anvil_local(screen, open, items).field.char_typed(ch);
    if let Some(a) = screen.anvil.as_mut() {
        follow_cursor(&mut a.field, baked, ANVIL_FIELD.2);
    }
    anvil_flush(session, screen, slot0);
    handled
}

/// Drain the field's responder into `AnvilName::on_name_changed`, and send the
/// packet it asks for.
///
/// The two-stage gate is M93n's and both stages are real: the field fires on
/// every mutation, `on_name_changed` normalises "the item's own name" to the
/// empty string, and `setItemName` refuses to re-send a name the server already
/// has.
fn anvil_flush(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    slot0: Option<(rewo_world::inventory::ItemSlot, &str)>,
) {
    let Some(local) = screen.anvil.as_mut() else {
        return;
    };
    let Some(typed) = local.field.take_value_changed() else {
        return;
    };
    let hover = slot0.map(|(_, n)| display_name_of(n));
    let input = slot0.zip(hover.as_deref()).map(|((s, _), hover_name)| {
        rewo_world::anvil::AnvilInput {
            // A stack whose patch carried anything is the closest Rewo gets to
            // `has(CUSTOM_NAME)` without decoding the component's text; the
            // approximation is one-directional, since a patched-but-unnamed
            // stack merely skips the clear-to-default normalisation.
            has_custom_name: s.has_components,
            hover_name,
        }
    });
    if let Some(send) = local.name.on_name_changed(&typed, input) {
        if let Err(e) = session.rename_item(&send) {
            log::warn!("anvil rename {send:?}: {e}");
        }
    }
}

/// `MerchantScreen.mouseClicked` — a trade button, or a scrollbar grab (M93u).
///
/// The button's press sets `shopItem = getIndex() + scrollOff` and then
/// `postButtonClick`, which does three things in order: `setSelectionHint`,
/// `tryMoveItems` and only then the packet. The first two are LOCAL — the
/// trade's items appear in the slots before the server answers — so a click
/// that the server later rejects still moved the screen first.
///
/// The scrollbar grab, like the stonecutter's, does **not** consume the press:
/// vanilla sets `isDragging` and falls through to `super.mouseClicked`.
/// The recipe book's press (M98).
///
/// **First of all**, and with a second rule under it:
///
/// ```java
/// if (this.recipeBookComponent.mouseClicked(...)) { … return true; }
/// else return this.widthTooNarrow && this.recipeBookComponent.isVisible()
///     ? true : super.mouseClicked(...);
/// ```
///
/// So a click the book does not want is still **swallowed** when the window is
/// too narrow and the book is open — the case where the book covers the menu.
/// Letting it fall through there would click a slot the player cannot see.
///
/// Returns whether the press was consumed.
fn book_press(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    items: &rewo_data::items::Items,
    // M107 — `event.hasShiftDown()`, which vanilla passes straight through to
    // `useMaxItems`: shift-clicking a recipe places as many as the ingredients
    // allow rather than one.
    shift: bool,
    // M99 — the search's inputs, so the press resolves the SAME page the render
    // did: a search narrows the page, and hit-testing against an unfiltered one
    // would place a recipe the player is not looking at.
    display: &std::collections::HashMap<String, String>,
    right: bool,
    w: f32,
    h: f32,
) -> bool {
    use rewo_world::recipe_book_screen as rb;
    // `!minecraft.player.isSpectator()` — a spectator cannot click the book at
    // all, and the guard is on the whole method rather than on any one widget.
    if session.own_game_mode().is_some_and(|g| g.is_spectator()) {
        return false;
    }
    let (bl, bt, scale) = rewo_gpu::container::recipe_book_origin(w, h);
    let bx = ((screen.mouse.0 - bl as f64) / scale as f64).floor() as i32;
    let by = ((screen.mouse.1 - bt as f64) / scale as f64).floor() as i32;
    // Bound before the overlay branch, because both placement paths need it and
    // one `let` is how they cannot disagree.
    let narrow = rb::width_too_narrow((w / scale) as i32);
    // M104 — an OPEN which-of-these overlay eats every click, wherever it
    // lands. `RecipeBookPage.mouseClicked`'s overlay branch is an unconditional
    // `return true`, so while it is up the whole screen is modal: a click on
    // the page's arrows, on the search box, on the tabs, or on the menu's own
    // slots underneath all reach the overlay and nothing else.
    if let Some(open) = screen.book_overlay.as_ref() {
        match open.click_at(bx, by, right) {
            rewo_world::recipe_overlay::Click::Select(i) => {
                let picked = open.buttons.get(i).map(|b| (b.recipe, b.craftable));
                // **The overlay STAYS OPEN.** The selecting branch does not
                // call `setVisible(false)` — only the else does — so picking a
                // variant leaves the popup up and you can pick another. That
                // reads like an oversight and is what makes the feature usable.
                if let Some((id, craftable)) = picked {
                    place_from_book(session, screen, id, craftable, shift, narrow);
                }
            }
            rewo_world::recipe_overlay::Click::Close => screen.book_overlay = None,
        }
        return true;
    }
    let query = rewo_world::recipe_search::normalize(&screen.book_search.value());
    let Some(view) =
        live_recipe_book(session, items, screen.book, Some((bx, by)), &query, display)
            .and_then(|b| b.view)
    else {
        return false;
    };
    let hit = rb::book_hit(bx, by, view, view.tabs);
    let action = screen.book.press(hit, right);
    // The `EditBox` is the ONLY owner of "is the search focused" — see
    // `focus_change`'s docs for why the duplicate flag it replaced was a bug.
    if let Some(v) = rb::focus_change(hit) {
        screen.book_search.set_focused(v);
    }
    match action {
        Some(rb::BookAction::ToggleFilter) => {
            // `toggleFiltering()` then `sendUpdateSettings()` — the local flag
            // moves first and the packet reports it, which is why this is one
            // call rather than two.
            if let Err(e) = session.toggle_recipe_book_filter(shown_book_index(session))
            {
                log::warn!("rewo: recipe book filter: {e}");
            }
            true
        }
        Some(rb::BookAction::Recipe { index, right }) => {
            let render =
                live_recipe_book(session, items, screen.book, Some((bx, by)), &query, display);
            if right {
                // M104 — open the which-of-these overlay, but only on a cell
                // holding more than one recipe: `!button.isOnlyOption()`.
                //
                // `isOnlyOption` is `size() == 1`, not `size() > 1` negated —
                // an empty collection opens an (empty) overlay in vanilla too.
                let collection = render
                    .as_ref()
                    .and_then(|b| b.slot_collections.get(index))
                    .cloned()
                    .unwrap_or_default();
                if collection.len() != 1 {
                    screen.book_overlay = Some(open_overlay(collection, index, view));
                }
            } else {
                let picked = render.as_ref().and_then(|b| {
                    let id = (*b.slot_recipes.get(index)?)?;
                    // **`isCraftable(recipe)`, not `hasCraftable()`.** The
                    // cell's `(craftable, multiple)` pair carries the
                    // COLLECTION's answer, which is true when ANY of its
                    // recipes can be made — so on a group holding one craftable
                    // and one not, using it would let the uncraftable one be
                    // clicked forever. The per-recipe flag rides on the same
                    // `Button`s the which-of-these overlay is built from.
                    let craftable = b
                        .slot_collections
                        .get(index)?
                        .iter()
                        .find(|btn| btn.recipe == id)
                        .is_some_and(|btn| btn.craftable);
                    Some((id, craftable))
                });
                if let Some((id, craftable)) = picked {
                    place_from_book(session, screen, id, craftable, shift, narrow);
                }
            }
            true
        }
        Some(rb::BookAction::Navigated) => true,
        // Missed the book — swallowed anyway on a narrow window.
        None => narrow,
    }
}

/// `RecipeBookComponent.tryPlaceRecipe` and what its caller does with the
/// answer (M107).
///
/// Both placement paths — a page cell and a which-of-these button — funnel
/// through here, because vanilla has one `tryPlaceRecipe` and the guard is
/// stateful: two copies would each hold half the history and neither would
/// suppress correctly.
///
/// Three things happen on a successful placement, and only the packet is
/// obvious:
///
/// * **The ghost is cleared first.** `ghostSlots.clear()` runs before
///   `handlePlaceRecipe`, so a failed placement leaves the screen briefly with
///   no ghost until the server's reply refills it. Keeping the old one would
///   show the previous recipe's ingredients against the new request.
/// * **The book closes on a narrow window** — `if (!isOffsetNextToMainGUI())
///   setVisible(false)`, and `xOffset` is 0 exactly when the window is too
///   narrow. There the book covers the menu you are about to look at.
/// * `lastRecipe`/`lastRecipeCollection` are recorded for the Enter-key
///   re-place. **Rewo has no key path into the book yet**, so those are not
///   modelled; what matters here is that a SUPPRESSED click must not record
///   them either, and it does not, because it never gets past the guard.
///
/// Returns whether the click placed.
fn place_from_book(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    recipe: i32,
    craftable: bool,
    use_max_items: bool,
    narrow: bool,
) -> bool {
    let effects = rewo_world::recipe_book_screen::place_effects(
        &mut screen.place_guard,
        recipe,
        craftable,
        use_max_items,
        narrow,
    );
    let Some(use_max_items) = effects.send else {
        return false;
    };
    if effects.clear_ghost {
        session.ghost_recipe = None;
    }
    if let Err(e) = session.place_recipe(recipe, use_max_items) {
        log::warn!("rewo: place_recipe: {e}");
    }
    if effects.close_book {
        // `setVisible(false)` — which also shuts the which-of-these overlay
        // (`recipeBookPage.setInvisible()`) and tells the server.
        screen.book_overlay = None;
        let book = shown_book_index(session);
        session.set_recipe_book_open(book, false);
        if let Err(e) = session.recipe_book_change_settings(book) {
            log::warn!("rewo: recipe book settings: {e}");
        }
    }
    true
}

/// Which of the four `RecipeBookSettings` slots the shown menu reads (M98).
fn shown_book_index(session: &PlaySession) -> usize {
    book_type_of(session.shown_menu().layout()).map_or(0, |b| b.index())
}

fn merchant_press(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    w: f32,
    h: f32,
) -> bool {
    use rewo_world::merchant_screen as ms;
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != ms::MERCHANT_MENU_PROTOCOL_ID {
        return false;
    }
    let n = session.merchant.as_ref().map_or(0, |m| m.offers.len());
    let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
        screen.mouse,
        w,
        h,
        open.layout.image_w as f32,
        open.layout.image_h as f32,
    );
    let scroll_off = screen.merchant.map_or(0, |l| l.scroll_off);
    // The grab is tested FIRST in vanilla and does not return; the button is
    // an ordinary widget press, which happens inside `super`.
    if ms::can_scroll(n) && ms::scroller_grabbed(gx, gy) {
        if let Some(l) = screen.merchant.as_mut() {
            l.dragging = true;
        }
    }
    let Some(button) = ms::button_at(gx, gy) else {
        return false;
    };
    let offer = ms::offer_for_button(button, scroll_off);
    // A button past the end of the list is drawn but dead — vanilla hides the
    // widget (`visible = false`) rather than disabling it, so there is nothing
    // there to press.
    if offer as usize >= n {
        return false;
    }
    if let Some(l) = screen.merchant.as_mut() {
        // `postButtonClick` sets `shopItem` LOCALLY first — the trade's items
        // appear before the server answers.
        l.selected = offer;
    }
    if let Err(e) = session.select_trade(offer) {
        log::warn!("select_trade {offer}: {e}");
    }
    true
}

/// What the merchant's trade list needs, resolved by the caller (M93u).
#[derive(Debug, Clone)]
pub struct MerchantView {
    /// The offers as sent, in the order the click's index addresses.
    pub offers: Vec<rewo_net::merchant::MerchantOffer>,
    /// `MerchantScreen.scrollOff` — an **offer index**, not a fraction.
    pub scroll_off: i32,
    /// Each offer's modified cost-A count, resolved here because the clamp's
    /// ceiling is the item's own max stack size and only this side holds the
    /// item table.
    pub cost_a_counts: Vec<i32>,
    /// `MerchantScreen.shopItem` — the selected offer, whose out-of-stock X
    /// is drawn in the right-hand panel. Screen-local: the packet does not
    /// carry a selection.
    pub selected: i32,
    /// `getTraderLevel` / `getTraderXp`, straight off the packet.
    pub level: i32,
    pub xp: i32,
    /// `showProgressBar()` — false for a wandering trader, which has no level
    /// and no bar.
    pub show_progress: bool,
    /// `getFutureTraderXp` — the xp the currently-matched offer would grant.
    ///
    /// **Derived here rather than received**: no packet carries it.
    /// `MerchantContainer.updateSellItem` matches the payment slots against
    /// the offers and takes the matched offer's xp, and the client holds both
    /// halves. See `merchant_future_xp` for the one case it declines.
    pub future_xp: i32,
}

/// `MerchantContainer.updateSellItem`'s `futureXp` (M93v).
///
/// Vanilla derives it: the payment slots are matched against the offers by
/// `getRecipeFor`, and the matched offer's xp is what the bar's result segment
/// shows. Both halves are on the client, so this is a derivation and not a
/// gap — the same shape as M93u's four class-C corrections one level down.
///
/// **The one case it declines.** `ItemCost.test` is `stack.is(item) &&
/// components.test(stack)`, and the second half is a
/// `DataComponentExactPredicate` — per-component *values*, where M41 gives
/// Rewo a digest of the whole patch. So a **constrained** cost cannot be
/// evaluated, and an offer carrying one is treated as unmatched. The
/// consequence is narrow and one-directional: the result segment is missing
/// where vanilla would show it, never present where vanilla would not. Vanilla
/// villager trades are plain items, so in practice this is the enchanted-book
/// and dyed-armour tail.
///
/// The slot order is `MerchantContainer`'s own: **if slot 0 is empty, slot 1
/// becomes `buyA` and `buyB` is empty** — so paying with only the second slot
/// still matches a one-item trade.
fn merchant_future_xp(
    m: &rewo_world::menu::OpenMenu,
    offers: &rewo_net::merchant::MerchantOffers,
    selected: i32,
    props: &dyn Fn(i32) -> i32,
) -> i32 {
    let slot = |i: usize| m.menu.menu_slot(i).map(|s| (s.item_id, s.count));
    let matches: Vec<rewo_world::merchant_screen::OfferMatch> = offers
        .offers
        .iter()
        .map(|o| rewo_world::merchant_screen::OfferMatch {
            cost_a_item: o.cost_a.item_id,
            need_a: o.modified_cost_a(props(o.cost_a.item_id)),
            cost_b: o.cost_b.as_ref().map(|c| (c.item_id, c.count)),
            constrained: o.cost_a.constrained
                || o.cost_b.as_ref().is_some_and(|c| c.constrained),
        })
        .collect();
    let satisfied = rewo_world::merchant_screen::satisfied_offers(&matches, slot(0), slot(1));
    rewo_world::merchant_screen::recipe_for(selected, &satisfied)
        .map_or(0, |i| offers.offers[i].xp)
}

/// The merchant screen's scroll, which no packet carries (M93u).
#[derive(Debug, Clone, Copy, Default)]
pub struct MerchantLocal {
    container_id: i32,
    pub scroll_off: i32,
    pub selected: i32,
    pub dragging: bool,
}

/// Resolve the view, seeding the scroll on a new container.
pub(crate) fn merchant_view(
    screen: &mut ScreenState,
    m: &rewo_world::menu::OpenMenu,
    offers: &rewo_net::merchant::MerchantOffers,
    props: &dyn Fn(i32) -> i32,
) -> MerchantView {
    let stale = screen.merchant.is_none_or(|l| l.container_id != m.container_id);
    if stale {
        screen.merchant = Some(MerchantLocal {
            container_id: m.container_id,
            scroll_off: 0,
            selected: 0,
            dragging: false,
        });
    }
    let local = screen.merchant.expect("just seeded");
    MerchantView {
        level: offers.villager_level,
        xp: offers.villager_xp,
        show_progress: offers.show_progress,
        future_xp: merchant_future_xp(m, offers, local.selected, props),
        cost_a_counts: offers
            .offers
            .iter()
            .map(|o| o.modified_cost_a(props(o.cost_a.item_id)))
            .collect(),
        offers: offers.offers.clone(),
        // Clamped on read rather than on write: the list can SHRINK under a
        // held scroll when the villager restocks, and vanilla's own guard is
        // `offer_visible`'s `!canScroll` short-circuit rather than a stored
        // clamp.
        selected: local.selected,
        scroll_off: local
            .scroll_off
            .min(rewo_world::merchant_screen::max_scroll_off(offers.offers.len()).max(0)),
    }
}

/// `minecraft:menu`'s `anvil` id.
pub const ANVIL_MENU_PROTOCOL_ID: i32 = 8;

/// The anvil screen's name field and the name it has sent (M93t).
#[derive(Debug, Clone, Default)]
pub struct AnvilLocal {
    container_id: i32,
    /// Slot 0 as last seen, so `slotChanged` fires exactly when vanilla's does.
    slot0: Option<(i32, u64)>,
    pub field: rewo_world::edit_box::EditBox,
    pub name: rewo_world::anvil::AnvilName,
}

/// `AnvilScreen.subInit` + `slotChanged` — seed the field, and re-seed it
/// whenever slot 0 changes (M93t).
///
/// ```java
/// public void slotChanged(container, slotIndex, itemStack) {
///    if (slotIndex == 0) {
///       this.name.setValue(itemStack.isEmpty() ? "" : itemStack.getHoverName().getString());
///       this.name.setEditable(!itemStack.isEmpty());
///       this.setFocused(this.name);
///    }
/// }
/// ```
///
/// So an **empty** input slot leaves an empty, UNEDITABLE field — and that
/// matters for far more than the text, because `AnvilScreen.keyPressed` falls
/// through to `super` only when the box can consume nothing. See `anvil_key`.
fn anvil_local<'a>(
    screen: &'a mut ScreenState,
    m: &rewo_world::menu::OpenMenu,
    items: &rewo_data::items::Items,
) -> &'a mut AnvilLocal {
    let slot0 = m.menu.menu_slot(0).map(|s| (s.item_id, s.components));
    let stale = match &screen.anvil {
        Some(a) => a.container_id != m.container_id || a.slot0 != slot0,
        None => true,
    };
    if stale {
        let mut field = anvil_field_new();
        let hover = slot0
            .and_then(|(id, _)| items.name(id))
            .map(display_name_of)
            .unwrap_or_default();
        field.set_value(&hover);
        field.set_editable(slot0.is_some());
        let mut local = AnvilLocal {
            container_id: m.container_id,
            slot0,
            field,
            name: screen
                .anvil
                .as_ref()
                .filter(|a| a.container_id == m.container_id)
                .map(|a| a.name.clone())
                .unwrap_or_default(),
        };
        // The seeding `setValue` fires the responder, as vanilla's does; drain
        // it so the re-seed does not send a rename of the name we were just
        // told.
        let _ = local.field.take_value_changed();
        screen.anvil = Some(local);
    }
    screen.anvil.as_mut().expect("just seeded")
}

/// A stack's `getHoverName().getString()` for the seed — the item's display
/// name, which for an un-renamed stack is its translated name.
///
/// **An approximation, and a recorded one**: Rewo resolves the display name
/// from the item registry, so a stack carrying a `custom_name` component seeds
/// the field with its *default* name instead. M41 decodes the component's
/// bytes but not its text, which is the same wall the tooltip's name override
/// hits. The consequence is narrow — the field starts on the wrong string for
/// an already-renamed item — and it does not reach the wire, because
/// `on_name_changed` compares against what the server was told.
fn display_name_of(id: &str) -> String {
    id.rsplit(':')
        .next()
        .unwrap_or(id)
        .split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the stonecutter's recipe grid needs, resolved by the caller (M93s).
///
/// Carried rather than derived in the overlay builder for `LoomView`'s reason:
/// the list keys off the input slot's item NAME, and only this side holds the
/// registry that turns an id into one.
#[derive(Debug, Clone)]
pub struct CutView {
    /// `getVisibleRecipes()` — `selectByInput` of the input slot, **in master
    /// order**, because the index a click sends indexes this.
    pub recipes: Vec<&'static rewo_data::stonecutter_table::Cut>,
    /// `StonecutterScreen.startIndex` — the first visible recipe. A multiple
    /// of 4, since the grid scrolls by whole rows.
    pub start_index: i32,
    /// `getSelectedRecipeIndex()`, from data slot 0.
    pub selected: i32,
    /// `scrollOffs`, for the thumb's position.
    pub scroll_offs: f32,
    /// `displayRecipes` — the grid, the icons and the scrollbar are all hidden
    /// when false.
    pub display: bool,
}

/// The stonecutter screen's scroll, which is **screen-local**: no packet
/// carries it, and vanilla resets it in `containerChanged` (M93s).
#[derive(Debug, Clone, Copy)]
pub struct CutLocal {
    container_id: i32,
    /// The input slot as last seen. `containerChanged` is registered as the
    /// menu's update listener and fires on **any** change to the input
    /// container — so taking one block off a stack resets the scroll even
    /// though `slotsChanged` rebuilds the recipe list only when the item TYPE
    /// changes. Two granularities on one event, and this is the screen's.
    input: Option<(i32, i32)>,
    scroll_offs: f32,
    /// Whether the thumb is being dragged. Cleared on any release.
    pub scrolling: bool,
}

/// `StonecutterScreen.mouseClicked` (M93s).
///
/// ```java
/// if (this.displayRecipes) {
///    for (int index = this.startIndex; index < endIndex; index++) {
///       … if (hit && this.menu.clickMenuButton(player, index)) {
///          play(UI_STONECUTTER_SELECT_RECIPE);
///          this.minecraft.gameMode.handleInventoryButtonClick(this.menu.containerId, index);
///          return true;
///       }
///    }
///    if (over the scrollbar) this.scrolling = true;   // and does NOT return
/// }
/// return super.mouseClicked(event, doubleClick);
/// ```
///
/// So a recipe consumes the press and a scrollbar grab does not — the grab
/// falls through to the slot logic, which finds nothing under the bar.
/// Returns whether the press was consumed.
fn cut_press(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    items: &rewo_data::items::Items,
    w: f32,
    h: f32,
) -> bool {
    use rewo_world::menu_screen as ms;
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != ms::STONECUTTER_MENU_PROTOCOL_ID {
        return false;
    }
    let view = cut_view(screen, open, items);
    if !view.display {
        // The whole block is inside `if (this.displayRecipes)`, so with no
        // grid there is no recipe click AND no scrollbar grab.
        return false;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
        screen.mouse,
        w,
        h,
        open.layout.image_w as f32,
        open.layout.image_h as f32,
    );
    if let Some(index) = ms::cut_cell_click_at(gx, gy, view.start_index) {
        // `isValidRecipeIndex` is the server's gate and the screen's: vanilla
        // calls `clickMenuButton` inside the hit test, so an out-of-range cell
        // does not consume the press either.
        if ms::cut_click_accepted(index, view.recipes.len()) {
            if let Err(e) = session.container_button_click(index) {
                log::warn!("stonecutter recipe {index}: {e}");
            }
            return true;
        }
    }
    if ms::cut_scroller_grabbed(gx, gy) {
        if let Some(c) = screen.cut.as_mut() {
            c.scrolling = true;
        }
    }
    false
}

/// The whole stonecutter view, resolved from the menu plus the screen's own
/// scroll — the caller's job, exactly as `beacon_live` is (M93s).
pub(crate) fn cut_view(
    screen: &mut ScreenState,
    m: &rewo_world::menu::OpenMenu,
    items: &rewo_data::items::Items,
) -> CutView {
    // Slots: input 0, result 1, then the player's 36.
    let name = m.menu.menu_slot(0).and_then(|s| items.name(s.item_id));
    let recipes = name.map_or_else(Vec::new, rewo_data::stonecutter_table::select_by_input);
    let local = cut_local(screen, m);
    let display = rewo_world::menu_screen::cut_display_recipes(name.is_some(), recipes.len());
    CutView {
        start_index: if display {
            rewo_world::menu_screen::cut_start_index(local.scroll_offs, recipes.len())
        } else {
            0
        },
        selected: m.data(0) as i32,
        scroll_offs: local.scroll_offs,
        display,
        recipes,
    }
}

/// `StonecutterScreen`'s scroll, seeded at 0 and owned by the screen until the
/// input changes — the beacon's shape (M93m), with a different reset trigger.
fn cut_local(screen: &mut ScreenState, m: &rewo_world::menu::OpenMenu) -> CutLocal {
    let input = m.menu.menu_slot(0).map(|s| (s.item_id, s.count));
    let stale = match screen.cut {
        Some(c) => c.container_id != m.container_id || c.input != input,
        None => true,
    };
    if stale {
        screen.cut = Some(CutLocal {
            container_id: m.container_id,
            input,
            scroll_offs: 0.0,
            scrolling: false,
        });
    }
    screen.cut.expect("just seeded")
}

/// The beacon screen's local choice and the watermarks it was seeded at.
#[derive(Debug, Clone, Copy)]
pub struct BeaconLocal {
    container_id: i32,
    data_writes: u64,
    choice: rewo_world::menu_screen::BeaconChoice,
}

/// `BeaconScreen`'s live choice — seeded from the menu, then owned by the
/// screen until the menu says otherwise (M93m).
fn beacon_live(
    screen: &mut ScreenState,
    m: &rewo_world::menu::OpenMenu,
    effect_ids: &BeaconEffectIds,
) -> rewo_world::menu_screen::BeaconChoice {
    let stale = match screen.beacon {
        Some(b) => b.container_id != m.container_id || b.data_writes != m.data_writes,
        None => true,
    };
    if stale {
        screen.beacon = Some(BeaconLocal {
            container_id: m.container_id,
            data_writes: m.data_writes,
            choice: beacon_choice(m, effect_ids),
        });
    }
    // `levels` and `has_payment` are the MENU's on every frame, not the
    // screen's: `updateStatus(levels)` is passed the menu's value each time
    // and `hasPayment()` reads the slot directly. Only the two effects are
    // screen-owned, so a payment arriving mid-selection lights Confirm
    // without disturbing the pick.
    let mut c = screen.beacon.expect("seeded above").choice;
    c.levels = m.beacon_levels();
    c.has_payment = m.beacon_has_payment();
    c
}

fn beacon_choice(
    m: &rewo_world::menu::OpenMenu,
    effect_ids: &BeaconEffectIds,
) -> rewo_world::menu_screen::BeaconChoice {
    rewo_world::menu_screen::BeaconChoice {
        levels: m.beacon_levels(),
        primary: m.beacon_primary().and_then(|id| effect_ids.of(id)),
        secondary: m.beacon_secondary().and_then(|id| effect_ids.of(id)),
        has_payment: m.beacon_has_payment(),
    }
}

/// The six beacon effects' `minecraft:mob_effect` registry ids.
///
/// From `rewo_data`'s report-backed table (M92c) — by NAME, never by position.
/// An unresolvable name leaves that slot `None` and the effect simply does not
/// match, which shows as a button with no icon rather than the *wrong* icon.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BeaconEffectIds([Option<i32>; 6]);

impl BeaconEffectIds {
    pub(crate) fn resolve(m: &rewo_data::mob_effects::MobEffects) -> Self {
        use rewo_world::menu_screen::BeaconEffect;
        Self(std::array::from_fn(|i| m.id_of(BeaconEffect::ALL[i].name())))
    }

    /// The registry id of one of the six, for `set_beacon` (M93m).
    fn id_of(&self, e: rewo_world::menu_screen::BeaconEffect) -> Option<i32> {
        let i = rewo_world::menu_screen::BeaconEffect::ALL
            .iter()
            .position(|x| *x == e)?;
        self.0[i]
    }

    /// Which of the six a registry id is, or `None` for any other effect.
    fn of(&self, id: i32) -> Option<rewo_world::menu_screen::BeaconEffect> {
        self.0
            .iter()
            .position(|e| *e == Some(id))
            .map(|i| rewo_world::menu_screen::BeaconEffect::ALL[i])
    }
}

/// What an enchanting-table row's state selects, as `(row sprite, numeral)`.
///
/// Kept beside the overlay builder rather than in `rewo-world` because the
/// indices are `rewo-data`'s and the state is `rewo-world`'s; this is the one
/// place both are in scope.
/// **The numeral goes through `EnchantRow::numeral()`, not a second match.**
/// This function had its own copy of that mapping, and a mutation found the
/// duplication: emptying `numeral()` changed nothing rendered, because nothing
/// rendered was reading it. That is M18's finding and M45's in one — a second
/// derivation of the same fact is a second chance to disagree, and here it was
/// also a witness grading a function the app did not call.
fn enchant_row_sprites(
    i: usize,
    row: rewo_world::menu_screen::EnchantRow,
) -> (usize, Option<usize>) {
    use rewo_data::assets as a;
    use rewo_world::menu_screen::EnchantRow;
    let background = match row {
        // `cost == 0` blits the disabled background and RETURNS; an
        // unaffordable offer blits the SAME one and then its numeral.
        EnchantRow::Empty | EnchantRow::Unaffordable { .. } => a::ENCHANT_ROW_DISABLED,
        EnchantRow::Available { .. } => a::ENCHANT_ROW,
        EnchantRow::Hovered { .. } => a::ENCHANT_ROW_HIGHLIGHTED,
    };
    let numeral = row
        .numeral()
        .map(|greyed| if greyed { a::ENCHANT_LEVEL_DISABLED } else { a::ENCHANT_LEVEL } + i);
    (background, numeral)
}

/// Everything an open menu paints over its background sheet, in draw order
/// (M91 the furnaces, M92 the rest).
///
/// Empty for a menu with no overlays *and* for one that is not open — a
/// `containershot` panel built with no `OpenMenu` has no data slots to read,
/// and inventing a plausible-looking half-lit furnace would make the gate's
/// panel witnesses grade a state no server ever sent.
///
/// `player` carries the two inputs the enchanting table needs that are not the
/// menu's at all — the local XP level and the creative flag.
fn menu_overlays(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    open: Option<&rewo_world::menu::OpenMenu>,
    player: EnchantPlayer<'_>,
    mouse_gui: Option<(f64, f64)>,
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
        // loom (M93q): for each visible cell, the button chrome then the
        // banner preview — a grey fill under an untinted pattern.
        rewo_world::menu_screen::LOOM_MENU_PROTOCOL_ID => {
            let Some(l) = player.loom.filter(|l| l.display) else {
                return out;
            };
            for row in 0..rewo_world::menu_screen::LOOM_ROWS {
                for col in 0..rewo_world::menu_screen::LOOM_COLS {
                    let index = (row + l.start_row) * rewo_world::menu_screen::LOOM_COLS + col;
                    // `break label82` — the grid stops at the end of the list
                    // rather than drawing empty cells.
                    let Some(pattern) = usize::try_from(index)
                        .ok()
                        .and_then(|i| l.patterns.get(i))
                    else {
                        break;
                    };
                    let (cx, cy) = rewo_world::menu_screen::loom_cell_origin(row, col);
                    let cell = rewo_world::menu_screen::ProgressBlit {
                        dx: cx,
                        dy: cy,
                        w: rewo_world::menu_screen::LOOM_CELL,
                        h: rewo_world::menu_screen::LOOM_CELL,
                        sx: 0,
                        sy: 0,
                        src: None,
                    };
                    let hovered = mouse_gui.is_some_and(|(x, y)| {
                        rewo_world::menu_screen::loom_cell_at(x, y, l.start_row) == Some(index)
                    });
                    out.push((
                        a::LOOM_PATTERN_CHROME
                            + if index == l.selected {
                                0
                            } else if hovered {
                                1
                            } else {
                                2
                            },
                        to_blit(cell),
                    ));
                    // The grey backing FIRST, then the pattern over it. The
                    // order is the whole reason fills share the sprites' list.
                    out.push(to_fill(
                        rewo_world::menu_screen::loom_preview_backing(cx, cy),
                        rewo_world::menu_screen::LOOM_PREVIEW_BACKING,
                    ));
                    if let Some(sprite) = a::banner_pattern_overlay(pattern) {
                        out.push((
                            sprite,
                            to_blit(rewo_world::menu_screen::loom_pattern_preview(cx, cy)),
                        ));
                    }
                }
            }
        }
        // merchant (M93u): the scroller, then per visible offer a trade arrow.
        // The three ITEMS per row are icons and go through the GUI-item pass.
        rewo_world::merchant_screen::MERCHANT_MENU_PROTOCOL_ID => {
            use rewo_world::merchant_screen as ms;
            let Some(v) = player.merchant else { return out };
            let n = v.offers.len();
            if let Some(y) = ms::scroller_y(v.scroll_off, n) {
                out.push((
                    a::VILLAGER_SCROLLER + usize::from(!ms::can_scroll(n)),
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: ms::SCROLL_X,
                        dy: y,
                        w: ms::SCROLLER_W,
                        h: ms::SCROLLER_H,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
            // The trade buttons FIRST (M93x) — `addRenderableWidget` puts them
            // in the widget layer, which `extractBackground` has already run
            // under. Drawn after the arrow they would cover it.
            //
            // A row past the end of the list draws nothing: vanilla toggles
            // `visible`, not `active`, so there is no greyed button.
            for (i, _) in v.offers.iter().enumerate() {
                let i = i as i32;
                if !ms::offer_visible(i, v.scroll_off, n) {
                    continue;
                }
                let row = if ms::can_scroll(n) { i - v.scroll_off } else { i };
                let hovered = mouse_gui.is_some_and(|(x, y)| ms::button_hovered(row, x, y));
                for sl in ms::button_slices(ms::TRADE_BUTTON_W) {
                    if sl.w == 0 {
                        continue;
                    }
                    out.push((
                        a::WIDGET_BUTTON + usize::from(hovered),
                        to_blit(rewo_world::menu_screen::ProgressBlit {
                            dx: ms::TRADE_BUTTON_X + sl.dx,
                            dy: ms::button_y(row),
                            w: sl.w,
                            h: ms::TRADE_BUTTON_H,
                            sx: sl.sx,
                            sy: 0,
                            // 1:1 — the source size equals the destination, so
                            // this is a tile and not a scale.
                            src: None,
                        }),
                    ));
                }
            }
            for (i, offer) in v.offers.iter().enumerate() {
                let i = i as i32;
                if !ms::offer_visible(i, v.scroll_off, n) {
                    continue;
                }
                // The row is the offer's position in the WINDOW, which is the
                // offer index only when nothing is scrolled.
                let row = if ms::can_scroll(n) { i - v.scroll_off } else { i };
                let y = ms::row_item_y(row);
                // `xo + 5 + 35 + 20` — past cost B, not past cost A. The first
                // cut of this read `5 + 5 + 20` and put every arrow 30 px left,
                // on top of the cost-A icon.
                out.push((
                    a::VILLAGER_TRADE_ARROW + usize::from(offer.out_of_stock),
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: ms::COST_B_X + 20,
                        dy: y + 3,
                        w: 10,
                        h: 9,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
            // The discount strikethrough (M93w), through the FIRST number.
            for (i, offer) in v.offers.iter().enumerate() {
                let idx = i as i32;
                if !ms::offer_visible(idx, v.scroll_off, n) {
                    continue;
                }
                if !ms::cost_a_display(offer.cost_a.count, v.cost_a_counts[i]).strikethrough {
                    continue;
                }
                let row = if ms::can_scroll(n) { idx - v.scroll_off } else { idx };
                out.push((
                    a::VILLAGER_STRIKETHROUGH,
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: ms::COST_A_X + ms::STRIKETHROUGH_DX,
                        dy: ms::row_item_y(row) + ms::STRIKETHROUGH_DY,
                        w: ms::STRIKETHROUGH_W,
                        h: ms::STRIKETHROUGH_H,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
            // The XP bar (M93v): background, then the fill, then the result
            // segment — which samples the sprite from `w` rather than 0, so it
            // CONTINUES the gradient where the fill stopped.
            //
            // `showProgressBar()` gates it: a wandering trader has no level.
            if v.show_progress {
                if let Some((fill, future)) = ms::xp_bar(v.level, v.xp, v.future_xp) {
                    let bar = |dx: i32, sx: i32, w: i32| rewo_world::menu_screen::ProgressBlit {
                        dx,
                        dy: ms::XP_BAR_Y,
                        w,
                        h: ms::XP_BAR_H,
                        sx,
                        sy: 0,
                        // `None` — the source size EQUALS the destination, so
                        // this is a 1:1 slice at `sx` and not a scale.
                        //
                        // `blitSprite(sprite, 102, 5, u, v, x, y, w, h)` passes
                        // the SHEET's size, not the source rect's: the rect is
                        // `w x h` at `(u, v)`. Setting `src` to (102, 5) — the
                        // obvious reading of those two arguments — squeezes the
                        // whole bar into the segment, which a mutation caught
                        // by NOT dying: the `sx` offset had no visible effect
                        // because every segment was showing the entire sprite.
                        src: None,
                    };
                    out.push((
                        a::VILLAGER_XP_BAR,
                        to_blit(bar(ms::XP_BAR_X, 0, ms::XP_BAR_W)),
                    ));
                    if fill > 0 {
                        out.push((a::VILLAGER_XP_BAR + 1, to_blit(bar(ms::XP_BAR_X, 0, fill))));
                    }
                    if future > 0 {
                        out.push((
                            a::VILLAGER_XP_BAR + 2,
                            to_blit(bar(ms::XP_BAR_X + fill, fill, future)),
                        ));
                    }
                }
            }
            // The 28x21 red X is NOT per row — `extractButtonArrows` only
            // swaps the arrow. It belongs to the SELECTED offer, in the
            // right-hand trading panel at `leftPos + 83 + 99`.
            if let Some(sel) = usize::try_from(v.selected)
                .ok()
                .and_then(|i| v.offers.get(i))
                .filter(|o| o.out_of_stock)
            {
                let _ = sel;
                out.push((
                    a::VILLAGER_OUT_OF_STOCK,
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: 182,
                        dy: 35,
                        w: 28,
                        h: 21,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
        }
        // anvil (M93t): the name field's cursor and selection, measured by
        // `anvil_field_render` alongside the text so the two cannot disagree
        // about where the run ends.
        ANVIL_MENU_PROTOCOL_ID => {
            // `extractBackground` blits the field's own 110x16 background at
            // (59, 20) — over a RED placeholder baked into `anvil.png`, so
            // this is chrome the screen cannot omit. The pair is chosen by the
            // same slot-0 predicate that makes the field editable.
            let has_input = m.menu.menu_slot(0).is_some();
            out.push((
                a::ANVIL_TEXT_FIELD + usize::from(!has_input),
                to_blit(rewo_world::menu_screen::ProgressBlit {
                    dx: 59,
                    dy: 20,
                    w: 110,
                    h: 16,
                    sx: 0,
                    sy: 0,
                    src: None,
                }),
            ));
            // Then the field's own cursor and selection, over it.
            out.extend_from_slice(player.anvil_fills);
            // `extractErrorIcon` — an input present and NO result, which is
            // the combination the anvil refused.
            if (has_input || m.menu.menu_slot(1).is_some()) && m.menu.menu_slot(2).is_none() {
                out.push((
                    a::ANVIL_ERROR,
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: 99,
                        dy: 45,
                        w: 28,
                        h: 21,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
        }
        // stonecutter (M93s): the scroller, then one button chrome per visible
        // cell. The result ICONS are not here — they are items, and go through
        // the GUI-item pass with the slots (see `screen_icons`).
        rewo_world::menu_screen::STONECUTTER_MENU_PROTOCOL_ID => {
            use rewo_world::menu_screen as ms;
            let Some(c) = player.cut.filter(|c| c.display) else {
                // `displayRecipes` hides the grid AND the scrollbar: the
                // scroller is drawn inside `extractBackground` unconditionally,
                // but `isScrollBarActive` is false, so it is the *disabled*
                // sprite that shows — and vanilla draws it even with no input.
                out.push((
                    a::CUT_SCROLLER + 1,
                    to_blit(cut_scroller_blit(ms::cut_scroller_y(0.0))),
                ));
                return out;
            };
            let active = ms::cut_scroll_active(true, c.recipes.len());
            out.push((
                a::CUT_SCROLLER + usize::from(!active),
                to_blit(cut_scroller_blit(ms::cut_scroller_y(c.scroll_offs))),
            ));
            for pos_index in 0..ms::CUT_PAGE {
                let index = c.start_index + pos_index;
                if index as usize >= c.recipes.len() {
                    break;
                }
                // `extractButtons`' three-way test, in its own order: selected
                // wins over hovered, and the hover box is the ICON's, two
                // pixels below the box a click uses.
                let hovered = mouse_gui.is_some_and(|(x, y)| {
                    ms::cut_cell_highlight_at(x, y, c.start_index, c.recipes.len()) == Some(index)
                });
                let chrome = if index == c.selected {
                    0
                } else if hovered {
                    1
                } else {
                    2
                };
                let (sx, sy) = ms::cut_cell_sprite_origin(pos_index);
                out.push((
                    a::CUT_RECIPE_CHROME + chrome,
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: sx,
                        dy: sy,
                        w: ms::CUT_CELL_W,
                        h: ms::CUT_CELL_H,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
        }
        // crafter_3x3 (M93j): the redstone arrow, then one cover per disabled
        // grid slot. Order matters only in that the covers must not be hidden
        // by the arrow, and they do not overlap it.
        rewo_world::menu::CRAFTER_MENU_PROTOCOL_ID => {
            out.push((
                a::CRAFTER_REDSTONE + usize::from(m.crafter_powered()),
                to_blit(rewo_world::menu_screen::crafter_redstone()),
            ));
            for slot in 0..rewo_world::menu::CRAFTER_GRID_SLOTS {
                if !m.crafter_slot_disabled(slot) {
                    continue;
                }
                // The slot's position comes from the LAYOUT, not re-derived
                // from `26 + x * 18`: a second copy of the grid arithmetic is
                // the drift M90's `slot_kind` bug was made of.
                let Some((sx, sy)) = layout.position(slot as usize) else {
                    continue;
                };
                out.push((
                    a::CRAFTER_DISABLED_SLOT,
                    to_blit(rewo_world::menu_screen::crafter_disabled_cover(
                        sx as i32, sy as i32,
                    )),
                ));
            }
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
        // enchantment (M92): each row's background, then its numeral ON TOP of
        // it — which is why `overlays` is an ordered list and not a set.
        13 => {
            let rows = enchant_rows_of(layout, Some(m), player, mouse_gui)
                .expect("id 13 with an open menu");
            for (i, row) in rows.into_iter().enumerate() {
                let (bg, numeral) = enchant_row_sprites(i, row);
                out.push((bg, to_blit(rewo_world::menu_screen::enchant_row_rect(i))));
                if let Some(n) = numeral {
                    out.push((n, to_blit(rewo_world::menu_screen::enchant_level_rect(i))));
                }
            }
        }
        // beacon (M92): each button's 22x22 chrome, then its 18x18 icon.
        9 => {
            let choice = player
                .beacon_override
                .unwrap_or_else(|| beacon_choice(m, &player.beacon_effects));
            for b in rewo_world::menu_screen::beacon_buttons() {
                let hovered = mouse_gui
                    .is_some_and(|(x, y)| rewo_world::menu_screen::beacon_button_hovered(b, x, y));
                let state = rewo_world::menu_screen::beacon_button_state(b, choice, hovered);
                if state == rewo_world::menu_screen::BeaconButtonState::Hidden {
                    continue;
                }
                out.push((
                    a::BEACON_BUTTON_CHROME + beacon_chrome_index(state),
                    to_blit(rewo_world::menu_screen::beacon_button_rect(b)),
                ));
                if let Some(icon) = beacon_icon_sprite(b, choice) {
                    out.push((icon, to_blit(rewo_world::menu_screen::beacon_icon_rect(b))));
                }
            }
        }
        _ => {}
    }
    out
}

/// A button state's offset into the four chrome sprites, which are listed in
/// the order `extractContents` tests for them.
fn beacon_chrome_index(s: rewo_world::menu_screen::BeaconButtonState) -> usize {
    use rewo_world::menu_screen::BeaconButtonState as S;
    match s {
        S::Disabled => 0,
        S::Selected => 1,
        S::Highlighted => 2,
        S::Normal => 3,
        // Never reached: a hidden button is skipped before it gets here.
        S::Hidden => 3,
    }
}

/// The 18x18 icon a beacon button draws inside its chrome.
///
/// The upgrade button **borrows the primary's effect**, so its icon changes as
/// you click elsewhere — it is the one button whose art is not a constant.
fn beacon_icon_sprite(
    b: rewo_world::menu_screen::BeaconButton,
    choice: rewo_world::menu_screen::BeaconChoice,
) -> Option<usize> {
    use rewo_data::assets as a;
    use rewo_world::menu_screen::{beacon_upgrade_effect, BeaconButtonKind, BeaconEffect};
    let effect = match b.kind {
        BeaconButtonKind::Power { effect, .. } => effect,
        BeaconButtonKind::Upgrade => beacon_upgrade_effect(choice)?,
        BeaconButtonKind::Confirm => return Some(a::BEACON_CONFIRM),
        BeaconButtonKind::Cancel => return Some(a::BEACON_CANCEL),
    };
    let i = BeaconEffect::ALL.iter().position(|e| *e == effect)?;
    Some(a::BEACON_EFFECT_ICON + i)
}

/// The two enchanting-table inputs that are the *player's* rather than the
/// menu's (M92).
///
/// A named pair rather than two loose parameters because they are only ever
/// read together and swapping a level for a flag would compile.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EnchantPlayer<'a> {
    /// `player.experienceLevel`, from `set_experience` (M79).
    pub xp_level: i32,
    /// `player.hasInfiniteMaterials()` — `abilities.instabuild` (M75).
    pub creative: bool,
    /// The beacon's six effect ids (M92c). Carried here rather than passed
    /// separately because both screens' extra inputs travel the same seam.
    pub beacon_effects: BeaconEffectIds,
    /// The loom's pattern grid (M93q), when a loom is open.
    pub loom: Option<LoomView>,
    /// The stonecutter's recipe grid (M93s), when a stonecutter is open.
    ///
    /// A reference, not a value: the visible list is a `Vec` built per frame
    /// from `selectByInput`, and this struct is `Copy` because the enchanting
    /// rows and the panel each take it by value.
    pub cut: Option<&'a CutView>,
    /// The anvil field's cursor and selection quads (M93t), already measured.
    pub anvil_fills: &'a [(usize, rewo_gpu::container::PanelBlit)],
    /// The merchant's trade list (M93u), when a merchant is open.
    pub merchant: Option<&'a MerchantView>,
    /// The beacon SCREEN's own choice (M93m), when a screen is driving.
    ///
    /// `None` re-reads the menu's data slots, which is right for a gate with
    /// no screen state — and wrong for the live client, where a click moves
    /// the choice and only `set_beacon` on confirm tells the server. Without
    /// this the render would keep painting the server's last word and a click
    /// would light nothing, which is M93i's "correct on the wire, invisible on
    /// screen" one screen over.
    pub beacon_override: Option<rewo_world::menu_screen::BeaconChoice>,
    /// M103 — the ghost recipe's two washes, already measured. Resolved by the
    /// caller for the reason the beacon's choice and the anvil's fills are: the
    /// slot positions need the layout, and `apply_screen` holds no ScreenState.
    pub ghost_under: &'a [(usize, rewo_gpu::container::PanelBlit)],
    pub ghost_over: &'a [(usize, rewo_gpu::container::PanelBlit)],
}

/// Which of `RecipeBookSettings`' four `TypeSettings` a menu reads (M94).
///
/// **Only four menus have a book at all** — `RecipeBookMenu` is abstract and
/// exactly three concrete classes implement `getRecipeBookType`: the player's
/// own `InventoryMenu` and `CraftingMenu` (both CRAFTING) and
/// `AbstractFurnaceMenu`, which returns the type its subclass was built with.
/// Every other screen returns `None` here and draws no book, which is why a
/// chest is unaffected by any of this.
fn book_type_of(
    layout: &rewo_world::menu_layout::MenuLayout,
) -> Option<rewo_world::recipe_book_screen::BookType> {
    use rewo_world::recipe_book_screen::BookType;
    // Keyed on the registry NAME, not the protocol id: the id is the server's
    // and a version bump renumbers it, while these four names are what the
    // decompile's class names correspond to. (M94's gate learned the hard way
    // that 13 is `enchantment`, not `crafting`.)
    if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID {
        // The player's own inventory IS a `RecipeBookMenu`: `InventoryMenu`
        // returns CRAFTING. It has no menu registry id because it is never
        // opened by `open_screen`.
        return Some(BookType::Crafting);
    }
    match layout.name {
        "crafting" => Some(BookType::Crafting),
        "furnace" => Some(BookType::Furnace),
        "blast_furnace" => Some(BookType::BlastFurnace),
        "smoker" => Some(BookType::Smoker),
        _ => None,
    }
}

/// An item's `getMaxStackSize`, for `accountStack`'s `min` (M96).
///
/// Falls back to 64, which is the default `Item.Properties` value and so the
/// right answer for the 1,242 of 1,537 items that do not override it.
fn max_stack_of(items: &rewo_data::items::Items, id: i32) -> i32 {
    items
        .name(id)
        .map_or(rewo_data::item_props_table::DEFAULT_MAX_STACK, |n| {
            rewo_data::item_props_table::max_stack_size(n)
        })
}

/// Everything the recipe book's chrome needs for one frame (M94).
#[derive(Debug, Clone, Default)]
pub(crate) struct BookRender {
    pub view: Option<rewo_world::recipe_book_screen::BookView>,
    /// `(hasCraftable, hasMultipleRecipes)` for each collection on the page.
    pub slots: Vec<(bool, bool)>,
    pub hover: rewo_world::recipe_book_screen::BookHover,
    /// Which book this is, which decides its tab list and its filter art
    /// (M95). M94 assumed four tabs for every book; a crafting book has FIVE.
    pub book: rewo_world::recipe_book_screen::BookType,
    /// The item each visible slot shows this frame, already cycled — `None`
    /// for a collection whose result needs a context Rewo has not got.
    pub slot_items: Vec<Option<i32>>,
    /// Per visible slot: several recipes AND one shared result display, the
    /// pair of conditions that draws the shadow copy.
    pub slot_shadowed: Vec<bool>,
    /// The recipe id each visible slot would place if clicked (M98) — the one
    /// the display cycle is on, not the collection's first.
    pub slot_recipes: Vec<Option<i32>>,
    /// Per visible slot, its whole collection resolved into overlay buttons —
    /// **in the collection's own order**, not the overlay's (M104).
    ///
    /// The promotion to craftable-first happens when the overlay is opened,
    /// because that is when vanilla does it and because the overlay is a
    /// snapshot: re-promoting each frame would re-sort an open overlay under
    /// the cursor. See `rewo_world::recipe_overlay::Open`.
    pub slot_collections: Vec<Vec<rewo_world::recipe_overlay::Button>>,
}

/// The recipe book for the menu currently on screen (M94), or `None` when it
/// is shut — which is also what keeps the menu centred.
///
/// # What this does not do yet
///
/// The selected tab and the current page are **client** state that only a
/// click can change, and nothing can click the book yet, so both are pinned to
/// 0 and the tab column renders with the first tab selected. `hasCraftable` is
/// answered as of M96 — see `held` below for the one input it still misses.
/// `RecipeBookComponent.isVisible()` — whether the book is showing.
///
/// **One predicate, consulted by the render's placement and by every hover.**
/// `ScreenState::hovered` had its own conversion through `Placement::centred`
/// and so ignored the book entirely; M89 and M106b each fixed one consumer of
/// this same question and each recorded that a per-call-site choice is how
/// they come to disagree.
///
/// It is the **settings flag for the open menu's book type**, which is what
/// `initVisuals` sets `this.visible` from. [`live_recipe_book`] additionally
/// requires the display registries, so `book.is_some()` is narrower in
/// principle — and identical in practice, because those come from the bake at
/// session setup rather than off the wire, so they are present before any
/// menu can open. A menu with no book at all answers `false`.
pub(crate) fn book_visible(session: &rewo_net::play::PlaySession) -> bool {
    let layout = session
        .menus
        .open()
        .map(|m| m.menu.layout())
        .unwrap_or_else(|| session.inventory.layout());
    book_visible_for(book_type_of(layout), &session.recipe_book_settings)
}

/// [`book_visible`]'s rule, over plain values.
///
/// Lifted out because `PlaySession` owns a socket and cannot be constructed in
/// a test — M71's finding, and M97's fix. A mutation making a bookless menu
/// answer `true` survived the whole suite while this lived inside the
/// session-taking function, and a bookless menu answering `true` would
/// displace a chest's panel by 77 px and suppress its hover on a narrow
/// window.
pub(crate) fn book_visible_for(
    book: Option<rewo_world::recipe_book_screen::BookType>,
    settings: &rewo_net::recipe_book::BookSettings,
) -> bool {
    use rewo_world::recipe_book_screen as rb;
    match book {
        Some(rb::BookType::Crafting) => settings.crafting.open,
        Some(rb::BookType::Furnace) => settings.furnace.open,
        Some(rb::BookType::BlastFurnace) => settings.blast_furnace.open,
        Some(rb::BookType::Smoker) => settings.smoker.open,
        // A menu with no book — a chest, a beacon — is never displaced.
        None => false,
    }
}

fn live_recipe_book(
    session: &rewo_net::play::PlaySession,
    items: &rewo_data::items::Items,
    state: rewo_world::recipe_book_screen::BookState,
    // M98 — the cursor in BOOK coordinates, or `None` when it is not being
    // tracked. Book space rather than screen: the conversion needs the window
    // size, and doing it once at the caller keeps the hover and the press
    // reading the same number.
    book_mouse: Option<(i32, i32)>,
    // M99 — the search field's contents, already lowercased, and the item
    // display-name map the search indexes.
    query: &str,
    display: &std::collections::HashMap<String, String>,
) -> Option<BookRender> {
    use rewo_world::recipe_book_screen as rb;
    let layout = session
        .menus
        .open()
        .map(|m| m.menu.layout())
        .unwrap_or_else(|| session.inventory.layout());
    let book = book_type_of(layout)?;
    let st = match book {
        rb::BookType::Crafting => session.recipe_book_settings.crafting,
        rb::BookType::Furnace => session.recipe_book_settings.furnace,
        rb::BookType::BlastFurnace => session.recipe_book_settings.blast_furnace,
        rb::BookType::Smoker => session.recipe_book_settings.smoker,
    };
    if !st.open {
        return None;
    }
    let names = session.recipe_display_ids.as_ref()?;
    let entries: Vec<BookEntry<'_>> = session
        .recipe_book
        .values()
        .filter_map(|e| {
            Some(BookEntry {
                id: e.id,
                group: e.group,
                category: names.category.name(e.category)?,
                results: e.display.result().items(),
                // M96 — the ingredient slots, tags resolved against the
                // server's own `update_tags` payload.
                ingredients: e.ingredients(&session.tags),
                // M99 — what the search indexes.
                search: search_entry_of(&e.display.result().items(), items, display),
                // M104 — the which-of-these overlay's ingredient grid.
                shape: e.display.overlay_shape(),
                grid_items: e.display.overlay_ingredients(),
            })
        })
        .collect();
    // What the player is holding, for `hasCraftable` — see
    // [`crafting_contents`], which is where the rules live so a test can reach
    // them (M97's lesson, fourth application).
    let mut held = crafting_contents(
        &session.inventory,
        session.menus.open().map(|m| &m.menu),
        book,
        &|id| max_stack_of(items, id),
    );
    // `Mth.floor(time / 30)` — vanilla's `time` advances by the partial tick
    // each render, so this is the session's tick clock divided by the swap
    // period. Shared by every slot, which is why a page of several-recipe
    // collections flips together rather than each on its own phase.
    let cycle = (session.ticks as f32 / rewo_net::recipe_book::TICKS_TO_SWAP_SLOT).floor() as i32;
    Some(book_render_from(
        book,
        st.filtering,
        &entries,
        &mut held,
        cycle,
        state,
        book_mouse,
        query,
    ))
}

/// The ghost recipe's two washes, split into the halves they belong in (M103).
///
/// Returns `(under, over)`. Vanilla's order per slot is fill, item, fill — so
/// the red goes with the panel's overlays and the white goes after the icons,
/// which is why they cannot be one list.
pub(crate) fn ghost_washes(
    ghosts: &[rewo_world::ghost_slots::Ghost],
    layout: &rewo_world::menu_layout::MenuLayout,
    player_inventory: bool,
) -> (
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
) {
    use rewo_world::ghost_slots as gs;
    let big = gs::big_result_slot(player_inventory);
    let argb = |v: u32| {
        [
            ((v >> 16) & 255) as f32 / 255.0,
            ((v >> 8) & 255) as f32 / 255.0,
            (v & 255) as f32 / 255.0,
            ((v >> 24) & 255) as f32 / 255.0,
        ]
    };
    let mut under = Vec::new();
    let mut over = Vec::new();
    for g in ghosts {
        let Some((sx, sy)) = layout.position(g.slot) else { continue };
        let (dx, dy, size) = gs::wash_rect(g.is_result, big);
        let fill = |tint: [f32; 4], dx: i32, dy: i32, size: i32| {
            (
                rewo_gpu::container::FILL_SPRITE,
                rewo_gpu::container::PanelBlit {
                    dx: (sx as i32 + dx) as f32,
                    dy: (sy as i32 + dy) as f32,
                    w: size as f32,
                    h: size as f32,
                    sx: 0.0,
                    sy: 0.0,
                    sw: 0.0,
                    sh: 0.0,
                    tint,
                },
            )
        };
        under.push(fill(argb(gs::WASH_UNDER), dx, dy, size));
        // The veil is always the plain 16x16 at the slot: only the wash BELOW
        // widens for a big result slot. Widening both would ring the icon in
        // white.
        over.push(fill(argb(gs::WASH_OVER), 0, 0, 16));
    }
    (under, over)
}

/// The ghost recipe for the shown menu, from `place_ghost_recipe` (M103).
///
/// M93y decoded that packet into `PlaySession::ghost_recipe` and nothing
/// consumed it — it is the only decoded-but-unrendered packet left in this
/// area. `handlePlaceRecipe` is gated on the container id matching the open
/// menu, so a ghost for a screen you have since closed is dropped rather than
/// drawn over whatever replaced it.
pub(crate) fn live_ghosts(
    session: &rewo_net::play::PlaySession,
) -> Vec<rewo_world::ghost_slots::Ghost> {
    use rewo_net::recipe_book::RecipeDisplay as D;
    use rewo_world::ghost_slots as gs;
    let Some((container, display)) = session.ghost_recipe.as_ref() else {
        return Vec::new();
    };
    // The ghost belongs to a container id; the shown menu has one too, and a
    // mismatch means the ghost is stale.
    if *container != session.shown_container_id() {
        return Vec::new();
    }
    let layout = session.shown_menu().layout();
    let book = match book_type_of(layout) {
        Some(b) => b,
        None => return Vec::new(),
    };
    let items = |d: &rewo_net::recipe_book::SlotDisplay| d.items();
    let (menu, inputs, shape) = match display {
        D::CraftingShaped { width, height, ingredients, .. } => (
            // `getGridWidth/Height` — 2x2 for the player's own inventory, 3x3
            // for a crafting table.
            gs::crafting_menu(
                if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID { 2 } else { 3 },
                if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID { 2 } else { 3 },
            ),
            ingredients.iter().map(items).collect::<Vec<_>>(),
            Some((*width as usize, *height as usize)),
        ),
        D::CraftingShapeless { ingredients, .. } => (
            gs::crafting_menu(
                if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID { 2 } else { 3 },
                if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID { 2 } else { 3 },
            ),
            ingredients.iter().map(items).collect(),
            None,
        ),
        D::Furnace { ingredient, fuel, .. } => (
            gs::FURNACE_MENU,
            vec![items(ingredient), items(fuel)],
            None,
        ),
        // A stonecutter or smithing display: `fillGhostRecipe`'s switch has no
        // case, so the result alone is ghosted — and neither screen has a book,
        // so this is unreachable in practice and transcribed anyway.
        _ => (
            gs::GhostMenu { result: 0, grid: None, furnace: None },
            Vec::new(),
            None,
        ),
    };
    let fuel_empty = menu
        .furnace
        .is_some_and(|(_, f)| session.shown_menu().menu_slot(f).is_none());
    let _ = book;
    gs::layout(menu, items(display.result()), &inputs, shape, fuel_empty)
}

/// `fillStackedContents` + `fillCraftSlotsStackedContents` (M102).
///
/// Vanilla calls **two** fills and they are disjoint:
///
/// ```java
/// player.getInventory().fillStackedContents(contents);   // the ITEMS
/// menu.fillCraftSlotsStackedContents(contents);          // the GRID
/// ```
///
/// `Inventory.items` is armour + storage + hotbar + offhand —
/// `PLAYER_ITEM_SLOTS`, **5..46**. It contains neither the 2x2 crafting grid
/// (1..5, which belongs to `InventoryMenu`) nor the craft **result** (slot 0,
/// which belongs to nothing). Walking the whole 46-slot menu is the obvious
/// reading and it double-counts the grid *and* adds the result — so a recipe
/// would read as craftable off its own output.
///
/// The inventory fill is `accountSimpleStack`, hence gated on
/// `isUsableForCrafting`; the craft-slot fill's gating depends on the family
/// (`craft_slots`). M96 named the predicate in a comment and applied nothing.
///
/// Extracted from `live_recipe_book` because that function needs a
/// `PlaySession` and so cannot be reached from a test — a mutation deleting the
/// craft-slot half survived until this split.
pub(crate) fn crafting_contents(
    player: &rewo_world::inventory::Inventory,
    open: Option<&rewo_world::inventory::Inventory>,
    book: rewo_world::recipe_book_screen::BookType,
    // `getMaxStackSize` per item id. A closure rather than the whole registry,
    // so this stays a function of plain values and its tests do not need the
    // user's decompile on disk.
    max_stack: &dyn Fn(i32) -> i32,
) -> rewo_world::stacked_contents::StackedContents {
    use rewo_world::recipe_book_screen as rb;
    let mut held = rewo_world::stacked_contents::StackedContents::new();
    let mut account = |inv: &rewo_world::inventory::Inventory, slot: usize, gated: bool| {
        if let Some(st) = inv.menu_slot(slot) {
            if !gated || inv.is_usable_for_crafting(st) {
                held.account_stack(st.item_id, st.count, max_stack(st.item_id));
            }
        }
    };
    for slot in rb::PLAYER_ITEM_SLOTS {
        account(player, slot, true);
    }
    let shown = open.unwrap_or(player);
    if let Some(cs) = rb::craft_slots(book, open.is_none()) {
        for slot in cs.range.clone() {
            account(shown, slot, cs.gated);
        }
    }
    held
}

/// One unlocked recipe, with everything already resolved (M97).
///
/// The seam between the session and the derivation: the session half is
/// lookups, the derivation half is the grouping, paging, cycling and craftable
/// arithmetic — and only the second half is worth grading, which is why it is
/// on this side of the line.
pub(crate) struct BookEntry<'a> {
    pub id: i32,
    pub group: Option<i32>,
    pub category: &'a str,
    /// The result display's item ids, in order.
    pub results: Vec<i32>,
    /// `None` when `craftingRequirements` was absent — never craftable.
    pub ingredients: Option<Vec<rewo_world::stacked_contents::Ingredient>>,
    /// The result items' names and ids, lowercased, for the search (M99).
    pub search: rewo_world::recipe_search::SearchEntry,
    /// How the which-of-these overlay lays this recipe's ingredients out, and
    /// what each of them resolves to (M104). Read together: the shape decides
    /// where ingredient `n` goes, the list decides what it shows.
    pub shape: rewo_world::recipe_overlay::Shape,
    pub grid_items: Vec<Vec<i32>>,
}

/// What the search indexes for one recipe's result items (M99).
///
/// `getTooltipLines` over the results, plus their registry keys. For Rewo the
/// tooltip of a bare item id is its **display name** and nothing else, since
/// every other line comes from a component and a recipe's result carries none —
/// so this is exact rather than an approximation, and stops being exact only
/// if results ever arrive with components.
pub(crate) fn search_entry_of(
    results: &[i32],
    items: &rewo_data::items::Items,
    display: &std::collections::HashMap<String, String>,
) -> rewo_world::recipe_search::SearchEntry {
    let mut out = rewo_world::recipe_search::SearchEntry::default();
    for id in results {
        let Some(name) = items.name(*id) else { continue };
        // A missing translation falls back to the id's own path, prettified —
        // the same fallback the tooltip takes, so a search finds what the
        // tooltip shows.
        let shown = display
            .get(name)
            .cloned()
            .unwrap_or_else(|| display_name_of(name));
        out.names.push(shown.to_lowercase());
        let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
        out.ids.push((ns.to_lowercase(), path.to_lowercase()));
    }
    out
}

/// The book's per-frame state, from resolved inputs (M97).
///
/// Split out of [`live_recipe_book`] because a `PlaySession` owns a socket and
/// cannot be built in a test — M71's lesson: logic in a place with no test
/// module is untestable, so move it. M96 shipped this arithmetic graded only at
/// its two ends (the solver's own tests, and the chrome witness), with the
/// derivation between them untested — the M92/M93b shape.
pub(crate) fn book_render_from(
    book: rewo_world::recipe_book_screen::BookType,
    filtering: bool,
    entries: &[BookEntry<'_>],
    held: &mut rewo_world::stacked_contents::StackedContents,
    cycle: i32,
    // M98 — the tab and page a click chose. Clamped here rather than trusted:
    // a tab index survives a book-type change (a furnace book has fewer tabs
    // than a crafting one) and a page survives the list shrinking under it.
    state: rewo_world::recipe_book_screen::BookState,
    // The cursor in book coordinates, for the arrows' and filter's hover art.
    book_mouse: Option<(i32, i32)>,
    // The search field's contents, already lowercased (M99).
    query: &str,
) -> BookRender {
    use rewo_world::recipe_book_screen as rb;
    let flat: Vec<(i32, Option<i32>, &str)> =
        entries.iter().map(|e| (e.id, e.group, e.category)).collect();
    let all = rb::collections(&flat);
    let tabs = book.tabs();
    let selected_tab = state.selected_tab.min(tabs.len() - 1);
    // Stage one of `updateCollections` is the tab's own membership. Tab 0 is
    // the SEARCH tab, whose categories are the book's whole set — so with the
    // selection pinned to 0 this shows everything the book has.
    let wanted = tabs[selected_tab].categories;
    // `updateCollections`' stages, in order (M93z): the tab's membership, then
    // the SEARCH, then the filter. The search stage is skipped entirely on an
    // empty query rather than run with one — see `recipe_search::matches`.
    let mine: Vec<_> = all
        .iter()
        .filter(|c| wanted.contains(&c.category.as_str()))
        .filter(|c| {
            // A collection's searchable text is the union of its recipes',
            // which is what `flatMap` over `getRecipes()` gives.
            let mut e = rewo_world::recipe_search::SearchEntry::default();
            for id in &c.recipes {
                if let Some(src) = entries.iter().find(|x| x.id == *id) {
                    e.names.extend(src.search.names.iter().cloned());
                    e.ids.extend(src.search.ids.iter().cloned());
                }
            }
            rewo_world::recipe_search::matches(&e, query)
        })
        .collect();
    let total_pages = rb::total_pages(mine.len());
    let page = rb::clamp_page(state.page, mine.len(), false);
    let range = rb::page_range(page, mine.len());
    let find = |id: &i32| entries.iter().find(|e| e.id == *id);
    let mut slots = Vec::new();
    let mut slot_items = Vec::new();
    let mut slot_shadowed = Vec::new();
    let mut slot_recipes = Vec::new();
    let mut slot_collections = Vec::new();
    for c in &mine[range] {
        let per_entry: Vec<Vec<i32>> = c
            .recipes
            .iter()
            .map(|id| find(id).map(|e| e.results.clone()).unwrap_or_default())
            .collect();
        let multiple = per_entry.len() > 1;
        // `allRecipesHaveSameResultDisplay` — every display item across every
        // entry the same. Rewo compares item IDS, where vanilla compares whole
        // stacks with `isSameItemSameComponents`; two recipes yielding the same
        // item with different components would shadow here and not in vanilla.
        let mut seen = per_entry.iter().flatten();
        let first = seen.next().copied();
        let same_result = seen.all(|i| Some(*i) == first);
        // Per-recipe affordability, which the which-of-these overlay needs one
        // by one (M104) where the cell needs only whether ANY of them is.
        // A recipe whose `craftingRequirements` were absent is never craftable,
        // which is `canCraft`'s own opening line rather than a default.
        let per_recipe: Vec<bool> = c
            .recipes
            .iter()
            .map(|id| {
                find(id)
                    .and_then(|e| e.ingredients.as_ref())
                    .is_some_and(|ing| held.try_pick(ing, 1))
            })
            .collect();
        // `hasCraftable()` — ANY of them.
        let craftable = per_recipe.iter().any(|&c| c);
        slot_collections.push(
            c.recipes
                .iter()
                .zip(&per_recipe)
                .map(|(&id, &can)| {
                    let (shape, grid) = find(&id)
                        .map(|e| (e.shape, e.grid_items.clone()))
                        .unwrap_or((rewo_world::recipe_overlay::Shape::Other, Vec::new()));
                    rewo_world::recipe_overlay::Button {
                        recipe: id,
                        craftable: can,
                        // `if (!items.isEmpty())` — an ingredient that resolves
                        // to nothing contributes no position, and because
                        // neither placement arm derives its position from a
                        // running counter, dropping one does not shift the rest.
                        slots: rewo_world::recipe_overlay::grid_positions(
                            book.furnace_family(),
                            shape,
                        )
                        .into_iter()
                        .filter_map(|p| {
                            let items = grid.get(p.ingredient).cloned().unwrap_or_default();
                            (!items.is_empty()).then_some((p, items))
                        })
                        .collect(),
                    }
                })
                .collect(),
        );
        slots.push((craftable, multiple));
        slot_items.push(rewo_net::recipe_book::display_item(&per_entry, cycle));
        slot_shadowed.push(multiple && same_result);
        // `getCurrentRecipe` — the entry the cycle is showing, which is what a
        // left-click places. Not the collection's first: clicking while the
        // cycle is on the second of two recipes places the SECOND.
        let n = c.recipes.len() as i32;
        slot_recipes.push(if n == 0 {
            None
        } else {
            c.recipes
                .get((cycle - n * cycle.div_euclid(n)) as usize)
                .copied()
        });
    }
    let view = rb::BookView {
        tabs: tabs.len(),
        selected_tab,
        page,
        total_pages,
        shown: slots.len(),
        filtering,
        furnace_family: book.furnace_family(),
    };
    let hover = match book_mouse.and_then(|(bx, by)| rb::book_hit(bx, by, view, tabs.len())) {
        Some(rb::BookHit::PageForward) => rb::BookHover { page_forward: true, ..Default::default() },
        Some(rb::BookHit::PageBackward) => {
            rb::BookHover { page_backward: true, ..Default::default() }
        }
        Some(rb::BookHit::Filter) => rb::BookHover { filter: true, ..Default::default() },
        _ => rb::BookHover::default(),
    };
    BookRender {
        view: Some(rb::BookView {
            tabs: tabs.len(),
            selected_tab,
            page,
            total_pages,
            shown: slots.len(),
            filtering,
            furnace_family: book.furnace_family(),
        }),
        slots,
        // M98 — from the SAME `book_hit` the press uses, so what lights up is
        // what a click would take. Deriving the hover from its own rects would
        // be two chances to disagree.
        hover,
        book,
        slot_items,
        slot_shadowed,
        slot_recipes,
        slot_collections,
    }
}

/// Build the which-of-these overlay a right-click on page cell `index` opens
/// (M104).
///
/// A free function of plain values rather than a method reaching into the
/// session, so a test can drive it — M97's lesson, and M92's finding one layer
/// on: the arithmetic between "a collection" and "a placed popup" is exactly
/// what neither the model's tests nor a pixel gate can see on its own.
///
/// The `centre` it passes is the one `RecipeBookPage.mouseClicked` passes, and
/// its `+ 13` on the vertical half is *inert* — see
/// `recipe_overlay::origin`'s tests for the proof, and note the constant is
/// still written out rather than dropped.
pub(crate) fn open_overlay(
    collection: Vec<rewo_world::recipe_overlay::Button>,
    index: usize,
    view: rewo_world::recipe_book_screen::BookView,
) -> rewo_world::recipe_overlay::Open {
    use rewo_world::recipe_book_screen as rb;
    use rewo_world::recipe_overlay as ro;
    // Craftable first, and the flag rides along rather than being recomputed:
    // the overlay is a snapshot, so its ordering and its greying are fixed at
    // this moment together.
    let pairs: Vec<(ro::Button, bool)> = collection
        .into_iter()
        .map(|b| {
            let c = b.craftable;
            (b, c)
        })
        .collect();
    let buttons: Vec<ro::Button> =
        ro::promote(&pairs, view.filtering).into_iter().map(|(b, _)| b).collect();
    ro::Open {
        origin: ro::origin(
            rb::grid_slot(index),
            buttons.len(),
            (rb::IMAGE_W / 2, 13 + rb::IMAGE_H / 2),
            ro::BUTTON_PITCH as f32,
        ),
        furnace: view.furnace_family,
        buttons,
    }
}

/// The anvil's name field, as `AnvilScreen.init` builds it (M101).
///
/// `setCanLoseFocus(false)` and `setInitialFocus(this.name)` — focused from the
/// moment the screen opens and unable to lose it, so its caret blinks for as
/// long as the screen is up.
///
/// A function rather than three lines inline, because those three lines sit
/// inside a path that needs a `PlaySession` and so cannot be reached from a
/// test — M93t wrote the comment and did only the first half, and nothing
/// caught it for eight milestones. Here the claim is one call to something
/// graded below.
fn anvil_field_new() -> rewo_world::edit_box::EditBox {
    let mut field = rewo_world::edit_box::EditBox::new(rewo_world::anvil::MAX_NAME_LENGTH);
    field.set_can_lose_focus(false);
    field.set_focused(true);
    field
}

/// `scrollTo(cursorPos)` for the book's field (M101).
///
/// Vanilla folds this into `setCursorPosition`; Rewo cannot, because the width
/// function is the caller's — so every input path that moves the cursor calls
/// it. Missing it leaves a field typed past its visible width showing the head
/// of the string with no caret at all.
fn follow_cursor(
    field: &mut rewo_world::edit_box::EditBox,
    baked: Option<&assets::BakedAssets>,
    inner_width: i32,
) {
    let Some(font) = baked.and_then(|b| b.font.as_ref()) else {
        return;
    };
    let advance = font.advance;
    let width = move |u: &[u16]| rewo_gpu::text::width(&String::from_utf16_lossy(u), &advance);
    field.follow_cursor(inner_width, &width);
}

/// The search field's background, text, caret and selection (M100).
///
/// # The nine-slice degenerates to two blits
///
/// `widget/text_field` is 200x20 with `border: 1` — but it is a **1-bit
/// paletted** image of exactly two colours: the border 160-grey (white when
/// focused) and the interior black, both fully opaque, measured out of the PNG.
/// Every one of the nine regions is therefore uniform, and a stretched 1x1
/// source is **pixel-identical** to a tiled one.
///
/// So: one blit of the whole rect sampling a border texel, then one of the
/// interior sampling a centre texel. The 1 px the first blit still shows around
/// the second *is* the border. Nine blits would draw the same pixels; two is
/// what the measurement licenses.
fn book_field_render(
    field: &rewo_world::edit_box::EditBox,
    advance: &[u8; 256],
    w: f32,
    h: f32,
    now_ms: u64,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
) {
    use rewo_data::assets as a;
    use rewo_world::recipe_book_screen as rb;
    let origin = rewo_gpu::container::recipe_book_origin(w, h);
    // `SPRITES.get(isActive(), isFocused())` — the one use of that record on
    // this screen that means what its argument names say.
    let sprite = a::BOOK_SEARCH_FIELD + usize::from(rb::search_sprite_focused(field.is_focused()));
    let quad = |dx: i32, dy: i32, w: i32, h: i32, sx: f32, sy: f32| {
        (
            sprite,
            rewo_gpu::container::PanelBlit {
                dx: dx as f32,
                dy: dy as f32,
                w: w as f32,
                h: h as f32,
                sx,
                sy,
                // A 1x1 source, stretched — exact because the region is
                // uniform. See the note above.
                sw: 1.0,
                sh: 1.0,
                tint: [1.0; 4],
            },
        )
    };
    let mut fills = vec![
        // The border colour over the whole rect…
        quad(rb::SEARCH_X, rb::SEARCH_Y, rb::SEARCH_W, rb::SEARCH_H, 0.0, 0.0),
        // …then the interior, one pixel in on every side.
        quad(
            rb::SEARCH_X + 1,
            rb::SEARCH_Y + 1,
            rb::SEARCH_W - 2,
            rb::SEARCH_H - 2,
            100.0,
            10.0,
        ),
    ];
    let (labels, text_fills, _) = edit_box_render(
        field,
        advance,
        origin,
        (rb::SEARCH_TEXT_X, rb::SEARCH_TEXT_Y, rb::SEARCH_INNER_W),
        Some((rb::SEARCH_HINT, rb::SEARCH_HINT_COLOR)),
        now_ms,
    );
    fills.extend(text_fills);
    (labels, fills)
}

/// The GUI-space top-left of the slot the cursor is over, for the highlight
/// (M106b).
///
/// **Through the same [`rewo_gpu::container::Placement`] the pass draws with.**
/// This was a bare `screen_to_gui_for`, which centres, while
/// `ContainerPass::set_state` resolves its own origin with
/// `Placement::with_book` — so with the recipe book open the cursor was
/// converted against a panel 77 GUI px left of the one the highlight was drawn
/// against, and the lit slot sat four columns right of the cursor. The
/// tooltip's conversion had the same bug; the panel, the slot icons
/// ([`menu_slot_rects`]) and the bespoke-widget hovers did not.
///
/// Extracted from `apply_screen` rather than fixed in place because that
/// function needs a `PlaySession` and no gate reaches it: every `set_container`
/// call in the whole app passes `hovered: None`, so the derivation had no
/// witness of any kind. M97's lesson again — move the logic to where a test can
/// see it.
fn hovered_slot_position(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    mouse: (f64, f64),
    w: f32,
    h: f32,
    book_open: bool,
) -> Option<(i32, i32)> {
    let slot = hovered_menu_slot(layout, mouse, w, h, book_open)?;
    layout.position(slot).map(|(x, y)| (x as i32, y as i32))
}

/// `AbstractContainerScreen.getHoveredSlot` — which menu slot the cursor is
/// over, through the book-aware placement.
///
/// **Rewo does not model `AbstractRecipeBookScreen.isHovering`'s narrow-window
/// override**, which returns false for every slot while the book is visible and
/// the window is under 379 GUI px — the case where the book covers the menu. In
/// vanilla that makes `hoveredSlot` null there, suppressing the highlight, the
/// item tooltip, the ghost tooltip and the number/Q/F keyboard actions
/// together. It is one predicate with five consumers and belongs in its own
/// change, not smuggled in with a tooltip.
pub(crate) fn hovered_menu_slot(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    mouse: (f64, f64),
    w: f32,
    h: f32,
    book_open: bool,
) -> Option<usize> {
    // `AbstractRecipeBookScreen.isHovering` (M112):
    //
    //     return (!this.widthTooNarrow || !this.recipeBookComponent.isVisible())
    //            && super.isHovering(...);
    //
    // On a window under 379 GUI px the book does not sit BESIDE the menu, it
    // sits OVER it — `updateScreenPosition`'s 177 px shift collapses to 0 —
    // so vanilla answers "no slot" for every slot rather than letting a click
    // reach through the panel that is covering it. Without this, a narrow
    // window lets a click on the book land on whatever menu slot happens to be
    // underneath.
    //
    // Placed here rather than at each consumer because it is ONE predicate
    // with several: the highlight, both tooltips, the click, the double-click
    // detector, the drag and the number/Q/F actions all arrive through this
    // function.
    let scale = rewo_gpu::hud::gui_scale(w, h);
    let gui_w = (w / scale) as i32;
    if book_open && rewo_world::recipe_book_screen::width_too_narrow(gui_w) {
        return None;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui_placed(
        mouse,
        w,
        h,
        rewo_gpu::container::Placement::with_book(
            layout.image_w as f32,
            layout.image_h as f32,
            book_open,
        ),
    );
    layout.slot_at(gx, gy)
}

/// [`hovered_menu_slot`] under its own name, so a gate can assert that a
/// witness's cursor really is over the slot it thinks it is.
///
/// b22 first passed for the wrong reason: it compared a book-open cursor
/// against a book-shut conversion, so the `None` it was reading came from the
/// 77 px placement shift and not from the guard it names. A mutation deleting
/// that guard survived. This exists so the witness can say which of the two it
/// is measuring.
pub(crate) fn hovered_menu_slot_for_gate(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    mouse: (f64, f64),
    w: f32,
    h: f32,
    book_open: bool,
) -> Option<usize> {
    hovered_menu_slot(layout, mouse, w, h, book_open)
}

/// Which of a frame's three tooltip producers wins (M106c).
///
/// `setTooltipForNextFrameInternal` is
/// `if (this.deferredTooltip == null || replaceExisting)`, and `replaceExisting`
/// is false on every path any of these three takes — so the **first** tooltip
/// set in a frame wins, and the calls that follow it are discarded.
///
/// That inverts the reading of `AbstractRecipeBookScreen.extractRenderState`,
/// which calls the container's `extractTooltip` and *then* the book's, as
/// though the book overwrote it. Writing this as a named function rather than
/// an `or_else` chain at the call site is the point: the parameters say which
/// producer is which, so getting the order wrong is a named mistake instead of
/// an anonymous one, and the rule has somewhere to be tested.
///
/// The producers are closures because two of the three are only worth
/// evaluating when the earlier ones declined — the container's tooltip walks a
/// component patch and the book's measures a page.
///
/// `ctx` is threaded through them rather than captured because all three want
/// the same `&mut GlyphCache`, and three closures cannot hold it at once. The
/// reborrow per call is what makes the laziness expressible at all.
fn frame_tooltip<T, C>(
    ctx: &mut C,
    menu: impl FnOnce(&mut C) -> Option<T>,
    book_page: impl FnOnce(&mut C) -> Option<T>,
    ghost: impl FnOnce(&mut C) -> Option<T>,
) -> Option<T> {
    if let Some(t) = menu(ctx) {
        return Some(t);
    }
    if let Some(t) = book_page(ctx) {
        return Some(t);
    }
    ghost(ctx)
}

/// The tooltip of a GHOST ingredient under the cursor (M106c).
///
/// `GhostSlots.extractTooltip` — keyed on the hovered menu slot, showing the
/// item the ghost's own cycle is on.
///
/// **This is where first-wins becomes observable.** The container's tooltip is
/// set earlier in the frame and `setTooltipForNextFrameInternal` only assigns
/// when nothing has claimed the slot, so a ghost drawn over a slot that already
/// holds an item describes the REAL item, not the ghost. Ordering these
/// producers the way their calls read — the book's last, therefore winning —
/// would show the ghost's name over a filled slot, which is plausible and
/// wrong.
///
/// **Gated on the book being open**, unlike the ghost's own render:
/// `extractGhostRecipe` is called unconditionally from `extractSlots` while
/// `extractTooltip` sits inside `if (this.isVisible())`. So shutting the book
/// leaves the ghost painted and stops it describing itself.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ghost_tooltip(
    ghosts: &[rewo_world::ghost_slots::Ghost],
    cycle: i32,
    layout: &'static rewo_world::menu_layout::MenuLayout,
    book_open: bool,
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
    advance: &[u8; 256],
    glyphs: Option<&mut rewo_gpu::velvet_glyph::GlyphCache>,
    mouse: (f64, f64),
    (w, h): (f32, f32),
) -> Option<(
    rewo_gpu::container::TooltipDraw,
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
)> {
    if !book_open {
        return None;
    }
    let slot = hovered_menu_slot(layout, mouse, w, h, book_open)?;
    // `this.ingredients.get(hoveredSlot)` — a map keyed by the slot, so a
    // hovered slot with no ghost is simply absent rather than a miss to
    // recover from.
    let ghost = ghosts.iter().find(|g| g.slot == slot)?;
    let item = ghost.item(cycle)?;
    let item_name = items.name(item)?;
    let lines = vec![vec![rewo_gpu::tooltip::Span::new(
        names.get(item_name)?.to_string(),
        rarity_color(stack_rarity(Some(item_name), None, false)),
    )]];
    tooltip_layout(lines, advance, glyphs, mouse, (w, h))
}

/// Every label and fill the open book contributes (M105).
///
/// The field's text and the page counter belong to different objects — an
/// `EditBox` on the component, and `RecipeBookPage`'s own `extractRenderState`
/// — but they share both preconditions (an open book, a font to measure with)
/// and they are drawn into one list. Composing them here rather than at the
/// call site is deliberate: `apply_screen` needs a `PlaySession` and so cannot
/// be reached from a test, and a composition step performed there would be
/// deletable with every unit test still green. That is M99's lesson — shrink
/// the untestable surface rather than pretend to cover it.
fn book_labels(
    b: &BookRender,
    field: &rewo_world::edit_box::EditBox,
    lang: &rewo_data::lang::Language,
    advance: &[u8; 256],
    w: f32,
    h: f32,
    now_ms: u64,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
) {
    let (mut labels, fills) = book_field_render(field, advance, w, h, now_ms);
    labels.extend(b.view.and_then(|v| book_page_label(v, lang, advance, w, h)));
    (labels, fills)
}

/// The `x/y` page counter under the recipe grid (M105).
///
/// The one piece of the book vanilla draws as bare text rather than through a
/// widget: `RecipeBookPage.extractRenderState` opens with it, before the
/// buttons and the arrows. Its model constants have existed since M93z with
/// nothing reading them.
///
/// Three details that a call site alone does not carry:
///
/// * **The five-argument `graphics.text` delegates to the six-argument one with
///   `dropShadow = true`.** Transcribing the visible arguments and defaulting
///   the shadow to `false` loses it with nothing to notice.
/// * **Colour `-1` is `0xFFFFFFFF`** — opaque white. `text` also early-returns
///   on `ARGB.alpha(color) == 0`, which is why the alpha is worth reading
///   rather than assuming.
/// * The x is computed **by hand** as `xo - width / 2 + 73` rather than through
///   the `centeredText` helper sitting a few lines away in the same class. The
///   two are arithmetically identical here; the hand-written form is kept
///   because it is what the class does.
fn book_page_label(
    view: rewo_world::recipe_book_screen::BookView,
    lang: &rewo_data::lang::Language,
    advance: &[u8; 256],
    w: f32,
    h: f32,
) -> Option<rewo_gpu::world::OwnedTextLine> {
    use rewo_world::recipe_book_screen as rb;
    // `Language.getOrDefault` returns the key when the map has no entry, and a
    // template with no specifiers survives `decomposeTemplate` unchanged — so
    // `or_key` is vanilla's behaviour, not a local fallback.
    let text = rb::page_label(view.page, view.total_pages, lang.or_key(rb::PAGE_LABEL_KEY))?;
    let (bl, bt, scale) = rewo_gpu::container::recipe_book_origin(w, h);
    // Measured with the SAME advances the text pass will draw with. Measuring
    // one font and drawing another is what M52b found in the tooltip when it
    // moved to Newsreader; here there is only the bitmap font, and passing the
    // table in keeps it that way by construction.
    let width = rewo_gpu::text::width(&text, advance);
    Some(rewo_gpu::world::OwnedTextLine {
        x: bl + rb::page_label_x(width) as f32 * scale,
        y: bt + rb::PAGE_LABEL_Y as f32 * scale,
        px: scale,
        color_linear: [1.0, 1.0, 1.0],
        alpha: 1.0,
        shadow: true,
        style: rewo_gpu::text::TextStyle::PLAIN,
        text,
    })
}

/// The book's chrome as blits, with its semantic sprites resolved to atlas
/// indices.
///
/// The mapping lives here rather than in the model for the reason the model's
/// doc gives: `MENU_OVERLAY_SPRITES` is append-only and its order is an atlas
/// contract, so keeping the geometry free of it lets the atlas grow without
/// touching a geometry file — and lets the model's tests name a *sprite* rather
/// than an index.
pub(crate) fn recipe_book_panel(
    b: &BookRender,
    // M100 — the search field's own quads, appended AFTER the chrome so the
    // field's background sits over the panel and its caret over that.
    field: &[(usize, rewo_gpu::container::PanelBlit)],
    // M104 — the open which-of-these overlay, if any, and the cursor in book
    // pixels for its hover. Drawn LAST, which is `graphics.nextStratum()`.
    open: Option<&rewo_world::recipe_overlay::Open>,
    book_mouse: Option<(i32, i32)>,
) -> Option<rewo_gpu::container::RecipeBookPanel> {
    use rewo_data::assets as a;
    use rewo_world::recipe_book_screen as rb;
    let view = b.view?;
    let mut blits = Vec::new();
    let mut overlays = Vec::new();
    let chrome = rb::book_chrome(view, &b.slots, b.hover)
        .into_iter()
        .chain(open.into_iter().flat_map(|o| {
            rb::overlay_chrome(
                o.origin,
                &o.craftable_flags(),
                o.furnace,
                book_mouse.and_then(|(bx, by)| o.hovered(bx, by)),
            )
        }));
    for q in chrome {
        let (sx, sy, sw, sh) = q.src.unwrap_or((0, 0, q.w, q.h));
        let blit = rewo_gpu::container::PanelBlit {
            dx: q.x as f32,
            dy: q.y as f32,
            w: q.w as f32,
            h: q.h as f32,
            sx: sx as f32,
            sy: sy as f32,
            sw: sw as f32,
            sh: sh as f32,
            tint: [1.0; 4],
        };
        match q.sprite {
            // The panel is the one quad that comes off a background SHEET
            // rather than a sprite, and the one whose source is not (0, 0).
            rb::BookSprite::Panel => blits.push(rewo_gpu::container::PanelBlit {
                sx: rb::PANEL_SOURCE.0 as f32,
                sy: rb::PANEL_SOURCE.1 as f32,
                ..blit
            }),
            rb::BookSprite::Tab { selected } => {
                overlays.push((a::BOOK_TAB + usize::from(selected), blit))
            }
            rb::BookSprite::Slot(s) => overlays.push((
                a::BOOK_SLOT
                    + match s {
                        rb::SlotSprite::Craftable => 0,
                        rb::SlotSprite::ManyCraftable => 1,
                        rb::SlotSprite::Uncraftable => 2,
                        rb::SlotSprite::ManyUncraftable => 3,
                    },
                blit,
            )),
            rb::BookSprite::PageForward { hovered } => {
                overlays.push((a::BOOK_PAGE_ARROW + usize::from(hovered), blit))
            }
            rb::BookSprite::PageBackward { hovered } => {
                overlays.push((a::BOOK_PAGE_ARROW + 2 + usize::from(hovered), blit))
            }
            rb::BookSprite::Filter { furnace, filtering, hovered } => {
                let base = if furnace { a::BOOK_FILTER_FURNACE } else { a::BOOK_FILTER };
                overlays.push((base + rb::filter_sprite_offset(filtering, hovered), blit))
            }
            // M104 — the only sprite in this list that is nine-sliced, so the
            // one whose `src` is not the whole of it.
            rb::BookSprite::OverlayPanel => overlays.push((a::BOOK_OVERLAY_PANEL, blit)),
            rb::BookSprite::OverlayButton { furnace, craftable, hovered } => overlays.push((
                a::BOOK_OVERLAY_BUTTON
                    + if furnace { 4 } else { 0 }
                    + 2 * usize::from(!craftable)
                    + usize::from(hovered),
                blit,
            )),
        }
    }
    overlays.extend_from_slice(field);
    Some(rewo_gpu::container::RecipeBookPanel { blits, overlays })
}

/// [`container_panel`] for `containershot`, which drives the production
/// builder rather than a copy of it — M45's finding: a gate that reimplements
/// a slice of the app's setup misses whatever the app adds to it.
pub(crate) fn container_panel_for_test(
    layout: &'static rewo_world::menu_layout::MenuLayout,
) -> Option<rewo_gpu::container::ContainerPanel> {
    container_panel(layout, None, EnchantPlayer::default(), None)
}

/// [`container_panel`] for an *open* menu, so `containershot` can grade the
/// M92 overlays — which only exist when there are data slots to read.
///
/// Drives the production builder for M45's reason: a gate that reimplements a
/// slice of the app's setup misses whatever the app adds to it.
pub(crate) fn container_panel_for_open_menu(
    open: &rewo_world::menu::OpenMenu,
    xp_level: i32,
    creative: bool,
    beacon_effects: BeaconEffectIds,
    mouse_gui: Option<(f64, f64)>,
    // M93m — the beacon screen's own choice. Carried on the SAME entry point
    // the gate already drives rather than a second one, so `containershot`
    // cannot exercise a path the live client does not take (M45).
    beacon_override: Option<rewo_world::menu_screen::BeaconChoice>,
    // M93q — the loom's grid. Carried for the same reason as the beacon's
    // choice, and needed for the same reason: with it hardcoded `None` the
    // gate could not reach `menu_overlays`' loom arm at all, so M93q's fill
    // was witnessed as a *primitive* (o19/o20) and never as a *use* — delete
    // the whole arm and both stayed green. That is M92's finding one level
    // over: a gate that cannot reach a call site does not test it.
    //
    // The view is SUPPLIED, not resolved, because resolving it needs an item
    // registry this gate has not got. What that leaves untested is the
    // resolution in `live_frame`, which `d7`–`d9` grade item-side and
    // `loom_pattern_table`'s own tests grade set-side.
    loom: Option<LoomView>,
    // M93s — the stonecutter's grid, supplied for the same reason.
    cut: Option<&CutView>,
    // M93u — and the merchant's trade list.
    merchant: Option<&MerchantView>,
) -> Option<rewo_gpu::container::ContainerPanel> {
    container_panel(
        open.layout,
        Some(open),
        EnchantPlayer {
            xp_level,
            creative,
            beacon_effects,
            beacon_override,
            loom,
            cut,
            // A gate drives no anvil field; `containershot` supplies its own
            // overlays when it wants a fill (M93q's o19/o20).
            anvil_fills: &[],
            merchant,
            // A gate drives no ghost recipe either.
            ghost_under: &[],
            ghost_over: &[],
        },
        mouse_gui,
    )
}

/// [`BeaconEffectIds::resolve`] for `containershot`.
pub(crate) fn beacon_effect_ids_for_test(
    m: &rewo_data::mob_effects::MobEffects,
) -> BeaconEffectIds {
    BeaconEffectIds::resolve(m)
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
/// Where each of a menu's slots sits on screen.
///
/// `book_open` is not optional decoration: an open recipe book MOVES the menu
/// (M94), and this origin has to move with the panel's or every icon lands 77 px
/// left of the slot it belongs to. M94 threaded the book through the panel draw
/// and the hover and **missed this path**, which is the one-accessor rule half
/// applied — the same shape as M90's `slot_kind`, and missed for the same
/// reason: a function taking bare numbers does not look like it belongs to the
/// menu.
fn menu_slot_rects(
    menu: &rewo_world::inventory::Inventory,
    w: f32,
    h: f32,
    book_open: bool,
) -> Vec<(f32, f32, f32)> {
    let layout = menu.layout();
    let (left, top, scale) = rewo_gpu::container::gui_origin_placed(
        w,
        h,
        rewo_gpu::container::Placement::with_book(
            layout.image_w as f32,
            layout.image_h as f32,
            book_open,
        ),
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
    menu_slot_rects(&rewo_world::inventory::Inventory::default(), w, h, false)
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
pub(crate) fn screen_tooltip(
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
    // M93k — the SHOWN menu's layout. Without it this hover centres a
    // 176x166 panel and scans the player's 46 slots, which is the M89 bug in
    // a fourth consumer: the highlight and the icons were made container-aware
    // and this one was not, so with a chest open the tooltip named whatever
    // the PLAYER happened to have at the same index.
    layout: &'static rewo_world::menu_layout::MenuLayout,
    open: Option<&rewo_world::menu::OpenMenu>,
    // `player.isSpectator()` — the fifth of the hint's conditions.
    spectator: bool,
    // M106b — whether the recipe book is open, which MOVES the menu. The panel,
    // its slot icons and its hover highlight all resolve their origin through
    // `Placement::with_book`; this one resolved through the centred form, so
    // with the book up it named a slot 77 GUI px right of the cursor.
    book_open: bool,
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
    let (gx, gy) = rewo_gpu::container::screen_to_gui_placed(
        mouse,
        w,
        h,
        rewo_gpu::container::Placement::with_book(
            layout.image_w as f32,
            layout.image_h as f32,
            book_open,
        ),
    );
    let slot = layout.slot_at(gx, gy)?;
    // M93k — the crafter's `gui.togglable_slot` hint, which is shown on an
    // EMPTY slot and so must be resolved before the item tooltip's
    // `menu_slot(slot)?` bails out.
    //
    // Vanilla's five conditions here are exactly the preconditions of a PICKUP
    // that would DISABLE the slot, so the hint is DERIVED from the same
    // decision the click uses rather than transcribed a second time — the two
    // then cannot disagree about whether a click would do anything. The
    // string says as much: the constant is named DISABLED_SLOT_TOOLTIP and
    // reads "Click to disable slot", and it appears on an ENABLED slot.
    let crafter_hint: Option<Vec<rewo_gpu::tooltip::Line>> = open.and_then(|m| {
        if !rewo_world::menu::is_crafter_grid_slot(layout.protocol_id, slot as i32) {
            return None;
        }
        let would_disable = rewo_world::menu::crafter_toggle(
            rewo_world::inventory::CONTAINER_INPUT_PICKUP,
            m.crafter_slot_disabled(slot as i32),
            inv.menu_slot(slot).is_some(),
            spectator,
            // Already known empty — the guard at the top of this function
            // returns early otherwise — but passed rather than hard-coded, so
            // the two guards cannot drift apart.
            inv.carried().is_none(),
            false,
        ) == rewo_world::menu::CrafterToggle::Disable;
        if !would_disable {
            return None;
        }
        lang.get("gui.togglable_slot")
            .map(|t| vec![vec![rewo_gpu::tooltip::Span::new(t.to_string(), [1.0, 1.0, 1.0])]])
    });
    // The item tooltip's content. A closure so the crafter's hint can
    // select between the two and share the assembly below — the measure,
    // position and glyph-run code is the same for any tooltip, and a second
    // copy of it is how two tooltips come to sit in different places.
    let item_lines = || -> Option<Vec<rewo_gpu::tooltip::Line>> {
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

        Some(lines)
    };
    let lines = match crafter_hint {
        Some(l) => l,
        None => item_lines()?,
    };
    tooltip_layout(lines, advance, glyphs, mouse, (w, h))
}

/// The tooltip of the recipe cell under the cursor (M106).
///
/// `RecipeBookPage.extractTooltip` — `Screen.getTooltipFromItem(displayStack)`
/// plus a "Right Click for More" line when the collection holds more than one
/// recipe.
///
/// **This loses to the menu's own tooltip, and that is not the order it is
/// called in.** `AbstractRecipeBookScreen` runs `this.extractTooltip` (the
/// container's) and *then* `recipeBookComponent.extractTooltip`, which reads as
/// "the book overwrites it" — and `setTooltipForNextFrameInternal`'s body is
/// `if (this.deferredTooltip == null || replaceExisting)`, with
/// `replaceExisting` false on every path either of them takes. So the FIRST
/// tooltip of a frame wins and the book's is discarded. Hence the caller's
/// `screen_tooltip(..).or_else(..)` rather than the reverse.
///
/// For a page cell the two can never both fire — the book sits beside the menu
/// on a wide window, and on a narrow one `AbstractRecipeBookScreen.isHovering`
/// returns false for every slot, so there is no hovered slot to describe. It is
/// the GHOST tooltip that makes first-wins observable; see [`ghost_tooltip`].
///
/// The stack is Rewo's `slot_items` id, which carries no components (M95): a
/// recipe result that shipped with a custom name or lore would show its plain
/// name here. The advanced block is still produced, because F3+H adds the
/// registry id to a book cell in vanilla too.
#[allow(clippy::too_many_arguments)]
pub(crate) fn book_tooltip(
    b: &BookRender,
    overlay_open: bool,
    book_mouse: Option<(i32, i32)>,
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
    lang: &rewo_data::lang::Language,
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
    use rewo_world::recipe_book_screen as rb;
    let view = b.view?;
    let (bx, by) = book_mouse?;
    let slot = rb::page_tooltip_slot(rb::book_hit(bx, by, view, view.tabs), overlay_open)?;
    // `getDisplayStack()` — the item the cycle is currently on, which is
    // already what `slot_items` holds. A collection whose result needs a
    // context Rewo cannot build has `None` and shows nothing, rather than a
    // tooltip for some other member of the group.
    let item = (*b.slot_items.get(slot)?)?;
    let item_name = items.name(item)?;
    let mut lines: Vec<rewo_gpu::tooltip::Line> = vec![vec![rewo_gpu::tooltip::Span::new(
        names.get(item_name)?.to_string(),
        rarity_color(stack_rarity(Some(item_name), None, false)),
    )]];
    {
        let has = |c: &str| stack_has_component(item_name, c, None, None);
        let durability = rewo_gpu::tooltip::DurabilityState {
            // A recipe result is undamaged by construction: the display stack
            // comes from the recipe, not from an inventory.
            damage: 0,
            max: rewo_data::item_props_table::max_damage(item_name).unwrap_or(0),
            has_max_damage: has("minecraft:max_damage").unwrap_or(false),
            has_damage: has("minecraft:damage").unwrap_or(false),
            unbreakable: has("minecraft:unbreakable").unwrap_or(false),
        };
        let advanced = rewo_gpu::tooltip::advanced_lines(
            flag,
            durability,
            true,
            rewo_data::item_components_table::prototype_component_count(item_name),
        );
        lines.extend(advanced_tooltip_lines(&advanced, item_name, lang));
    }
    // `if (this.hasMultipleRecipes()) texts.add(MORE_RECIPES_TOOLTIP)` — LAST,
    // after everything the item contributed, including the advanced block.
    if b.slots.get(slot).is_some_and(|&(_, multiple)| multiple) {
        lines.push(vec![rewo_gpu::tooltip::Span::new(
            lang.or_key(rb::MORE_RECIPES_KEY).to_string(),
            [1.0, 1.0, 1.0],
        )]);
    }
    tooltip_layout(lines, advance, glyphs, mouse, (w, h))
}

/// Turn a resolved set of tooltip lines into its box, position and glyph runs
/// (M106).
///
/// Extracted from [`screen_tooltip`] unchanged so the recipe book's own
/// tooltips can share it. The comment it was extracted from already said why —
/// "the measure, position and glyph-run code is the same for any tooltip, and a
/// second copy of it is how two tooltips come to sit in different places" — and
/// M106 is the second producer that makes it true rather than prospective.
fn tooltip_layout(
    lines: Vec<rewo_gpu::tooltip::Line>,
    advance: &[u8; 256],
    glyphs: Option<&mut rewo_gpu::velvet_glyph::GlyphCache>,
    mouse: (f64, f64),
    (w, h): (f32, f32),
) -> Option<(
    rewo_gpu::container::TooltipDraw,
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
)> {
    // `if (!lines.isEmpty())` in `setTooltipForNextFrameInternal` — an empty
    // list sets no tooltip at all, which is not the same as an empty box.
    if lines.is_empty() {
        return None;
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
                color_linear: srgb_bytes_to_linear_f(color),
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
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

pub(crate) fn screen_icons(
    inv: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    trim_materials: &[rewo_net::trim_parse::TrimMaterialDef],
    w: f32,
    h: f32,
    // M93j — the open menu, for the slots whose render it REPLACES. `None` is
    // the player's own inventory, which replaces none.
    open: Option<&rewo_world::menu::OpenMenu>,
    // M93s — the stonecutter's recipe grid, whose buttons draw item icons that
    // belong to no slot.
    cut: Option<&CutView>,
    // M93u — and the merchant's trade rows, likewise.
    merchant: Option<&MerchantView>,
    // M95 — the recipe book, whose tab icons and recipe results are items.
    // Its presence also MOVES the menu, so it moves every icon measured from
    // the menu's origin too.
    book: Option<&BookRender>,
    // M103 — the ghost recipe's slots, and the cycle index its items rotate on.
    ghosts: &[rewo_world::ghost_slots::Ghost],
    cycle: i32,
    // M104 — the open which-of-these overlay, whose ingredient grids are items
    // too. Supplied rather than reached for, so a gate drives the same call
    // the live client makes (M45).
    overlay: Option<&rewo_world::recipe_overlay::Open>,
) -> (Vec<rewo_gpu::gui_item::GuiItem>, Vec<rewo_gpu::world::OwnedTextLine>) {
    let rects = menu_slot_rects(inv, w, h, book.is_some());
    let (_, _, scale) = rewo_gpu::container::gui_origin(w, h);
    let mut icons = Vec::new();
    let mut labels = Vec::new();
    for (slot, rect) in rects.iter().enumerate() {
        // A slot the screen covers draws neither its icon nor its count —
        // `extractSlot` never reaches `super`, and the count is part of what
        // super draws.
        if open.is_some_and(|m| m.slot_hides_item(slot)) {
            continue;
        }
        if let Some(stack) = inv.menu_slot(slot) {
            if let Some(icon) = icon_for(items, trim_materials, stack, rect.0, rect.1, rect.2) {
                icons.push(icon);
            }
            labels.extend(count_label(stack, rect.0, rect.1, scale));
        }
    }
    // M103 — the ghost recipe's items, in the menu's own slots. Between the two
    // washes by construction: the red is in the panel's overlays (back half) and
    // the white in its front overlays, and the icon pass runs between them.
    //
    // **No count label** except on the result, which is `itemDecorations`'
    // own rule — an input ghost never shows a number even when the recipe wants
    // several of that ingredient.
    for g in ghosts {
        let Some(id) = g.item(cycle) else { continue };
        let Some((sx, sy)) = inv.layout().position(g.slot) else { continue };
        let (left, top, _) = rewo_gpu::container::gui_origin_placed(
            w,
            h,
            rewo_gpu::container::Placement::with_book(
                inv.layout().image_w as f32,
                inv.layout().image_h as f32,
                book.is_some(),
            ),
        );
        if let Some(icon) = icon_for(
            items,
            trim_materials,
            rewo_world::inventory::ItemSlot::plain(id, 1),
            left + sx as f32 * scale,
            top + sy as f32 * scale,
            16.0 * scale,
        ) {
            icons.push(icon);
        }
    }
    // M95 — the recipe book's items: the tab icons and the page's results.
    //
    // On the BOOK's origin, not the menu's — the book is window-anchored and
    // the gap between the two alternates with the window's parity (M94).
    //
    // **No count label**, for the stonecutter's reason one screen over: both
    // `fakeItem` and `graphics.item` draw the model alone, and
    // `itemDecorations` is a separate call neither makes. A recipe yielding 8
    // torches shows no "8".
    if let Some(b) = book.and_then(|b| b.view.map(|v| (b, v))) {
        let (bk, view) = b;
        let (bl, bt, _) = rewo_gpu::container::recipe_book_origin(w, h);
        let tabs = bk.book.tabs();
        for icon in rewo_world::recipe_book_screen::book_icons(view, tabs, &bk.slot_shadowed) {
            use rewo_world::recipe_book_screen::BookIconKind as K;
            let id = match icon.kind {
                K::TabPrimary(i) => tabs.get(i).and_then(|t| items.id(t.primary)),
                K::TabSecondary(i) => tabs.get(i).and_then(|t| t.secondary).and_then(|n| items.id(n)),
                // The SAME item for both copies: the shadow is the display
                // stack drawn twice, not a second recipe's result.
                K::Slot { index, .. } => bk.slot_items.get(index).copied().flatten(),
            };
            let Some(id) = id else { continue };
            if let Some(g) = icon_for(
                items,
                trim_materials,
                rewo_world::inventory::ItemSlot::plain(id, 1),
                bl + icon.x as f32 * scale,
                bt + icon.y as f32 * scale,
                16.0 * scale,
            ) {
                icons.push(g);
            }
        }
    }
    // M104 — the which-of-these overlay's ingredient grids. On the BOOK's
    // origin like the cells above, and drawn after them because the overlay is
    // a stratum over the whole book.
    //
    // **6 px, not 16**: `scale(0.375F)` sits between two translates, so the
    // trailing `-8, -8` is scaled with it and `Pos` is the ingredient's CENTRE.
    // `item_rect` is where that composition lives.
    //
    // No count label, for the same reason the cells have none: `graphics.item`
    // draws the model alone.
    if let Some(o) = overlay {
        let (bl, bt, _) = rewo_gpu::container::recipe_book_origin(w, h);
        for (i, b) in o.buttons.iter().enumerate() {
            let button = rewo_world::recipe_overlay::button_origin(o.origin, i, o.total());
            for (pos, choices) in &b.slots {
                // One-level cycle, on the same clock the cells use — so a cell
                // and the overlay it opened can be showing different items on
                // the same frame.
                let Some(n) = rewo_world::recipe_overlay::select_ingredient(choices.len(), cycle)
                else {
                    continue;
                };
                let (ix, iy, size) = rewo_world::recipe_overlay::item_rect(button, *pos);
                if let Some(g) = icon_for(
                    items,
                    trim_materials,
                    rewo_world::inventory::ItemSlot::plain(choices[n], 1),
                    bl + ix * scale,
                    bt + iy * scale,
                    size * scale,
                ) {
                    icons.push(g);
                }
            }
        }
    }
    // M93s — the stonecutter's recipe buttons, which are ITEMS and so belong
    // here rather than in the overlay atlas.
    //
    // **No count label.** `extractRecipes` calls `graphics.item`, which draws
    // the model alone; `itemDecorations` is a separate call it never makes. So
    // a slab recipe yielding 2 shows no "2", unlike every real slot above —
    // reusing `count_label` here would be the natural thing and would be wrong
    // on 124 of the 319 recipes.
    if let Some(c) = cut.filter(|c| c.display) {
        let (left, top, _) = rewo_gpu::container::gui_origin_for(
            w,
            h,
            open.map_or(176.0, |m| m.layout.image_w as f32),
            open.map_or(166.0, |m| m.layout.image_h as f32),
        );
        for pos_index in 0..rewo_world::menu_screen::CUT_PAGE {
            let index = c.start_index + pos_index;
            let Some(recipe) = usize::try_from(index).ok().and_then(|i| c.recipes.get(i)) else {
                break;
            };
            let Some(id) = items.id(recipe.result) else { continue };
            let (gx, gy) = rewo_world::menu_screen::cut_cell_origin(pos_index);
            if let Some(icon) = icon_for(
                items,
                trim_materials,
                rewo_world::inventory::ItemSlot::plain(id, recipe.count as i32),
                left + gx as f32 * scale,
                top + gy as f32 * scale,
                16.0 * scale,
            ) {
                icons.push(icon);
            }
        }
    }
    // M93u — the merchant's three items per visible row. Like the
    // stonecutter's grid these are items rather than sprites, and unlike it
    // they DO carry counts: `extractOffers` calls `itemDecorations` for cost B
    // and the result, and `extractAndDecorateCostA` for cost A.
    if let Some(v) = merchant {
        use rewo_world::merchant_screen as ms;
        let (left, top, scale) = rewo_gpu::container::gui_origin_for(
            w,
            h,
            open.map_or(176.0, |m| m.layout.image_w as f32),
            open.map_or(166.0, |m| m.layout.image_h as f32),
        );
        let n = v.offers.len();
        for (i, offer) in v.offers.iter().enumerate() {
            let idx = i as i32;
            if !ms::offer_visible(idx, v.scroll_off, n) {
                continue;
            }
            let row = if ms::can_scroll(n) { idx - v.scroll_off } else { idx };
            let y = ms::row_item_y(row);
            // Returns rather than pushes, so the cost-A branch below can use
            // the same geometry without a second mutable borrow.
            let at = |gx: i32, id: i32, count: i32| {
                let (px, py) = (left + gx as f32 * scale, top + y as f32 * scale);
                (
                    icon_for(
                        items,
                        trim_materials,
                        rewo_world::inventory::ItemSlot::plain(id, count),
                        px,
                        py,
                        16.0 * scale,
                    ),
                    count_label(
                        rewo_world::inventory::ItemSlot::plain(id, count),
                        px,
                        py,
                        scale,
                    ),
                )
            };
            // Cost A: ONE icon, at its MODIFIED count — `fakeItem` is called
            // once, outside `extractAndDecorateCostA`'s branch. The discounted
            // display is two NUMBERS over a single item, not two items.
            let modified = v.cost_a_counts[i];
            let disp = ms::cost_a_display(offer.cost_a.count, modified);
            //
            // Only `.0`, the icon: cost A's DIGITS take the branch below.
            // `icon_for` draws the item's model and ignores the count, so
            // which count is passed here is inert — mutating it to the base
            // cost is an equivalent mutant, and it is spelled `modified`
            // because that is the stack vanilla passes to `fakeItem`.
            icons.extend(at(ms::COST_A_X, offer.cost_a.item_id, modified).0);
            // Both digits are FORCED in the discounted branch, including a 1 —
            // `count == 1 ? "1" : null` exists to defeat `itemCount`'s own
            // "a single item shows no digit" rule.
            let forced = |n: i32| (n == 1).then(|| n.to_string());
            for (gx, count) in [
                (ms::COST_A_X, disp.at_icon),
                (ms::COST_A_X + ms::DISCOUNT_SECOND_X, disp.at_second),
            ] {
                let Some(n) = count else { continue };
                labels.extend(count_label_of(
                    n,
                    disp.strikethrough.then(|| forced(n)).flatten(),
                    left + gx as f32 * scale,
                    top + y as f32 * scale,
                    scale,
                ));
            }
            // Cost B and the result take the ordinary path: `itemDecorations`
            // with no `countText`, so a single item shows no digit there.
            for (gx, id, count) in [
                offer.cost_b.as_ref().map(|b| (ms::COST_B_X, b.item_id, b.count)),
                match &offer.result {
                    rewo_net::item_stack::WireSlot::Stack(st) => {
                        Some((ms::RESULT_X, st.item_id, st.count))
                    }
                    rewo_net::item_stack::WireSlot::Empty => None,
                },
            ]
            .into_iter()
            .flatten()
            {
                let (icon, label) = at(gx, id, count);
                icons.extend(icon);
                labels.extend(label);
            }
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
    count_label_of(stack.count, None, x, y, scale)
}

/// `itemCount` with vanilla's `countText` override (M93w).
///
/// ```java
/// if (itemStack.getCount() != 1 || countText != null) {
///    String amount = countText == null ? String.valueOf(itemStack.getCount()) : countText;
/// ```
///
/// **The override's only job is to defeat the `!= 1` rule.** A single item
/// normally shows no digit; the merchant's discounted price passes
/// `count == 1 ? "1" : null` so both halves of the comparison stay visible,
/// which matters exactly when a discount has reached 1.
fn count_label_of(
    count: i32,
    force: Option<String>,
    x: f32,
    y: f32,
    scale: f32,
) -> Option<rewo_gpu::world::OwnedTextLine> {
    if count == 1 && force.is_none() {
        return None;
    }
    let text = force.unwrap_or_else(|| count.to_string());
    // Vanilla measures the string; the digits are a uniform 6 px including
    // their one-pixel gap, and the trailing gap is not part of the width.
    let width = text.chars().count() as f32 * 6.0 - 1.0;
    Some(rewo_gpu::world::OwnedTextLine {
        // `x + 19 - 2 - width`, `y + 6 + 3`.
        x: x + (17.0 - width) * scale,
        y: y + 9.0 * scale,
        px: scale,
        color_linear: [1.0, 1.0, 1.0],
        alpha: 1.0,
        shadow: true,
        style: rewo_gpu::text::TextStyle::PLAIN,
        text,
    })
}

/// The enchanting table's three cost numerals (M92).
///
/// The first text a container screen draws that is not a stack count, and the
/// alignment is why it needs the real advance table rather than a 6-px-per-
/// digit estimate: `leftPosText + 86 - font.width(costText)` is **right**-
/// aligned, so a wrong width moves a two-digit cost and leaves a one-digit one
/// looking correct.
///
/// An empty row draws nothing at all — `cost == 0` returns before the numeral,
/// the name and the cost, so a table with no item shows three blank rows.
fn enchant_cost_labels(
    rows: [rewo_world::menu_screen::EnchantRow; 3],
    advance: &[u8; 256],
    w: f32,
    h: f32,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    let layout = &rewo_world::menu_layout::REGISTRY[13]; // enchantment
    let (left, top, scale) =
        rewo_gpu::container::gui_origin_for(w, h, layout.image_w as f32, layout.image_h as f32);
    let mut out = Vec::new();
    for (i, row) in rows.into_iter().enumerate() {
        let (Some(cost), Some(rgb)) = (row.cost(), row.cost_color()) else {
            continue;
        };
        let text = cost.to_string();
        let (x, y) =
            rewo_world::menu_screen::enchant_cost_pos(i, rewo_gpu::text::width(&text, advance));
        out.push(rewo_gpu::world::OwnedTextLine {
            x: left + x as f32 * scale,
            y: top + y as f32 * scale,
            px: scale,
            color_linear: srgb_bytes_to_linear(rgb),
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text,
        });
    }
    out
}

/// The anvil name field's geometry, from `AnvilScreen.subInit` (M93t).
///
/// ```java
/// this.name = new EditBox(this.font, xo + 62, yo + 24, 103, 12, …);
/// this.name.setBordered(false);
/// ```
///
/// Unbordered, so `textX = getX() + 0` and `textY = getY()` — a bordered box
/// would inset by 4 and centre vertically by `(height - 8) / 2`, and reusing
/// those here would put the name four pixels right and two down.
/// `getInnerWidth()` is likewise the full 103 rather than `width - 8`.
const ANVIL_FIELD: (i32, i32, i32) = (62, 24, 103);

/// The name field's text, cursor and selection (M93t).
///
/// The two-piece draw in `extractWidgetRenderState` exists to *place the
/// cursor*, not to change the text: the halves are separated by `+1` and then
/// pulled back by `-1` for an insert cursor, so the run is contiguous either
/// way and one label is exact.
fn anvil_field_render(
    local: &rewo_world::edit_box::EditBox,
    advance: &[u8; 256],
    w: f32,
    h: f32,
    now_ms: u64,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
    Option<String>,
) {
    let layout = &rewo_world::menu_layout::REGISTRY[ANVIL_MENU_PROTOCOL_ID as usize];
    let (left, top, scale) =
        rewo_gpu::container::gui_origin_for(w, h, layout.image_w as f32, layout.image_h as f32);
    let (fx, fy, inner) = ANVIL_FIELD;
    edit_box_render(local, advance, (left, top, scale), (fx, fy, inner), None, now_ms)
}

/// One `EditBox`'s text, caret and selection — shared by the anvil's name field
/// (M93t) and the recipe book's search (M100).
///
/// Extracted rather than copied: the caret's x is the width of the run before
/// it, the `insert` rule decides whether the caret is a bar or an underscore,
/// and the selection's rect is clamped against the inner width. A second copy
/// of that is three chances to drift, and the drift would be a caret one pixel
/// out — invisible in review and obvious in use.
///
/// `origin` is the surface's `(left, top, scale)` in screen pixels; `field` is
/// `(x, y, inner_width)` in that surface's own coordinates. `hint` is the text
/// drawn when the field is **empty and unfocused**, with its own colour.
fn edit_box_render(
    local: &rewo_world::edit_box::EditBox,
    advance: &[u8; 256],
    (left, top, scale): (f32, f32, f32),
    (fx, fy, inner): (i32, i32, i32),
    hint: Option<(&str, [f32; 3])>,
    // Wall-clock milliseconds, for the caret's blink phase (M101).
    now_ms: u64,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
    Option<String>,
) {
    let width = |u: &[u16]| rewo_gpu::text::width(&String::from_utf16_lossy(u), advance);
    let displayed = local.displayed(inner, &width).to_vec();
    let rel_cursor = local.cursor_position().saturating_sub(local.display_pos());
    let on_screen = rel_cursor <= displayed.len();

    let mut labels = Vec::new();
    let mut fills = Vec::new();
    let mut append = None;
    let text = String::from_utf16_lossy(&displayed);
    if !text.is_empty() {
        labels.push(rewo_gpu::world::OwnedTextLine {
            x: left + fx as f32 * scale,
            y: top + fy as f32 * scale,
            px: scale,
            // `setTextColor(-1)` AND `setTextColorUneditable(-1)` — the anvil
            // sets both to white, so an uneditable field is not greyed.
            color_linear: [1.0, 1.0, 1.0],
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text,
        });
    }

    // `if (hint != null && displayed.isEmpty() && !isFocused())` — the hint
    // goes when the field takes FOCUS, not when the first character arrives.
    // Clicking an empty search box therefore blanks "Search..." before you
    // type, which reads as a bug until you check the guard.
    if let Some((text, color)) = hint {
        if displayed.is_empty() && !local.is_focused() {
            labels.push(rewo_gpu::world::OwnedTextLine {
                x: left + fx as f32 * scale,
                y: top + fy as f32 * scale,
                px: scale,
                // The hint is a styled COMPONENT, so its own colour wins over
                // the field's — `SEARCH_HINT_STYLE` is gray, where the book
                // sets the field itself to white. The constant is vanilla's
                // byte `/255`, so it converts here.
                color_linear: srgb_bytes_to_linear_f(color),
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: text.to_string(),
            });
        }
    }

    // `insert = cursorPos < value.length() || value.length() >= maxLength` —
    // so a full field shows the BAR even with the cursor at the end, which is
    // how vanilla tells you there is no room left.
    let insert = local.cursor_position() < local.len() || local.len() >= local.max_length();
    let before = if on_screen { &displayed[..rel_cursor] } else { &displayed[..] };
    let mut cursor_x = fx + width(before) + if before.is_empty() { 0 } else { 1 };
    if on_screen && insert {
        cursor_x -= 1;
    }

    // The selection: `textHighlight(min(cursorX, x+width), textY-1,
    // min(highlightX-1, x+width), textY+1+9, invert)`. The anvil sets
    // `setInvertHighlightedTextColor(false)`, so only the blue fill runs.
    let rel_highlight = local
        .highlight_position()
        .saturating_sub(local.display_pos())
        .min(displayed.len());
    if rel_highlight != rel_cursor {
        let hx = fx + width(&displayed[..rel_highlight]);
        let (x0, x1) = (cursor_x.min(fx + inner), (hx - 1).min(fx + inner));
        let (x0, x1) = (x0.min(x1), x0.max(x1));
        fills.push((
            rewo_gpu::container::FILL_SPRITE,
            rewo_gpu::container::PanelBlit {
                dx: x0 as f32,
                dy: (fy - 1) as f32,
                w: (x1 - x0) as f32,
                h: 11.0,
                sx: 0.0,
                sy: 0.0,
                sw: 0.0,
                sh: 0.0,
                // `-16776961` = 0xFF0000FF. NOTE the pipeline is
                // `GUI_TEXT_HIGHLIGHT`, whose blend Rewo's single container
                // blend does not reproduce — the colour is exact, the
                // compositing is a plain alpha draw.
                tint: [0.0, 0.0, 1.0, 1.0],
            },
        ));
    }

    // `showCursor = isFocused() && isCursorVisible(millis - focusedTime) &&
    // cursorOnScreen` — THREE conditions. M93t had only the first, so the
    // anvil's caret was solid and drawn even when scrolled out of view.
    if local.is_focused() && local.cursor_visible(now_ms) && on_screen {
        if insert {
            // `fill(x, y - 1, x + 1, y + lineHeight)`, lineHeight 9 + 1.
            fills.push((
                rewo_gpu::container::FILL_SPRITE,
                rewo_gpu::container::PanelBlit {
                    dx: cursor_x as f32,
                    dy: (fy - 1) as f32,
                    w: 1.0,
                    h: 11.0,
                    sx: 0.0,
                    sy: 0.0,
                    sw: 0.0,
                    sh: 0.0,
                    tint: [1.0, 1.0, 1.0, 1.0],
                },
            ));
        } else {
            // The append cursor is the CHARACTER "_", not a rectangle.
            append = Some(String::from("_"));
            labels.push(rewo_gpu::world::OwnedTextLine {
                x: left + cursor_x as f32 * scale,
                y: top + fy as f32 * scale,
                px: scale,
                color_linear: [1.0, 1.0, 1.0],
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: "_".into(),
            });
        }
    }
    (labels, fills, append)
}

/// [`anvil_field_render`] for `containershot`.
///
/// The clock is fixed at **0** (M101): a caret that blinks would make the same
/// scene render two ways depending on when the gate ran, and a witness cannot
/// hold that constant. At 0 the caret is visible, which is the state the
/// existing anvil witnesses were written against.
pub(crate) fn anvil_field_render_for_test(
    local: &rewo_world::edit_box::EditBox,
    advance: &[u8; 256],
    w: f32,
    h: f32,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
    Option<String>,
) {
    anvil_field_render(local, advance, w, h, 0)
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

/// Resolve an item id into the facts the click arithmetic needs.
///
/// `None` for an id the registry does not contain, which makes the whole click
/// decline rather than predicting against a guessed stack cap.
///
/// `pub(crate)` so `containershot` can grade **this** function rather than its
/// own copy. M92's finding is the reason: a gate that constructs the input
/// production must derive leaves the derivation untested by construction —
/// there, five `mob_effect` ids were read from a `registry_data` branch that
/// cannot fire, and `lightmapshot`/`swingshot` could not see it because both
/// supplied the ids themselves. Every unit test of the quick-move hand-builds
/// an `ItemProps`, so without a witness on this function the table lookups
/// below could all return the wrong thing in the live client and stay green.
pub(crate) fn item_props(
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
        // M93 — the beacon quick-move's one item predicate.
        beacon_payment: rewo_data::beacon_payment_table::is_beacon_payment(name),
        // M93b — the stonecutter's.
        stonecuttable: rewo_data::stonecutter_table::accepts_input(name),
        // M93e — the PROTOTYPE half of `isDamageableItem`. The patch half
        // rides on the stack. `prototype_has_component` is M56's table, which
        // already exists for the tooltip's component count.
        proto_max_damage: rewo_data::item_components_table::prototype_has_component(
            name,
            "minecraft:max_damage",
        )
        .unwrap_or(false),
        proto_damage: rewo_data::item_components_table::prototype_has_component(
            name,
            "minecraft:damage",
        )
        .unwrap_or(false),
        // M93f — `is(PAPER) || is(MAP) || is(GLASS_PANE)`. Item identity, so
        // three names rather than a table. `minecraft:map` is the EMPTY map;
        // `filled_map` is a different item and routes by its MAP_ID component.
        cartography_additional: matches!(
            name,
            "minecraft:paper" | "minecraft:map" | "minecraft:glass_pane"
        ),
        // M93g — the loom. The banner half is item identity from the tag; the
        // other two are the CONJUNCTIONS, tag AND prototype component, and a
        // tag-only test would look correct on every vanilla item.
        loom_banner: rewo_data::loom_table::is_banner(name),
        loom_dye: rewo_data::loom_table::in_loom_dyes(name)
            && rewo_data::item_components_table::prototype_has_component(
                name,
                "minecraft:dye",
            )
            .unwrap_or(false),
        loom_pattern: rewo_data::loom_table::in_loom_patterns(name)
            && rewo_data::item_components_table::prototype_has_component(
                name,
                "minecraft:provides_banner_patterns",
            )
            .unwrap_or(false),
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
        // A one-slot drag is re-dispatched as PICKUP, so it reaches
        // `slotClicked` exactly as a click does and must run the same toggle
        // (M93i). Vanilla has ONE `slotClicked` override; two call sites here
        // with only one of them toggling is how they would come to disagree.
        let toggle = session.crafter_slot_click(
            slot as i32,
            button,
            rewo_world::inventory::CONTAINER_INPUT_PICKUP,
        );
        if toggle != rewo_world::menu::CrafterToggle::None {
            println!("[rewo-m93i] CRAFTER slot {slot}: {toggle:?} (from a one-slot drag)");
        }
        if let Some(p) = session.shown_menu_mut().click_pickup(slot as i32, button, &props) {
            if session
                .container_click_input(&p, rewo_world::inventory::CONTAINER_INPUT_PICKUP)
                .is_ok()
            {
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
    screen: &mut ScreenState,
    action: SlotAction,
    w: f32,
    h: f32,
) {
    let book_open = book_visible(session);
    let Some(slot) = screen.hovered(session.shown_menu().layout(), w, h, book_open) else {
        return;
    };
    // M107 — `AbstractRecipeBookScreen.slotClicked` calls
    // `recipeBookComponent.slotClicked(slot)` after `super`, and that resets
    // `lastPlacedRecipe` and clears the ghost whenever the slot is a crafting
    // one. Placed HERE rather than after the send, because vanilla's reset is
    // gated only on which slot was clicked: a click that moves nothing still
    // clears it.
    {
        let layout = session.shown_menu().layout();
        let player_inventory =
            layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID;
        if book_type_of(layout).is_some_and(|b| {
            rewo_world::recipe_book_screen::is_crafting_slot(b, player_inventory, slot)
        }) {
            screen.place_guard.crafting_slot_clicked();
            session.ghost_recipe = None;
        }
    }
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
