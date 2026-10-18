//! The GPU-driven world renderer.
//!
//! All chunk geometry lives in two device-local **mega-buffers** (vertices +
//! indices), each column a free-list-suballocated region. A per-column
//! **metadata SSBO** (world-space AABB + draw params) feeds a **compute cull
//! pass**: one invocation per column frustum-tests its AABB and, if visible,
//! appends a `VkDrawIndexedIndirectCommand` and bumps an atomic count. The
//! CPU then issues a single **`vkCmdDrawIndexedIndirectCount`** — the draw
//! cost stops scaling with column count and culling runs on the GPU.
//!
//! Vertices are world-space (the mesher emits `cx*16+lx+corner`), so no
//! per-column origin is needed — exactly what one shared indirect draw wants.
//!
//! Frame split: `cull()` runs the compute + count reset BEFORE dynamic
//! rendering begins (compute can't run inside a render pass); `draw()` binds
//! the mega-buffers and issues the indirect draw INSIDE the pass.
//!
//! Column uploads (rare — only on chunk load / block edit) are staged into a
//! batch recorded on an upload-ring slot and submitted without a CPU fence
//! wait, so the per-FRAME path never stalls.
//!
//! [`WorldRenderer`] is the façade: it owns the GPU resources and the passes,
//! and the subsystems live in the child modules, each an `impl WorldRenderer`
//! block plus the sub-struct it owns:
//!
//! | module | owns |
//! |---|---|
//! | `resources` | `new` / `destroy`, the buffer helpers |
//! | `arena` | mega-buffers, free-lists, column registry, upload ring, retry queue |
//! | `cull` | compute cull, column metadata rings, draw-count readback |
//! | `terrain` | opaque / water pipelines, descriptor sets, world push block |
//! | `textures` | block texture array and its animated layers |
//! | `lightmap` | `WorldLightmapState`, the `LightmapExtra` UBO ring |
//! | `sky` | gradient sky, celestial / End sky, tints, fog band |
//! | `gamma` | the UNORM gamma-space scope (`in_gamma_space`) |
//! | `frame` | `draw` and the world half of the frame's draw order |
//! | `lines` | selection outline, leash ribbons |
//! | `scene_passes` | end portal, clouds, border, weather, particles, crumbling |
//! | `entity_passes` | entity + preview passes, per-player texture uploads |
//! | `gui_passes` | hand, HUD, container, screen chrome, text; `GuiFrame` |
//! | `depth` | `DepthTarget`, reversed-Z projection |

use ash::vk;

use crate::entities::EntityPass;
use crate::hud::HudPass;
use crate::text::TextPass;

mod arena;
mod cull;
mod depth;
mod entity_passes;
mod frame;
mod gamma;
mod gui_passes;
mod lightmap;
mod lines;
mod resources;
mod scene_passes;
mod sky;
mod terrain;
mod textures;

use arena::Arena;
pub use arena::{MAX_COLUMNS, MAX_INDICES, MAX_VERTS, VERTEX_STRIDE};
use cull::Culler;
pub use depth::{perspective_reverse_z, DepthTarget};
pub use entity_passes::PlayerTexture;
pub use gamma::{unorm_of, GammaScope};
use gui_passes::GuiFrame;
pub use gui_passes::OwnedTextLine;
use lightmap::LightmapRing;
pub use lightmap::{WorldLightmapState, BLOCK_LIGHT_FACTOR};
use lines::{LeashRibbon, SelectOutline};
use sky::SkyGradient;
pub use sky::{SkyMode, SKY_HORIZON};
use terrain::TerrainPass;
pub(crate) use textures::upload_texture_array;
use textures::TextureArray;
pub use textures::{LayerAnimation, FLOW_TEX_SIZE};

pub const DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT;

pub struct WorldRenderer {
    // -- graphics --
    /// The terrain pipelines (opaque + water) and their descriptor sets.
    terrain: TerrainPass,
    /// Fullscreen gradient sky (no descriptor set, no vertex input).
    sky_gradient: SkyGradient,
    /// Block-selection outline (LINE_LIST, depth-tested against terrain).
    select_outline: SelectOutline,
    /// Leash ribbons (M170).
    leash_ribbon: LeashRibbon,
    /// The `LightmapExtra` uniform buffers (one per ring slot, host-mapped)
    /// and the cursor naming the slot this frame writes and binds.
    lm: LightmapRing,
    /// The block texture array, its sampler and the animated layers.
    textures: TextureArray,
    /// The fluid flow sprites' 32 px array (M197), sampled by a flowing top.
    flow_textures: TextureArray,
    /// The terrain arena: mega-buffers, free-lists, registered columns and the
    /// async upload ring (see `arena.rs`).
    arena: Arena,
    /// Players whose skin / cape / face slot an upload took since the last
    /// [`WorldRenderer::take_evicted_textures`].
    evicted_textures: Vec<(u128, PlayerTexture)>,
    /// GPU culling: the compute pipeline, its buffer rings and the column
    /// metadata they are filled from (see `cull.rs`).
    culler: Culler,
    // -- entities + HUD (optional passes; drawn after terrain in `draw`) --
    color_format: vk::Format,
    /// This frame's attachment views for the glint's gamma-space scope (M50).
    gamma: Option<GammaScope>,
    entities: Option<EntityPass>,
    /// Sun/moon/star pass (M12), drawn between the gradient sky and terrain.
    /// `None` when celestial textures were unavailable (degrade to bare sky).
    celestial: Option<crate::celestial::CelestialPass>,
    /// The End skybox pass (M16). `None` until `init_end_sky` supplies
    /// `textures/environment/end_sky.png` — a missing asset degrades to no sky
    /// contribution at all, never to an invented flat colour.
    end_sky: Option<crate::end_sky::EndSkyPass>,
    /// The end portal / gateway pass (M32). `None` until `init_end_portal`
    /// supplies both textures — a missing asset degrades to no portal drawn,
    /// never to an invented colour.
    end_portal: Option<crate::end_portal::EndPortalPass>,
    /// The cloud deck (M33). `None` until `init_clouds`; a dimension whose
    /// `cloud_color` alpha is zero simply never gets a draw.
    clouds: Option<crate::clouds::CloudPass>,
    /// Rain and snow (M33). `None` until `init_weather` supplies both textures.
    weather: Option<crate::weather::WeatherPass>,
    /// The world-border wall (M80). `None` until `init_border` supplies
    /// `misc/forcefield.png`; `Some` with no `set_border` draws nothing, which
    /// is what a session inside a 60-million-block default border wants.
    border: Option<crate::border::BorderPass>,
    /// Item icons in hotbar slots (M34). `None` until `init_gui_items`.
    gui_items: Option<crate::gui_item::GuiItemPass>,
    /// Rebuild counters for the two ringed passes the app repacks mid-session,
    /// so `live --render-check` can tell a legitimate ring reset from a
    /// use-after-free. See [`Self::gui_item_generation`].
    gui_item_generation: u64,
    hand_generation: u64,
    /// M192 — counters for the hand atlas: full installs and region updates.
    hand_atlas_installs: u64,
    hand_region_updates: u64,
    /// The inventory screen (M35). `None` until `init_container`; `Some` but
    /// closed draws nothing.
    container: Option<crate::container::ContainerPass>,
    /// The screen framework's chrome pass (M82) — a screen's backdrop gradient
    /// and its buttons. Distinct from `container`, which is the *inventory's*
    /// panel and slot highlights and is inventory-specific throughout.
    screen: Option<crate::screen::ScreenPass>,
    /// This frame's GUI inputs — what the `set_*` calls stored for the
    /// GUI passes to draw at `render` (the passes themselves stay above).
    gui: GuiFrame,
    /// This frame's scene inputs — camera, lightmap, sky and fog, celestial
    /// and End state, the selection — what the `set_*` calls stored for the
    /// world passes to draw at `render`.
    scene: SceneFrame,
    /// A **second** entity pass, for the inventory screen's player preview
    /// (M36).
    ///
    /// Its own instance rather than a mode on the world's, because the two
    /// disagree about everything the pass holds: the world's buffer carries
    /// every visible entity and is drawn once through one matrix, and the
    /// preview carries exactly one model drawn through a different one. Two
    /// `set_draws` calls into a single ring would leave the first draw reading
    /// the second's vertices.
    ///
    /// Built on first use, so a session that never opens the inventory never
    /// pays for the second atlas.
    preview: Option<crate::entities::EntityPass>,
    /// The first-person hand (M38). `None` until `init_hand`.
    hand: Option<crate::hand_pass::HandPass>,
    /// Particles (M37). `None` until `init_particles` supplies the atlas.
    particles: Option<crate::particles::ParticlePass>,
    /// The block-break crack overlay (M81). `None` until `init_crumbling`
    /// supplies the ten stage textures.
    crumbling: Option<crate::crumbling::CrumblingPass>,
    hud: Option<HudPass>,
    locator: Option<crate::locator_bar::LocatorBarPass>,
    text: Option<TextPass>,
    /// The Velvet type stack (M52b) — real variable-font text, for the pieces
    /// that need per-run styling the bitmap pass cannot express (tooltips
    /// first; chat and F3 next).
    velvet_text: Option<crate::velvet_text::VelvetTextPass>,
    pub drawn_last_frame: usize,
    pub culled_last_frame: usize,
}

/// One frame's scene inputs, separate from the renderer's GPU resources:
/// what the world passes (sky, terrain, entities, celestial, End, selection)
/// read when the frame is recorded. Each field is written by a
/// `WorldRenderer::set_*` call. The counterpart of [`GuiFrame`].
struct SceneFrame {
    /// Eye position for translucent sort + fog origin (`set_camera`).
    camera_eye: [f32; 3],
    /// The resolved lightmap uniforms (sky/block factors, sky colour, gamma,
    /// darkness, night vision). Set by `set_lightmap` / `set_lightmap_state`.
    lightmap: WorldLightmapState,
    sky_tint: [f32; 3],
    fog_tint: [f32; 3],
    /// Biome sky/fog base color, LINEAR (M14). Defaults to the `SKY_HORIZON`
    /// constant so the sky/demo/skyshot render exactly as before until the live
    /// app supplies the camera biome color. The horizon = `sky_base`, the zenith
    /// = `sky_base * ZENITH_RATIO`, both then multiplied by the day/night
    /// `sky_tint`; fog = `fog_base * fog_tint`.
    sky_base: [f32; 3],
    fog_base: [f32; 3],
    /// Distance-fog band [start, end] (env `REWO_FOG=start,end`).
    fog: [f32; 2],
    /// Which sky `draw` renders (`DimensionType.Skybox`). Default
    /// [`SkyMode::Overworld`] — the pre-M16 behaviour.
    sky_mode: SkyMode,
    /// Current clear-weather celestial state (set by `set_celestial`).
    celestial_state: crate::celestial::CelestialState,
    /// `(intensity, xAngle, yAngle)` for the End flash, or `None` where the
    /// level has no `EndFlashState` at all — which is every dimension whose
    /// skybox is not END.
    end_flash: Option<(f32, f32, f32)>,
    /// The game time the end portal shader scrolls on (`set_end_portals`).
    end_portal_time: f32,
    /// The targeted block (world coords), or `None` — set each frame.
    selection: Option<[i32; 3]>,
}

impl WorldRenderer {
    /// Set the frame's eye position — drives translucent back-to-front
    /// column sorting. Callers that never render water can skip it.
    pub fn set_camera(&mut self, eye: [f32; 3]) {
        self.scene.camera_eye = eye;
    }
}
