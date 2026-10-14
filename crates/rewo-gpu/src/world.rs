//! M5 GPU-driven world renderer.
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
//! Column uploads (rare — only on chunk load / block edit) go through a
//! one-shot staging copy with its own fence, so the per-FRAME path never
//! stalls (removing M4's per-frame `wait_idle`). The dedicated async
//! transfer queue + staging ring is the M5 follow-on.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::entities::{EntityDraw, EntityPass, FontData, MobTextures};
use crate::hud::{HudPass, HudSpritesData};
use crate::text::{TextLine, TextPass};
use crate::Gpu;

mod arena;
mod cull;
mod gamma;
mod lightmap;
mod sky;
mod textures;

use arena::Arena;
pub use arena::{MAX_COLUMNS, MAX_INDICES, MAX_VERTS, VERTEX_STRIDE};
use cull::{frustum_planes, Culler};
pub use gamma::{unorm_of, GammaScope};
use lightmap::{LightmapRing, WorldLightmapExtra, LIGHTMAP_UBO_RING};
pub use lightmap::{WorldLightmapState, BLOCK_LIGHT_FACTOR};
use sky::{parse_fog_env, SkyGradient};
pub use sky::{SkyMode, SKY_HORIZON};
pub(crate) use textures::upload_texture_array;
pub use textures::LayerAnimation;
use textures::TextureArray;

/// An owned screen-space text line (the app fills these each frame; the
/// renderer borrows them into `TextLine` at draw time).
pub struct OwnedTextLine {
    pub x: f32,
    pub y: f32,
    pub px: f32,
    /// [`crate::text::TextLine::color_linear`] — **linear**, not the `/255` of
    /// a vanilla colour byte.
    ///
    /// Named for the space rather than called `color` because M130 found nine
    /// of the twenty-two callers handing over the byte: the pass writes this
    /// value straight into an **sRGB** attachment, so a `/255` renders a third
    /// of a stop bright and looks entirely plausible. A field called `color`
    /// admits both readings and cannot be got wrong loudly; this one at least
    /// says which it wants at the point of assignment.
    pub color_linear: [f32; 3],
    pub alpha: f32,
    /// [`crate::text::TextLine::shadow`] — deliberately **not** defaulted, so
    /// adding a caller is a decision rather than an omission. Everything but
    /// M79's XP level number passes `true`.
    pub shadow: bool,
    /// [`crate::text::TextLine::style`] — the five `Style` flags (M126c).
    pub style: crate::text::TextStyle,
    pub text: String,
}

pub const DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT;

#[repr(C)]
#[derive(Clone, Copy)]
struct WorldPush {
    view_proj: [[f32; 4]; 4],
    /// xyz camera world pos, w = fog start distance.
    cam_fog: [f32; 4],
    /// xyz fog color (linear = sky horizon), w = fog end distance.
    fog_col: [f32; 4],
    /// The scalar lightmap uniforms, packed to fill the spare push lanes:
    /// x = SkyFactor (time of day), y = BlockFactor, z = BrightnessFactor
    /// (gamma minus the darkness effect), w = DarknessScale. Vanilla scales
    /// only the sky half by the time of day, so a sunrise is one push, not a
    /// remesh.
    light: [f32; 4],
    /// xyz sky-light colour (white by day, blue at night); w = the
    /// night-vision factor (the ambient seed `max(black, nvColor*w)`).
    sky_col: [f32; 4],
}
// 128 bytes: exactly the push-constant size Vulkan guarantees. Anything
// further has to move into a uniform buffer — which is what M16's dimension
// ambient colour does (see `WorldLightmapExtra` / `LIGHTMAP_UBO_RING`).
const _: () = assert!(std::mem::size_of::<WorldPush>() == 128);
#[repr(C)]
#[derive(Clone, Copy)]
struct LinePush {
    view_proj: [[f32; 4]; 4],
    color: [f32; 4],
}

/// The leash ribbon's push (M170) — colour is per-vertex, so only the camera.
#[repr(C)]
#[derive(Clone, Copy)]
struct LeashPush {
    view_proj: [[f32; 4]; 4],
}

/// Ribbon vertex budget: ~13 leashes at 294 verts each (98 triangles). A
/// leash beyond the cap is dropped whole rather than torn.
const LEASH_MAX_VERTS: usize = 4096;
/// Which per-player atlas pool an eviction came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerTexture {
    Skin,
    Cape,
    Face,
}

/// The owner key the inventory preview's single player uploads under.
const PREVIEW_OWNER: u128 = 0;

pub struct WorldRenderer {
    // -- graphics --
    tex_set_layout: vk::DescriptorSetLayout,
    graphics_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    /// Blended, depth-write-off variant for the water pass.
    water_pipeline: vk::Pipeline,
    /// Fullscreen gradient sky (no descriptor set, no vertex input).
    sky_gradient: SkyGradient,
    /// Block-selection outline (LINE_LIST, depth-tested against terrain).
    select_layout: vk::PipelineLayout,
    select_pipeline: vk::Pipeline,
    select_ring: crate::buf_ring::BufRing,
    /// Leash ribbons (M170): a TRIANGLE_LIST of position+colour verts, world
    /// space, depth-tested against terrain and entities. Rebuilt each frame.
    leash_layout: vk::PipelineLayout,
    leash_pipeline: vk::Pipeline,
    leash_ring: crate::buf_ring::BufRing,
    leash_warned: bool,
    leash_verts: Vec<crate::leash::LeashVertex>,
    tex_pool: vk::DescriptorPool,
    /// One set per frame in flight: identical texture binding, but each points
    /// at its own `LightmapExtra` uniform slot (see [`LIGHTMAP_UBO_RING`]).
    tex_sets: [vk::DescriptorSet; LIGHTMAP_UBO_RING],
    /// The `LightmapExtra` uniform buffers (one per ring slot, host-mapped)
    /// and the cursor naming the slot this frame writes and binds.
    lm: LightmapRing,
    /// The block texture array, its sampler and the animated layers.
    textures: TextureArray,
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

/// One frame's GUI inputs, separate from the renderer's GPU resources.
/// Each field is written by a `WorldRenderer::set_*` call and read when
/// the frame is recorded.
#[derive(Default)]
struct GuiFrame {
    /// This frame's screen chrome. Empty = nothing to draw, which is the
    /// in-game case and also a screen that draws only text.
    screen_draw: crate::screen::ScreenDraw,
    /// Whether the screen is open, and which slot's GUI-space top-left the
    /// cursor is over.
    container_open: Option<Option<(i32, i32)>>,
    /// The open container's panel (M87). `None` means the player's own
    /// inventory, whose 176x166 panel the container pass draws from its own
    /// `inventory.png` rect.
    container_panel: Option<crate::container::ContainerPanel>,
    /// The recipe book (M94). Beside the panel, not inside it: the player's
    /// own inventory has no `ContainerPanel` and still has a book.
    recipe_book: Option<crate::container::RecipeBookPanel>,
    /// This frame's tooltip: the text block's GUI-space top-left and size,
    /// both measured by the caller (M40), plus the bundle grid inside it if the
    /// hovered stack produced one (M58). The pass owns no font.
    container_tooltip: Option<crate::container::TooltipDraw>,
    /// This frame's durability bars, in screen pixels (M41). Independent of
    /// the screen being open — a worn pickaxe shows one in the hotbar.
    item_bars: Vec<crate::container::ItemBar>,
    /// This frame's preview matrix and window, or `None` when it is closed.
    preview_state: Option<([[f32; 4]; 4], vk::Rect2D)>,
    /// This frame's hand view-projection, or `None` when nothing is drawn.
    hand_vp: Option<[[f32; 4]; 4]>,
    locator_state: Option<crate::locator_bar::LocatorBarState>,
    /// Live HUD state (health 0..20, food 0..20, selected slot 0..8); when
    /// `None`, no HUD draws (view/demo/bench aren't "playing").
    hud_state: Option<(u8, crate::hud::HudGauges)>,
    /// M168 — this frame's survival gauges (`survival_hud::layout`), in GUI
    /// pixels. Replaced every frame with the rest of `hud_state`.
    hud_survival: Vec<crate::hud::HudBlit>,
    /// This frame's chat backdrops (M109). A separate field from
    /// [`Self::hud_state`] because it is set on a different cadence — the HUD
    /// state comes from the packet stream and this comes from the chat store's
    /// own geometry — and because an empty slice is a meaningful value (no
    /// chat on screen) rather than an absent one.
    hud_fills: Vec<crate::hud::HudFill>,
    /// M151 — this frame's caller-placed HUD sprites (the tab list's ping
    /// icons). Separate from `hud_fills` because they name an atlas rect
    /// rather than a colour.
    hud_icons: Vec<crate::hud::HudBlit>,
    velvet_runs: Vec<crate::velvet_text::OwnedRun>,
    /// Screen-space text lines to draw this frame (chat, coords); empty →
    /// nothing (view/demo/bench never set them).
    text_lines: Vec<OwnedTextLine>,
}

impl WorldRenderer {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        tex_size: u32,
        layers: &[Vec<u8>],
    ) -> Result<Self, String> {
        unsafe {
            let textures = TextureArray::create(gpu, tex_size, layers)?;
            let device = gpu.device.clone();

            // ---- graphics descriptor + pipeline ----
            // Binding 0: the block texture array. Binding 1: the small
            // `LightmapExtra` UBO carrying the dimension ambient colour — the
            // push block is exactly full at 128 bytes, so this is where a new
            // canonical lightmap input has to live.
            let tex_bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                // Read by world.vert, which samples the lightmap per vertex.
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
            ];
            let tex_set_layout = device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&tex_bindings),
                    None,
                )
                .map_err(|e| format!("tex set layout: {e}"))?;
            let ring = LIGHTMAP_UBO_RING as u32;
            let tex_pool_sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .descriptor_count(ring),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(ring),
            ];
            let tex_pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(ring)
                        .pool_sizes(&tex_pool_sizes),
                    None,
                )
                .map_err(|e| format!("tex pool: {e}"))?;
            let tex_layouts = [tex_set_layout];
            let ring_layouts = [tex_set_layout; LIGHTMAP_UBO_RING];
            let allocated = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(tex_pool)
                        .set_layouts(&ring_layouts),
                )
                .map_err(|e| format!("tex set: {e}"))?;
            let tex_sets: [vk::DescriptorSet; LIGHTMAP_UBO_RING] =
                std::array::from_fn(|i| allocated[i]);

            let mut lm = LightmapRing::create(gpu)?;

            let image_info = [vk::DescriptorImageInfo::default()
                .sampler(textures.sampler)
                .image_view(textures.view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            // `buffer_info` slices must outlive the update call, so build them
            // all up front rather than inside the loop.
            let buf_infos: [[vk::DescriptorBufferInfo; 1]; LIGHTMAP_UBO_RING] =
                std::array::from_fn(|i| {
                    [vk::DescriptorBufferInfo::default()
                        .buffer(lm.bufs[i])
                        .offset(0)
                        .range(std::mem::size_of::<WorldLightmapExtra>() as u64)]
                });
            let mut writes: Vec<vk::WriteDescriptorSet> = Vec::new();
            for i in 0..LIGHTMAP_UBO_RING {
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(tex_sets[i])
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&image_info),
                );
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(tex_sets[i])
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                        .buffer_info(&buf_infos[i]),
                );
            }
            device.update_descriptor_sets(&writes, &[]);
            // Seed every slot with the default state so a first frame that
            // never reaches `draw` still binds initialized memory.
            lm.seed_defaults();

            // view_proj is read in the vertex stage, the fog fields in the
            // fragment stage — one range spanning both.
            let pc_ranges = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .offset(0)
                .size(std::mem::size_of::<WorldPush>() as u32)];
            let graphics_layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&tex_layouts)
                        .push_constant_ranges(&pc_ranges),
                    None,
                )
                .map_err(|e| format!("graphics layout: {e}"))?;
            let pipeline = build_graphics_pipeline(&device, graphics_layout, color_format, false)?;
            let water_pipeline =
                build_graphics_pipeline(&device, graphics_layout, color_format, true)?;

            let sky_gradient = SkyGradient::create(&device, color_format)?;

            // ---- selection outline (lines) ----
            let line_pc = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .offset(0)
                .size(std::mem::size_of::<LinePush>() as u32)];
            let select_layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&line_pc),
                    None,
                )
                .map_err(|e| format!("select layout: {e}"))?;
            let select_pipeline = build_line_pipeline(&device, select_layout, color_format)?;
            // ---- leash ribbon (M170) ----
            let leash_pc = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(std::mem::size_of::<LeashPush>() as u32)];
            let leash_layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&leash_pc),
                    None,
                )
                .map_err(|e| format!("leash layout: {e}"))?;
            let leash_pipeline = build_leash_pipeline(&device, leash_layout, color_format)?;
            // Both written from `draw`, so they cannot grow; sized for their
            // budgets up front.
            let leash_ring = crate::buf_ring::BufRing::with_capacity(
                gpu,
                "leash-ribbon",
                vk::BufferUsageFlags::VERTEX_BUFFER,
                (LEASH_MAX_VERTS * std::mem::size_of::<crate::leash::LeashVertex>()) as u64,
            )?;
            let select_ring = crate::buf_ring::BufRing::with_capacity(
                gpu,
                "selection-lines",
                vk::BufferUsageFlags::VERTEX_BUFFER,
                24 * 12, // 24 line verts × vec3
            )?;

            // ---- compute cull descriptor + pipeline ----
            let cull_pipeline = cull::create_cull_pipeline(&device)?;

            // ---- mega-buffers + metadata + indirect + count ----
            let mega = arena::create_mega_buffers(gpu)?;
            let cull_bufs = cull::create_cull_buffers(gpu)?;

            // Cull descriptor set (meta, indirect cmds, count).
            let cull_descriptors =
                cull::create_cull_descriptors(&device, &cull_pipeline, &cull_bufs)?;

            // ---- async upload ring ----
            let upload_ring = arena::create_upload_ring(gpu)?;
            let (count_readback, count_readback_alloc) = create_host_buffer(
                gpu,
                16,
                vk::BufferUsageFlags::TRANSFER_DST,
                "count-readback",
            )?;

            Ok(Self {
                velvet_text: None,
                gui: GuiFrame::default(),
                scene: SceneFrame {
                    camera_eye: [0.0; 3],
                    lightmap: WorldLightmapState::default(),
                    sky_tint: [1.0; 3],
                    fog_tint: [1.0; 3],
                    sky_base: SKY_HORIZON,
                    fog_base: SKY_HORIZON,
                    fog: parse_fog_env(),
                    sky_mode: SkyMode::default(),
                    celestial_state: crate::celestial::CelestialState::default(),
                    end_flash: None,
                    end_portal_time: 0.0,
                    selection: None,
                },
                tex_set_layout,
                graphics_layout,
                pipeline,
                water_pipeline,
                sky_gradient,
                select_layout,
                select_pipeline,
                select_ring,
                leash_layout,
                leash_pipeline,
                leash_ring,
                leash_warned: false,
                leash_verts: Vec::new(),
                tex_pool,
                tex_sets,
                lm,
                textures,
                arena: Arena::new(mega, upload_ring),
                evicted_textures: Vec::new(),
                culler: Culler::new(
                    cull_pipeline,
                    cull_bufs,
                    cull_descriptors,
                    (count_readback, count_readback_alloc),
                ),
                color_format,
                gamma: None,
                entities: None,
                celestial: None,
                end_sky: None,
                end_portal: None,
                clouds: None,
                border: None,
                weather: None,
                gui_items: None,
                gui_item_generation: 0,
                hand_generation: 0,
                container: None,
                screen: None,
                preview: None,
                hand: None,
                particles: None,
                crumbling: None,
                hud: None,
                locator: None,
                text: None,
                drawn_last_frame: 0,
                culled_last_frame: 0,
            })
        }
    }

    /// Set the frame's eye position — drives translucent back-to-front
    /// column sorting. Callers that never render water can skip it.
    pub fn set_camera(&mut self, eye: [f32; 3]) {
        self.scene.camera_eye = eye;
    }

    fn retire_end_portal(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.end_portal.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }
    fn retire_clouds(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.clouds.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }
    fn retire_border(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.border.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }
    fn retire_container(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.container.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }
    fn retire_weather(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.weather.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }
    fn retire_particles(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.particles.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }
    fn retire_crumbling(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.crumbling.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }
    fn retire_entities(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.entities.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }
    fn retire_hud(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.hud.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }
    fn retire_locator(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.locator.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }
    fn retire_text(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.text.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }
    fn retire_velvet_text(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.velvet_text.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    /// Attach the End skybox pass (M16), from the user's own client jar's
    /// `assets/minecraft/textures/environment/end_sky.png`. Without it,
    /// [`SkyMode::End`] draws nothing — `end_sky_ready` reports which.
    /// Build the end-portal pass from `environment/end_sky.png` and
    /// `entity/end_portal/end_portal.png` (M32). Without both, no portal draws.
    pub fn init_end_portal(
        &mut self,
        gpu: &mut Gpu,
        sky: &crate::end_portal::PortalImage,
        portal: &crate::end_portal::PortalImage,
    ) -> Result<(), String> {
        self.retire_end_portal(gpu);
        self.end_portal = Some(crate::end_portal::EndPortalPass::new(
            gpu,
            self.color_format,
            sky,
            portal,
        )?);
        Ok(())
    }

    pub fn end_portal_ready(&self) -> bool {
        self.end_portal.is_some()
    }

    /// Build the cloud pass (M33). No texture: `clouds.png` is decoded on the
    /// CPU into cell data, and the shader carries its six face colours inline.
    pub fn init_clouds(&mut self, gpu: &mut Gpu) -> Result<(), String> {
        self.retire_clouds(gpu);
        self.clouds = Some(crate::clouds::CloudPass::new(gpu, self.color_format)?);
        Ok(())
    }

    pub fn clouds_ready(&self) -> bool {
        self.clouds.is_some()
    }

    /// This frame's cloud deck. An empty face list draws nothing.
    pub fn set_clouds(
        &mut self,
        gpu: &mut Gpu,
        draw: &crate::clouds::CloudDraw,
    ) -> Result<(), String> {
        match self.clouds.as_mut() {
            Some(pass) => pass.set_draw(gpu, draw),
            None => Ok(()),
        }
    }

    /// Build the world-border pass (M80) over `misc/forcefield.png`.
    pub fn init_border(
        &mut self,
        gpu: &mut Gpu,
        tex: &crate::border::BorderImage,
    ) -> Result<(), String> {
        self.retire_border(gpu);
        self.border = Some(crate::border::BorderPass::new(gpu, self.color_format, tex)?);
        Ok(())
    }

    pub fn border_ready(&self) -> bool {
        self.border.is_some()
    }

    /// This frame's border wall. `None` clears it — a frame whose `extract`
    /// decided the wall is invisible must draw nothing rather than the last
    /// visible geometry.
    pub fn set_border(
        &mut self,
        gpu: &mut Gpu,
        draw: Option<&crate::border::BorderDraw>,
    ) -> Result<(), String> {
        match self.border.as_mut() {
            Some(pass) => pass.set_draw(gpu, draw),
            None => Ok(()),
        }
    }

    /// Build the GUI-item pass (M34) over an RGBA atlas the caller packs.
    pub fn init_gui_items(
        &mut self,
        gpu: &mut Gpu,
        atlas_rgba: &[u8],
        atlas_w: u32,
        atlas_h: u32,
    ) -> Result<(), String> {
        // Rebuildable: the atlas is repacked whenever the hotbar needs a
        // texture it does not hold. Same size (the usual case) updates the
        // texels in place; otherwise the old pass — raw handles, not RAII — is
        // retired through deferred destruction, never idled on a gameplay path.
        if let Some(p) = self.gui_items.as_mut() {
            if p.update_atlas(gpu, atlas_rgba, atlas_w, atlas_h)? {
                return Ok(());
            }
        }
        if let Some(mut old) = self.gui_items.take() {
            gpu.defer_destroy(move |g| old.destroy_now(g));
        }
        self.gui_items = Some(crate::gui_item::GuiItemPass::new(
            gpu,
            self.color_format,
            atlas_rgba,
            atlas_w,
            atlas_h,
        )?);
        self.gui_item_generation += 1;
        Ok(())
    }

    /// How many times the GUI-item pass has been (re)built.
    ///
    /// `live --render-check` needs it: a rebuild legitimately discards the
    /// whole ring — `GuiItemPass::destroy` idles first, so nothing in flight is
    /// reading it — and without this the gate would score that as the very
    /// use-after-free it is looking for. It scored exactly one such false
    /// orphan on the hand pass before this existed.
    pub fn gui_item_generation(&self) -> u64 {
        self.gui_item_generation
    }

    /// How many times the hand pass has been (re)built. See
    /// [`Self::gui_item_generation`].
    pub fn hand_generation(&self) -> u64 {
        self.hand_generation
    }

    /// Build the inventory-screen pass (M35).
    pub fn init_container(
        &mut self,
        gpu: &mut Gpu,
        sprites: &crate::container::ContainerSpriteData<'_>,
    ) -> Result<(), String> {
        self.retire_container(gpu);
        self.container = Some(crate::container::ContainerPass::new(
            gpu,
            self.color_format,
            sprites,
        )?);
        Ok(())
    }

    /// Build the screen-framework chrome pass (M82). Idempotent, so the app
    /// can call it the first time any screen opens rather than at startup.
    pub fn init_screen(
        &mut self,
        gpu: &mut Gpu,
        sprites: &crate::screen::WidgetSpriteData<'_>,
    ) -> Result<(), String> {
        if self.screen.is_some() {
            return Ok(());
        }
        self.screen = Some(crate::screen::ScreenPass::new(
            gpu,
            self.color_format,
            sprites,
        )?);
        Ok(())
    }

    pub fn screen_ready(&self) -> bool {
        self.screen.is_some()
    }

    /// This frame's screen chrome. `ScreenDraw::default()` draws nothing.
    pub fn set_screen(&mut self, draw: crate::screen::ScreenDraw) {
        self.gui.screen_draw = draw;
    }

    /// Build the preview's entity pass. Idempotent, and cheap to call every
    /// time the screen opens.
    pub fn init_preview(
        &mut self,
        gpu: &mut Gpu,
        font: Option<crate::entities::FontData<'_>>,
        tex: crate::entities::MobTextures<'_>,
    ) -> Result<(), String> {
        if self.preview.is_some() {
            return Ok(());
        }
        self.preview = Some(crate::entities::EntityPass::new(
            gpu,
            self.color_format,
            font,
            tex,
        )?);
        Ok(())
    }

    /// Build the first-person hand pass over an atlas holding the item
    /// textures and the player's skin (M38).
    pub fn init_hand(
        &mut self,
        gpu: &mut Gpu,
        atlas_rgba: &[u8],
        atlas_w: u32,
        atlas_h: u32,
    ) -> Result<(), String> {
        // Rebuildable, like the GUI-item atlas: the resident texture set
        // changes when the held item does. Same-size atlases update in place;
        // otherwise the old pass is retired through deferred destruction.
        if let Some(p) = self.hand.as_mut() {
            if p.update_atlas(gpu, atlas_rgba, atlas_w, atlas_h)? {
                return Ok(());
            }
        }
        if let Some(mut old) = self.hand.take() {
            gpu.defer_destroy(move |g| old.destroy_now(g));
        }
        self.hand = Some(crate::hand_pass::HandPass::new(
            gpu,
            self.color_format,
            atlas_rgba,
            atlas_w,
            atlas_h,
        )?);
        self.hand_generation += 1;
        Ok(())
    }

    pub fn hand_ready(&self) -> bool {
        self.hand.is_some()
    }

    /// Whether the vanilla-font text pass exists (`init_text`) — chat, the
    /// command line, screen labels. Without it `set_text` stores lines that
    /// nothing draws.
    pub fn text_ready(&self) -> bool {
        self.text.is_some()
    }

    /// The raw handle of the buffer the hand pass would bind this frame, or 0.
    ///
    /// For `live --render-check`'s rotation witnesses (M86): the whole point of
    /// [`crate::buf_ring::BufRing`] is that consecutive frames hand back
    /// *different* buffers, and that is only observable from outside if the
    /// handle is. Raw `u64` rather than `vk::Buffer` so the check does not have
    /// to reach into ash's types to compare two of them.
    pub fn hand_vertex_buffer(&self) -> u64 {
        use ash::vk::Handle;
        self.hand
            .as_ref()
            .and_then(|p| p.vertex_buffer())
            .map(|b| b.as_raw())
            .unwrap_or(0)
    }

    /// The raw handle of the buffer the GUI-item pass would bind this frame,
    /// or 0. See [`Self::hand_vertex_buffer`].
    pub fn gui_item_vertex_buffer(&self) -> u64 {
        use ash::vk::Handle;
        self.gui_items
            .as_ref()
            .and_then(|p| p.vertex_buffer())
            .map(|b| b.as_raw())
            .unwrap_or(0)
    }

    /// Every buffer the hand pass is keeping alive. See
    /// [`crate::buf_ring::BufRing::live`] for why the gate needs this rather
    /// than just the current handle.
    pub fn hand_live_buffers(&self) -> Vec<u64> {
        self.hand
            .as_ref()
            .map(|p| p.live_vertex_buffers())
            .unwrap_or_default()
    }

    /// Every buffer the GUI-item pass is keeping alive.
    pub fn gui_item_live_buffers(&self) -> Vec<u64> {
        self.gui_items
            .as_ref()
            .map(|p| p.live_vertex_buffers())
            .unwrap_or_default()
    }

    /// This frame's hand geometry and the matrix that projects it. An empty
    /// vertex list draws nothing, which is the scoping / hidden-hand case.
    pub fn set_hand(
        &mut self,
        gpu: &mut Gpu,
        verts: &[crate::gui_item::GuiItemVertex],
        view_proj: [[f32; 4]; 4],
    ) -> Result<(), String> {
        self.set_hand_with_glint(gpu, verts, &[], view_proj)
    }

    /// The hand plus this frame's enchantment glint (M44), in one buffer.
    pub fn set_hand_with_glint(
        &mut self,
        gpu: &mut Gpu,
        verts: &[crate::gui_item::GuiItemVertex],
        glint: &[crate::gui_item::GuiItemVertex],
        view_proj: [[f32; 4]; 4],
    ) -> Result<(), String> {
        self.gui.hand_vp = (!verts.is_empty()).then_some(view_proj);
        match self.hand.as_mut() {
            Some(p) => p.set_vertices_with_glint(gpu, verts, glint),
            None => Ok(()),
        }
    }

    /// Build the hand glint's pipeline and upload its sheet. Goes with
    /// `init_hand`, which is rebuilt whenever the held item's atlas changes.
    pub fn init_hand_glint(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> Result<(), String> {
        // M50: the glint renders into the UNORM view, so its pipeline must
        // declare that format. No counterpart format means no gamma-space
        // target, and the glint is dropped rather than blended in linear light.
        let Some(format) = unorm_of(self.color_format) else {
            return Ok(());
        };
        match self.hand.as_mut() {
            Some(p) => p.init_glint(gpu, rgba, w, h, format),
            None => Ok(()),
        }
    }

    pub fn preview_ready(&self) -> bool {
        self.preview.is_some()
    }

    /// This frame's preview: the model to pose, the matrix that places it, and
    /// the window it is clipped to. `None` closes it.
    pub fn set_preview(&mut self, draw: Option<(&EntityDraw<'_>, [[f32; 4]; 4], vk::Rect2D)>) {
        match (draw, self.preview.as_mut()) {
            (Some((d, vp, rect)), Some(pass)) => {
                // The camera basis only orients billboards — nametags — and the
                // preview draws none, so any orthonormal pair will do.
                pass.set_draws(
                    std::slice::from_ref(d),
                    &[],
                    &[],
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    0.0,
                    [0.0, 0.0, 0.0],
                );
                self.gui.preview_state = Some((vp, rect));
            }
            _ => self.gui.preview_state = None,
        }
    }

    pub fn container_ready(&self) -> bool {
        self.container.is_some()
    }

    /// Open or close the screen for this frame. `hovered` is the GUI-space
    /// top-left of the slot under the cursor, or `None`.
    /// The open container's panel, or `None` for the player's own inventory.
    ///
    /// Separate from [`Self::set_container`] because the two answer different
    /// questions — that one is *whether* a screen is open and what is hovered,
    /// this one is *which* menu — and a container can open and close without
    /// the hover changing.
    pub fn set_container_panel(&mut self, panel: Option<crate::container::ContainerPanel>) {
        self.gui.container_panel = panel;
    }

    /// The panel height the renderer is actually holding, if a container's.
    ///
    /// Read back rather than inferred from the open menu's layout: a fallback
    /// in the panel builder would leave the layout saying 168 while the
    /// renderer drew the player's 166, and a witness that asked the layout
    /// could not tell those apart. This is what the draw uses.
    pub fn container_panel_height(&self) -> Option<f32> {
        self.gui.container_panel.as_ref().map(|p| p.gui_h)
    }

    /// How many overlay sprites this frame's container panel carries (M92).
    ///
    /// Read back OUT of the renderer for M88's reason: asking the open menu
    /// what it *should* draw answers from the model, and the question is
    /// whether the windowed frame loop reached the builder at all.
    pub fn container_panel_overlays(&self) -> usize {
        self.gui
            .container_panel
            .as_ref()
            .map_or(0, |p| p.overlays.len())
    }

    /// How many quads this frame's RECIPE BOOK carries — its panel plus its
    /// sprites — or 0 for a shut book (M94).
    ///
    /// Read out of the renderer for the same reason as the two above: asking
    /// the session whether the book *should* be open answers from the model,
    /// and the question is whether the windowed frame loop reached the book's
    /// builder at all. That is the M86 gap, one feature over.
    pub fn container_panel_book_quads(&self) -> usize {
        self.gui
            .recipe_book
            .as_ref()
            .map_or(0, |b| b.blits.len() + b.overlays.len())
    }

    pub fn set_recipe_book(&mut self, book: Option<crate::container::RecipeBookPanel>) {
        self.gui.recipe_book = book;
    }

    pub fn set_container(&mut self, open: bool, hovered: Option<(i32, i32)>) {
        self.gui.container_open = open.then_some(hovered);
    }

    /// This frame's tooltip, or `None` for no tooltip. The caller measures it
    /// with [`crate::container::tooltip_size`] and places it with
    /// [`crate::container::tooltip_position`]; only the sprites live here,
    /// because the text — the lines, the `+N` badge and the bar's label — goes
    /// through the ordinary text pass.
    pub fn set_container_tooltip(&mut self, tooltip: Option<crate::container::TooltipDraw>) {
        self.gui.container_tooltip = tooltip;
    }

    /// This frame's durability bars (M41). The caller decides which stacks
    /// have one — `isBarVisible` needs the item's max damage, which is not on
    /// the wire.
    pub fn set_item_bars(&mut self, bars: Vec<crate::container::ItemBar>) {
        self.gui.item_bars = bars;
    }

    pub fn gui_items_ready(&self) -> bool {
        self.gui_items.is_some()
    }

    /// This frame's item icons, already placed and shaded by
    /// `gui_item::build_vertices`.
    pub fn set_gui_items(
        &mut self,
        gpu: &mut Gpu,
        verts: &[crate::gui_item::GuiItemVertex],
    ) -> Result<(), String> {
        match self.gui_items.as_mut() {
            Some(p) => p.set_vertices(gpu, verts),
            None => Ok(()),
        }
    }

    /// The icons plus this frame's enchantment glint (M43), which rides in the
    /// same buffer after them.
    pub fn set_gui_items_with_glint(
        &mut self,
        gpu: &mut Gpu,
        verts: &[crate::gui_item::GuiItemVertex],
        glint: &[crate::gui_item::GuiItemVertex],
    ) -> Result<(), String> {
        match self.gui_items.as_mut() {
            Some(p) => p.set_vertices_with_glint(gpu, verts, glint),
            None => Ok(()),
        }
    }

    /// Build the glint's pipeline and upload `misc/enchanted_glint_item.png`.
    ///
    /// The item pass is rebuilt whenever its atlas is repacked, and this goes
    /// with it — hence the call site being beside `init_gui_items` rather than
    /// once at startup.
    pub fn init_gui_glint(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> Result<(), String> {
        // M50: the glint renders into the UNORM view, so its pipeline must
        // declare that format. No counterpart format means no gamma-space
        // target, and the glint is dropped rather than blended in linear light.
        let Some(format) = unorm_of(self.color_format) else {
            return Ok(());
        };
        match self.gui_items.as_mut() {
            Some(p) => p.init_glint(gpu, rgba, w, h, format),
            None => Ok(()),
        }
    }

    /// Build the weather pass (M33) from `rain.png` and `snow.png`.
    pub fn init_weather(
        &mut self,
        gpu: &mut Gpu,
        rain: &crate::weather::WeatherImage,
        snow: &crate::weather::WeatherImage,
    ) -> Result<(), String> {
        self.retire_weather(gpu);
        self.weather = Some(crate::weather::WeatherPass::new(
            gpu,
            self.color_format,
            rain,
            snow,
        )?);
        Ok(())
    }

    pub fn weather_ready(&self) -> bool {
        self.weather.is_some()
    }

    /// This frame's rain and snow geometry.
    pub fn set_weather(
        &mut self,
        gpu: &mut Gpu,
        draw: &crate::weather::WeatherDraw,
    ) -> Result<(), String> {
        match self.weather.as_mut() {
            Some(pass) => pass.set_draw(gpu, draw),
            None => Ok(()),
        }
    }

    /// Build the particle pass (M37) from the particle atlas.
    pub fn init_particles(
        &mut self,
        gpu: &mut Gpu,
        atlas: &crate::particles::ParticleAtlas,
    ) -> Result<(), String> {
        self.retire_particles(gpu);
        self.particles = Some(crate::particles::ParticlePass::new(
            gpu,
            self.color_format,
            atlas,
        )?);
        Ok(())
    }

    pub fn particles_ready(&self) -> bool {
        self.particles.is_some()
    }

    /// This frame's particle geometry.
    pub fn set_particles(
        &mut self,
        gpu: &mut Gpu,
        draw: &crate::particles::ParticleDraw,
    ) -> Result<(), String> {
        match self.particles.as_mut() {
            Some(pass) => pass.set_draw(gpu, draw),
            None => Ok(()),
        }
    }

    /// Build the block-break crumbling pass (M81) from the ten
    /// `destroy_stage_N` textures.
    ///
    /// Built against **`unorm_of(self.color_format)`**, not the format
    /// itself: the pass's `2·src·dst` blend has to run on gamma-encoded
    /// numbers, so it is drawn inside [`Self::with_gamma_space`] and its
    /// pipeline must declare the format that scope reopens with. A pass built
    /// against the sRGB format and drawn there is a validation error, not a
    /// colour shift — the same contract M50's glint records.
    pub fn init_crumbling(
        &mut self,
        gpu: &mut Gpu,
        stages: &[Vec<u8>],
        size: u32,
    ) -> Result<(), String> {
        let Some(format) = unorm_of(self.color_format) else {
            log::warn!(
                "world: no UNORM counterpart for {:?} — no block-break overlay",
                self.color_format
            );
            return Ok(());
        };
        self.retire_crumbling(gpu);
        self.crumbling = Some(crate::crumbling::CrumblingPass::new(
            gpu, format, stages, size,
        )?);
        Ok(())
    }

    pub fn crumbling_ready(&self) -> bool {
        self.crumbling.is_some()
    }

    /// This frame's block-break decal geometry.
    pub fn set_crumbling(
        &mut self,
        gpu: &mut Gpu,
        verts: &[crate::crumbling::CrumblingVertex],
    ) -> Result<(), String> {
        match self.crumbling.as_mut() {
            Some(pass) => pass.set_verts(gpu, verts),
            None => Ok(()),
        }
    }

    /// This frame's portals. `game_time` is the raw world clock; the pass
    /// converts it to the daily fraction the shader expects.
    pub fn set_end_portals(
        &mut self,
        gpu: &mut Gpu,
        draws: &[crate::end_portal::PortalDraw],
        game_time: i64,
    ) -> Result<(), String> {
        self.scene.end_portal_time = crate::end_portal::game_time_fraction(game_time);
        match self.end_portal.as_mut() {
            Some(p) => p.set_draws(gpu, draws),
            None => Ok(()),
        }
    }

    /// Attach the entity pass (mob models / capsules + nametags).
    /// Callers that never attach it (demo/view/bench snapshots) pay
    /// nothing. Mobs whose textures are missing fall back to capsules.
    /// Build the entity glint's pipeline and upload its sheet (M45).
    pub fn init_entity_glint(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> Result<(), String> {
        // M50: the glint renders into the UNORM view, so its pipeline must
        // declare that format. No counterpart format means no gamma-space
        // target, and the glint is dropped rather than blended in linear light.
        let Some(format) = unorm_of(self.color_format) else {
            return Ok(());
        };
        match self.entities.as_mut() {
            Some(p) => p.init_glint(gpu, rgba, w, h, format),
            None => Ok(()),
        }
    }

    /// Vertices in the worn-armour foil range after the last `set_entities`
    /// (M50). `0` with no entity pass. See
    /// [`crate::entities::EntityPass::armor_glint_vertex_count`].
    pub fn armor_glint_vertex_count(&self) -> u32 {
        self.entities
            .as_ref()
            .map(|p| p.armor_glint_vertex_count())
            .unwrap_or(0)
    }

    /// Upload `misc/enchanted_glint_armor.png` for the worn-armour foil (M50).
    /// Separate from [`Self::init_entity_glint`] because it is a separate
    /// sheet; they share the pipeline underneath.
    pub fn init_entity_armor_glint(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> Result<(), String> {
        // M50: the glint renders into the UNORM view, so its pipeline must
        // declare that format. No counterpart format means no gamma-space
        // target, and the glint is dropped rather than blended in linear light.
        let Some(format) = unorm_of(self.color_format) else {
            return Ok(());
        };
        match self.entities.as_mut() {
            Some(p) => p.init_armor_glint(gpu, rgba, w, h, format),
            None => Ok(()),
        }
    }

    pub fn init_entities(
        &mut self,
        gpu: &mut Gpu,
        font: Option<FontData<'_>>,
        tex: MobTextures<'_>,
    ) -> Result<(), String> {
        self.retire_entities(gpu);
        self.entities = Some(EntityPass::new(gpu, self.color_format, font, tex)?);
        Ok(())
    }

    /// Install the baked held-item models (M22). No-op before
    /// `init_entities`, which is the serverless-still case.
    pub fn set_held_items(&mut self, items: crate::held::HeldItems) {
        if let Some(e) = self.entities.as_mut() {
            e.set_held_items(items);
        }
    }

    /// Page in the atlas textures the named held items need, before the frame
    /// is built (M22). Uploading needs the device, and only the handful of
    /// items actually on screen should occupy the pool.
    /// The width of a string in font pixels — `Font.width`. Sign text centres
    /// on it, so the caller needs it before building a draw.
    pub fn text_width(&self, text: &str) -> f32 {
        self.entities.as_ref().map_or(0.0, |p| p.text_width(text))
    }

    /// The font's per-glyph advances, for callers that must lay text out
    /// before a draw exists — sign line breaking runs against the board width
    /// in these units (M27).
    ///
    /// Handed out rather than wrapped in a `split` method because the break
    /// rule is `StringSplitter`'s, which is vanilla *data* logic and lives in
    /// `rewo-data` beside the dye table; this crate owns the glyphs, not the
    /// rule.
    pub fn font_advance(&self) -> Option<&[u8; 256]> {
        self.entities.as_ref().map(|p| p.font_advance())
    }

    pub fn prepare_held_items(&mut self, gpu: &mut Gpu, names: &[&str]) -> Result<(), String> {
        match self.entities.as_mut() {
            Some(e) => e.prepare_held_items(gpu, names),
            None => Ok(()),
        }
    }

    /// `init_entities` with resource-pack CEM model overrides (M9).
    pub fn init_entities_with_cem(
        &mut self,
        gpu: &mut Gpu,
        font: Option<FontData<'_>>,
        tex: MobTextures<'_>,
        cem: std::collections::HashMap<crate::mobs::EntityModelKind, crate::mobs::Model>,
    ) -> Result<(), String> {
        self.retire_entities(gpu);
        self.entities = Some(EntityPass::new_with_cem(
            gpu,
            self.color_format,
            font,
            tex,
            cem,
        )?);
        Ok(())
    }

    /// The attached entity pass, if any — `rewo mobshot` uses it to iterate
    /// available mob models and their geometric ground truth.
    pub fn entity_pass(&self) -> Option<&EntityPass> {
        self.entities.as_ref()
    }

    /// [`Self::upload_player_skin`], into the **preview** pass (M36).
    ///
    /// A separate call because the two passes hold separate atlases, so a UV
    /// from one means nothing in the other — uploading once and reusing the
    /// offset would put a slice of some mob's texture on the player.
    pub fn upload_preview_skin(&mut self, gpu: &mut Gpu, rgba: &[u8]) -> Option<[f32; 2]> {
        let pass = self.preview.as_mut()?;
        // The preview draws one player, the local one: a fixed owner, so a
        // re-upload reuses its slot and nothing is ever evicted.
        match pass.upload_skin(gpu, PREVIEW_OWNER, rgba) {
            Ok((uv, _)) => Some(uv),
            Err(e) => {
                log::warn!("preview: skin upload failed: {e}");
                None
            }
        }
    }

    /// Put one permuted armour-trim sprite in the entity atlas's trim pool,
    /// or return where it already is (M48).
    pub fn upload_entity_trim(
        &mut self,
        gpu: &mut Gpu,
        key: &str,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> Option<(u32, u32)> {
        self.entities.as_mut()?.upload_trim(gpu, key, rgba, w, h)
    }

    /// Upload player `owner`'s 64x64 skin into the entity atlas and return the
    /// UV offset for its `EntityDraw::skin_uv`. `None` if the entity pass
    /// isn't initialized, or every skin slot is drawn this frame. A player
    /// whose slot this took is queued for [`Self::take_evicted_textures`].
    pub fn upload_player_skin(
        &mut self,
        gpu: &mut Gpu,
        owner: u128,
        rgba: &[u8],
    ) -> Option<[f32; 2]> {
        let pass = self.entities.as_mut()?;
        match pass.upload_skin(gpu, owner, rgba) {
            Ok((uv, evicted)) => {
                self.evicted_textures
                    .extend(evicted.map(|o| (o, PlayerTexture::Skin)));
                Some(uv)
            }
            Err(e) => {
                log::warn!("entities: skin upload failed: {e}");
                None
            }
        }
    }

    /// Claim a tab-list face slot and upload one player's 16x8 face strip
    /// (M155). `None` when there is no HUD pass, matching the four uploads
    /// around it.
    pub fn upload_tab_face(&mut self, gpu: &mut Gpu, owner: u128, rgba: &[u8]) -> Option<u8> {
        match self.hud.as_mut()?.upload_face(gpu, owner, rgba) {
            Ok((slot, evicted)) => {
                self.evicted_textures
                    .extend(evicted.map(|o| (o, PlayerTexture::Face)));
                Some(slot)
            }
            Err(e) => {
                log::warn!("hud: tab face upload failed: {e}");
                None
            }
        }
    }

    /// Claim a cape slot and upload one player's cape sheet (M60). Returns
    /// the slot's atlas origin in texels — see `EntityPass::upload_cape` for
    /// why this is an origin and not a UV delta.
    pub fn upload_player_cape(
        &mut self,
        gpu: &mut Gpu,
        owner: u128,
        rgba: &[u8],
    ) -> Option<(u32, u32)> {
        let pass = self.entities.as_mut()?;
        match pass.upload_cape(gpu, owner, rgba) {
            Ok((o, evicted)) => {
                self.evicted_textures
                    .extend(evicted.map(|o| (o, PlayerTexture::Cape)));
                Some(o)
            }
            Err(e) => {
                log::warn!("entities: cape upload failed: {e}");
                None
            }
        }
    }

    /// The same, into the **preview** pass (M64).
    ///
    /// A separate call for the reason [`Self::upload_preview_skin`] is one:
    /// the two passes hold separate atlases, so an origin from the world's is
    /// a rectangle of some mob's texture in the preview's. This is stronger
    /// than the skin case, not weaker — an origin is an absolute texel
    /// address rather than a delta, so reusing one would sample a *fixed*
    /// wrong place rather than merely a shifted one.
    pub fn upload_preview_cape(&mut self, gpu: &mut Gpu, rgba: &[u8]) -> Option<(u32, u32)> {
        let pass = self.preview.as_mut()?;
        match pass.upload_cape(gpu, PREVIEW_OWNER, rgba) {
            Ok((o, _)) => Some(o),
            Err(e) => {
                log::warn!("preview: cape upload failed: {e}");
                None
            }
        }
    }

    /// Attach the in-game HUD pass (crosshair/hotbar/hearts/hunger).
    pub fn init_hud(&mut self, gpu: &mut Gpu, sprites: &HudSpritesData<'_>) -> Result<(), String> {
        self.retire_hud(gpu);
        self.hud = Some(HudPass::new(gpu, self.color_format, sprites)?);
        Ok(())
    }

    /// Set this frame's HUD state: the selected slot 0..8, M79's two gauges,
    /// and M168's survival gauges (`survival_hud::layout`'s output, in GUI
    /// pixels). Never called → no HUD (view/demo/bench).
    pub fn set_hud(
        &mut self,
        slot: u8,
        gauges: crate::hud::HudGauges,
        survival: Vec<crate::hud::HudBlit>,
    ) {
        self.gui.hud_state = Some((slot, gauges));
        self.gui.hud_survival = survival;
    }

    /// Set this frame's chat backdrops (M109), in GUI pixels.
    ///
    /// **Replaces**, and an empty slice is how chat says it has nothing to
    /// draw — a stale backdrop under nothing would be a black bar hanging over
    /// the world after the last message faded.
    pub fn set_hud_fills(&mut self, rects: Vec<crate::hud::HudFill>) {
        self.gui.hud_fills = rects;
    }

    /// Set this frame's caller-placed HUD sprites (M151), in GUI pixels.
    ///
    /// **Replaces**, for [`Self::set_hud_fills`]'s reason: the tab list exists
    /// only while its key is held, and a stale list would leave six ping icons
    /// floating over the world the moment it is let go.
    pub fn set_hud_icons(&mut self, icons: Vec<crate::hud::HudBlit>) {
        self.gui.hud_icons = icons;
    }

    /// Attach the locator bar (M83). Independent of `init_hud` because its
    /// sprites bake separately — see `rewo_data::assets::LocatorSprites`.
    pub fn init_locator_bar(
        &mut self,
        gpu: &mut Gpu,
        sprites: &crate::locator_bar::LocatorSpritesData<'_>,
    ) -> Result<(), String> {
        self.retire_locator(gpu);
        self.locator = Some(crate::locator_bar::LocatorBarPass::new(
            gpu,
            self.color_format,
            sprites,
        )?);
        Ok(())
    }

    /// Set (or clear) this frame's locator-bar draw list. `None` is the
    /// contextual bar *not* being the locator bar, which is a different thing
    /// from an empty marker list: `Some(state)` with no markers still paints
    /// the 182x5 strip, because `extractBackground` and `extractRenderState`
    /// are separate calls and only the second one loops.
    pub fn set_locator_bar(&mut self, state: Option<crate::locator_bar::LocatorBarState>) {
        self.gui.locator_state = state;
    }

    /// Attach the screen-space text pass (chat + coords overlay).
    pub fn init_text(&mut self, gpu: &mut Gpu, font: &FontData<'_>) -> Result<(), String> {
        self.retire_text(gpu);
        self.text = Some(TextPass::new(gpu, self.color_format, font)?);
        Ok(())
    }

    /// Replace this frame's screen-space text lines (chat, coords).
    /// Build the Velvet text pass against this target's **UNORM twin**.
    ///
    /// Not the sRGB format: the pass draws inside [`Self::with_gamma_space`],
    /// and Vulkan requires a pipeline's rendering formats to match the
    /// attachment. A target with no mutable-format twin gets no Velvet text at
    /// all rather than text blended in the wrong space.
    pub fn init_velvet_text(
        &mut self,
        gpu: &mut Gpu,
        cache: &crate::velvet_glyph::GlyphCache,
    ) -> Result<(), String> {
        let Some(format) = unorm_of(self.color_format) else {
            return Err("velvet text: target has no UNORM twin".into());
        };
        self.retire_velvet_text(gpu);
        self.velvet_text = Some(crate::velvet_text::VelvetTextPass::new(gpu, format, cache)?);
        Ok(())
    }

    /// Re-upload the glyph atlas if the cache has grown or been written to.
    /// Must run outside a rendering scope — it records a transfer.
    pub fn sync_velvet_atlas(
        &mut self,
        gpu: &mut Gpu,
        cache: &mut crate::velvet_glyph::GlyphCache,
    ) -> Result<(), String> {
        match self.velvet_text.as_mut() {
            Some(p) => p.sync_atlas(gpu, cache),
            None => Ok(()),
        }
    }

    pub fn set_velvet_runs(&mut self, runs: Vec<crate::velvet_text::OwnedRun>) {
        self.gui.velvet_runs = runs;
    }

    pub fn set_text(&mut self, lines: Vec<OwnedTextLine>) {
        self.gui.text_lines = lines;
    }

    /// Rebuild this frame's entity geometry (no-op until `init_entities`).
    /// `time` (seconds) drives the ambient mob animations.
    pub fn set_entities(
        &mut self,
        draws: &[EntityDraw<'_>],
        cam_right: [f32; 3],
        cam_up: [f32; 3],
        time: f32,
    ) {
        self.set_entities_and_block_entities(draws, &[], &[], cam_right, cam_up, time);
    }

    /// [`Self::set_entities`] plus the frame's block entities (M25b). They
    /// share the entity pass's buffer, atlas and shader — a chest is textured
    /// geometry at a world position, which is what that pass already draws.
    pub fn set_entities_and_block_entities(
        &mut self,
        draws: &[EntityDraw<'_>],
        block_entities: &[crate::entities::BlockEntityDraw<'_>],
        world_text: &[crate::entities::WorldTextDraw<'_>],
        cam_right: [f32; 3],
        cam_up: [f32; 3],
        time: f32,
    ) {
        if let Some(pass) = self.entities.as_mut() {
            pass.set_draws(
                draws,
                block_entities,
                world_text,
                cam_right,
                cam_up,
                time,
                self.scene.camera_eye,
            );
        }
    }

    /// Players whose skin, cape or tab face lost its atlas slot to another
    /// player's upload since the last call (least recently drawn first to go;
    /// a slot drawn in the current frame is never taken). Each one's stored
    /// address now points at someone else's texture: drop it, and upload that
    /// player's texture again when it is next needed.
    pub fn take_evicted_textures(&mut self) -> Vec<(u128, PlayerTexture)> {
        std::mem::take(&mut self.evicted_textures)
    }

    /// Set the targeted block for the selection outline (`None` = hidden).
    pub fn set_selection(&mut self, block: Option<[i32; 3]>) {
        self.scene.selection = block;
    }

    /// Push the world/fog constants (view_proj + camera + fog band).
    fn push_world(&self, device: &ash::Device, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4]) {
        let (light, sky_col) = self.scene.lightmap.push_words();
        let push = WorldPush {
            view_proj,
            cam_fog: [
                self.scene.camera_eye[0],
                self.scene.camera_eye[1],
                self.scene.camera_eye[2],
                self.scene.fog[0],
            ],
            fog_col: [
                self.scene.fog_base[0] * self.scene.fog_tint[0],
                self.scene.fog_base[1] * self.scene.fog_tint[1],
                self.scene.fog_base[2] * self.scene.fog_tint[2],
                self.scene.fog[1],
            ],
            light,
            sky_col,
        };
        unsafe {
            device.cmd_push_constants(
                cb,
                self.graphics_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                std::slice::from_raw_parts(
                    (&push as *const WorldPush) as *const u8,
                    std::mem::size_of::<WorldPush>(),
                ),
            );
        }
    }

    /// In-pass draws, in blend-correct order: gradient sky (background) →
    /// opaque terrain (indirect) → opaque entity capsules/models →
    /// translucent water (CPU-sorted back-to-front) → nametag text.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        // Advance the `LightmapExtra` ring once per frame and publish this
        // frame's ambient colour into the slot the terrain/water draws will
        // bind. Done here (not in `set_lightmap_state`) so exactly one slot is
        // consumed per frame regardless of how often the state was set.
        self.lm.advance(&self.scene.lightmap);

        self.draw_sky_pass(gpu, cb, view_proj, extent);
        if self.culler.column_count > 0 {
            self.draw_terrain(gpu, cb, view_proj, extent);
        }
        // The block-break crack (M81), immediately after the terrain whose
        // depth it re-tests against and whose colour it multiplies. Before the
        // entities, so a mob standing in front of a cracked block covers the
        // crack rather than being darkened by it. In gamma space, because
        // `2·src·dst` is not invariant under the sRGB transfer function.
        if let Some(pass) = &self.crumbling {
            if !pass.is_empty() {
                self.in_gamma_space(gpu, cb, extent, || pass.draw(gpu, cb, view_proj, extent));
            }
        }
        // Selection outline over the terrain (depth-tested against it).
        self.draw_leash(gpu, cb, view_proj, extent);
        self.draw_selection(gpu, cb, view_proj, extent);
        if let Some(pass) = &self.entities {
            pass.draw_solid(gpu, cb, view_proj, extent);
            // M52: the vanilla emissive mob layers (mob eyes, the warden's
            // glow) ride straight on top of the solid models — that is where
            // vanilla's `order(1)` layer submits land, under everything below.
            pass.draw_emissive(gpu, cb, view_proj, extent);
            // M50: the worn-armour foil, then the trim. All three depth-test
            // `EQUAL` against the solid geometry just written, so all three
            // must follow it — and the foil precedes the trim because
            // `renderLayers` submits layer, foil, trim in that order and
            // `SubmitNodeStorage` drains its phases in ascending `order`. A
            // trim's opaque texels therefore paint over the foil.
            // Both foils blend in gamma space; the trim between them is an
            // ordinary alpha blend and stays in the sRGB scope.
            self.in_gamma_space(gpu, cb, extent, || {
                pass.draw_armor_glint(gpu, cb, view_proj, extent)
            });
            pass.draw_trim(gpu, cb, view_proj, extent);
            // The glint sits on the solid geometry it just wrote — its depth
            // test is `EQUAL`, so it has to follow, and it precedes the
            // translucent passes because it is additive over opaque pixels.
            self.in_gamma_space(gpu, cb, extent, || {
                pass.draw_glint(gpu, cb, view_proj, extent)
            });
        }
        // End portals (M32). After solid geometry and before the translucent
        // pass: the shader writes opaque pixels and depth, so it belongs with
        // the solids, and drawing it after them lets terrain in front occlude
        // it exactly as vanilla's depth-tested pipeline does.
        if let Some(pass) = &self.end_portal {
            pass.draw(gpu, cb, view_proj, self.scene.end_portal_time, extent);
        }
        // Clouds (M33) sit between the solids and the translucent pass: they
        // are translucent themselves and must blend over terrain, but they are
        // also depth-writing, so water in front of them still sorts correctly.
        if let Some(pass) = &self.clouds {
            pass.draw(gpu, cb, view_proj, extent);
        }
        self.draw_translucent(gpu, cb, view_proj, extent);
        // Particles (M37) sit with the other blended, depth-tested,
        // non-depth-writing passes — after everything solid and translucent so
        // terrain occludes them, before weather so rain layers over smoke.
        if let Some(pass) = &self.particles {
            let (light, sky_col) = self.scene.lightmap.push_words();
            pass.draw(
                gpu,
                cb,
                view_proj,
                (light, sky_col, self.scene.lightmap.extra_words()),
                extent,
            );
        }
        // Rain and snow come after everything solid and translucent, blended
        // and depth-tested but not depth-writing — vanilla's
        // `WEATHER_NO_DEPTH_WRITE` branch.
        if let Some(pass) = &self.weather {
            let (light, sky_col) = self.scene.lightmap.push_words();
            pass.draw(
                gpu,
                cb,
                view_proj,
                (light, sky_col, self.scene.lightmap.extra_words()),
                extent,
            );
        }
        // The world border (M80) draws in vanilla's `weather` frame-graph pass,
        // immediately after the rain and snow — and unlike them it *writes*
        // depth, so it must come after everything it should not be occluded by.
        if let Some(pass) = &self.border {
            pass.draw(gpu, cb, view_proj, extent);
        }
        if let Some(pass) = &self.entities {
            pass.draw_text(gpu, cb, view_proj, extent);
        }
        // The first-person hand, after the world and before the HUD.
        //
        // Vanilla clears the depth buffer first, which is what stops a wall a
        // block in front of you from cutting your arm in half — the hand is
        // drawn in view space at a fraction of a block, well inside anything
        // the world put there.
        if let (Some(pass), Some(vp)) = (&self.hand, self.gui.hand_vp) {
            clear_depth_rect(gpu, cb, vk::Rect2D::default().extent(extent));
            pass.draw(gpu, cb, vp, extent);
            self.in_gamma_space(gpu, cb, extent, || pass.draw_glint(gpu, cb, vp, extent));
        }
        // HUD, then the inventory screen over it, then text — all screen
        // space. The item icons are one pass drawn once, so when the screen is
        // open they belong to it rather than to the hotbar: the caller feeds
        // `set_gui_items` the screen's 46 slot rects instead of the hotbar's
        // nine, and this order puts them between the two highlight halves,
        // where vanilla puts them.
        let screen = self.gui.container_open.filter(|_| self.container.is_some());
        // The container pass owns the durability bars whether or not the
        // screen is open, because the hotbar draws them too — so its geometry
        // is built unconditionally and only the panel is gated on `open`.
        if let Some(pass) = self.container.as_mut() {
            pass.set_state(
                extent,
                screen.is_some(),
                screen.flatten(),
                self.gui.container_tooltip.as_ref(),
                &self.gui.item_bars,
                self.gui.container_panel.as_ref(),
                self.gui.recipe_book.as_ref(),
            );
        }
        if let (Some(hud), Some((slot, gauges))) = (self.hud.as_mut(), self.gui.hud_state) {
            hud.draw(
                gpu,
                cb,
                extent,
                &self.gui.hud_survival,
                slot,
                gauges,
                &self.gui.hud_fills,
                &self.gui.hud_icons,
            );
            if screen.is_none() {
                // M83 — the contextual bar's slot, which is the XP bar's slot:
                // `Hud.nextContextualInfoState` picks one of them and the
                // caller has already made that choice (`set_locator_bar(None)`
                // when the XP bar won). Behind the hotbar icons for the same
                // reason the XP bar is: it is background chrome.
                if let (Some(pass), Some(state)) =
                    (self.locator.as_mut(), self.gui.locator_state.as_ref())
                {
                    pass.draw(gpu, cb, extent, state);
                }
                if let Some(items) = &self.gui_items {
                    items.draw(gpu, cb, extent);
                    self.in_gamma_space(gpu, cb, extent, || items.draw_glint(gpu, cb, extent));
                }
                // A hotbar bar goes over its icon, same as in the screen.
                if let Some(pass) = &self.container {
                    pass.draw_bars(gpu, cb, extent);
                }
            }
        }
        if let (Some(pass), Some(_)) = (self.container.as_ref(), screen) {
            pass.draw_back(gpu, cb, extent);
            // The player preview sits between the panel and the icons: it is
            // inside the window the panel paints, and an item on the cursor
            // passes over it.
            if let (Some(preview), Some((vp, rect))) = (&self.preview, self.gui.preview_state) {
                clear_depth_rect(gpu, cb, rect);
                preview.draw_solid(gpu, cb, vp, extent);
            }
            if let Some(items) = &self.gui_items {
                items.draw(gpu, cb, extent);
                self.in_gamma_space(gpu, cb, extent, || items.draw_glint(gpu, cb, extent));
            }
            pass.draw_front(gpu, cb, extent);
            pass.draw_bars(gpu, cb, extent);
            // The tooltip box goes over everything else the screen draws. Its
            // text follows in the text pass below, which is drawn last of all.
            pass.draw_tooltip(gpu, cb, extent);
        }
        // The screen framework's chrome (M82), over the HUD and the inventory
        // and under the text pass — vanilla's own order is
        // `extractBackground` then the widgets then the deferred text, and the
        // text pass below is drawn last of all for every caller.
        if let Some(pass) = self.screen.as_mut() {
            pass.set_state(extent, &self.gui.screen_draw);
            pass.draw(gpu, cb, extent);
        }
        if let Some(text) = self.text.as_mut() {
            if !self.gui.text_lines.is_empty() {
                let lines: Vec<TextLine> = self
                    .gui
                    .text_lines
                    .iter()
                    .map(|l| TextLine {
                        x: l.x,
                        y: l.y,
                        px: l.px,
                        color_linear: l.color_linear,
                        alpha: l.alpha,
                        shadow: l.shadow,
                        style: l.style,
                        text: &l.text,
                    })
                    .collect();
                text.draw(gpu, cb, extent, &lines);
            }
        }
        // Velvet text last, and in gamma space. `take` because
        // `in_gamma_space` borrows `&self` while the pass needs `&mut` — the
        // same dance `self.text.take()` does at teardown.
        if !self.gui.velvet_runs.is_empty() {
            if let Some(mut vt) = self.velvet_text.take() {
                let runs: Vec<crate::velvet_text::Run> = self
                    .gui
                    .velvet_runs
                    .iter()
                    .map(|r| crate::velvet_text::Run {
                        glyphs: &r.glyphs,
                        color: r.color,
                        alpha: r.alpha,
                    })
                    .collect();
                self.in_gamma_space(gpu, cb, extent, || vt.draw(gpu, cb, extent, &runs));
                self.velvet_text = Some(vt);
            }
        }
    }

    /// The block-selection wireframe: the 12 edges of the targeted block,
    /// slightly inflated to avoid z-fighting, depth-tested against terrain.
    /// Replace this frame's leash ribbons. `verts` is a TRIANGLE_LIST already
    /// folded (position + linear colour) by `leash::build_ribbon`, concatenated
    /// across every visible leash. Truncated to the vertex budget.
    pub fn set_leash(&mut self, verts: &[crate::leash::LeashVertex]) {
        self.leash_verts.clear();
        // Whole triangles only, so a truncated ribbon never draws a torn one.
        let n = verts.len().min(LEASH_MAX_VERTS) / 3 * 3;
        if n < verts.len() && !self.leash_warned {
            self.leash_warned = true;
            log::warn!(
                "world: {} leash vertices exceed the {LEASH_MAX_VERTS}-vertex budget — truncating",
                verts.len()
            );
        }
        self.leash_verts.extend_from_slice(&verts[..n]);
    }

    /// This frame's leash-ribbon vertex count — the render-check's r60
    /// observable (a leash gathered by `collect_leashes` reached the pass).
    pub fn leash_vert_count(&self) -> usize {
        self.leash_verts.len()
    }

    /// Draw this frame's leash ribbons, if any. Depth-tested against the
    /// terrain and entities (GREATER, reversed-Z, no write) so a rope is
    /// occluded by blocks and closer mobs but composites over the sky.
    fn draw_leash(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let count = self.leash_verts.len() as u32;
        if count < 3 {
            return;
        }
        let stride = std::mem::size_of::<crate::leash::LeashVertex>();
        let bytes = self
            .leash_ring
            .write_fixed(bytemuck::cast_slice(&self.leash_verts), stride * 3);
        let count = (bytes / stride) as u32;
        let Some(vbuf) = self.leash_ring.bind() else {
            return;
        };
        let push = LeashPush { view_proj };
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.leash_pipeline);
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            device.cmd_push_constants(
                cb,
                self.leash_layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                std::slice::from_raw_parts(
                    (&push as *const LeashPush) as *const u8,
                    std::mem::size_of::<LeashPush>(),
                ),
            );
            device.cmd_bind_vertex_buffers(cb, 0, &[vbuf], &[0]);
            device.cmd_draw(cb, count, 1, 0, 0);
        }
    }

    fn draw_selection(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let Some(b) = self.scene.selection else {
            return;
        };
        let (x0, y0, z0) = (
            b[0] as f32 - 0.002,
            b[1] as f32 - 0.002,
            b[2] as f32 - 0.002,
        );
        let (x1, y1, z1) = (
            b[0] as f32 + 1.002,
            b[1] as f32 + 1.002,
            b[2] as f32 + 1.002,
        );
        // 8 corners → 12 edges → 24 line vertices.
        let c = [
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y0, z1],
            [x0, y0, z1], // bottom
            [x0, y1, z0],
            [x1, y1, z0],
            [x1, y1, z1],
            [x0, y1, z1], // top
        ];
        let edges = [
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 0), // bottom loop
            (4, 5),
            (5, 6),
            (6, 7),
            (7, 4), // top loop
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7), // verticals
        ];
        let mut verts = [[0f32; 3]; 24];
        for (i, &(a, bb)) in edges.iter().enumerate() {
            verts[i * 2] = c[a];
            verts[i * 2 + 1] = c[bb];
        }

        self.select_ring
            .write_fixed(bytemuck::cast_slice(&verts), std::mem::size_of_val(&verts));
        let Some(vbuf) = self.select_ring.bind() else {
            return;
        };
        let push = LinePush {
            view_proj,
            color: [0.0, 0.0, 0.0, 0.7], // black outline, like vanilla
        };
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.select_pipeline);
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            device.cmd_push_constants(
                cb,
                self.select_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                std::slice::from_raw_parts(
                    (&push as *const LinePush) as *const u8,
                    std::mem::size_of::<LinePush>(),
                ),
            );
            device.cmd_bind_vertex_buffers(cb, 0, &[vbuf], &[0]);
            device.cmd_draw(cb, 24, 1, 0, 0);
        }
    }

    /// Water: per-column direct draws, CPU frustum-culled (mirroring
    /// cull.comp's positive-vertex test) and sorted far→near — the plan's
    /// "per-section back-to-front CPU sort; intra-section artifacts
    /// accepted v1". Column counts with water are small; the indirect path
    /// stays opaque-only.
    fn draw_translucent(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let planes = frustum_planes(&view_proj);
        let visible = |mn: &[f32; 4], mx: &[f32; 4]| -> bool {
            planes.iter().all(|p| {
                let pv = [
                    if p[0] >= 0.0 { mx[0] } else { mn[0] },
                    if p[1] >= 0.0 { mx[1] } else { mn[1] },
                    if p[2] >= 0.0 { mx[2] } else { mn[2] },
                ];
                p[0] * pv[0] + p[1] * pv[1] + p[2] * pv[2] + p[3] >= 0.0
            })
        };
        let eye = self.scene.camera_eye;
        let mut cols: Vec<(f32, u32, u32, i32)> = self
            .arena
            .columns
            .values()
            .filter(|s| s.tidx_len > 0 && visible(&s.meta.aabb_min, &s.meta.aabb_max))
            .map(|s| {
                let cx = (s.meta.aabb_min[0] + s.meta.aabb_max[0]) * 0.5 - eye[0];
                let cy = (s.meta.aabb_min[1] + s.meta.aabb_max[1]) * 0.5 - eye[1];
                let cz = (s.meta.aabb_min[2] + s.meta.aabb_max[2]) * 0.5 - eye[2];
                (
                    cx * cx + cy * cy + cz * cz,
                    s.tidx_len as u32,
                    s.tidx_off as u32,
                    s.tvtx_off as i32,
                )
            })
            .collect();
        if cols.is_empty() {
            return;
        }
        cols.sort_by(|a, b| b.0.total_cmp(&a.0));

        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.water_pipeline);
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.graphics_layout,
                0,
                &[self.tex_sets[self.lm.cursor]],
                &[],
            );
            self.push_world(device, cb, view_proj);
            device.cmd_bind_vertex_buffers(cb, 0, &[self.arena.vbuf], &[0]);
            device.cmd_bind_index_buffer(cb, self.arena.ibuf, 0, vk::IndexType::UINT32);
            for (_, count, first, voff) in cols {
                device.cmd_draw_indexed(cb, count, 1, first, voff, 0);
            }
        }
    }

    fn draw_terrain(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.graphics_layout,
                0,
                &[self.tex_sets[self.lm.cursor]],
                &[],
            );
            self.push_world(device, cb, view_proj);
            device.cmd_bind_vertex_buffers(cb, 0, &[self.arena.vbuf], &[0]);
            device.cmd_bind_index_buffer(cb, self.arena.ibuf, 0, vk::IndexType::UINT32);
            device.cmd_draw_indexed_indirect_count(
                cb,
                self.culler.indirect_bufs[self.culler.cursor],
                0,
                self.culler.count_bufs[self.culler.cursor],
                0,
                self.culler.column_count,
                std::mem::size_of::<vk::DrawIndexedIndirectCommand>() as u32,
            );
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        if let Some(mut pass) = self.celestial.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.end_sky.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.clouds.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.border.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.hand.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.preview.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.locator.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.container.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.screen.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.gui_items.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.particles.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.crumbling.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.weather.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.end_portal.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.entities.take() {
            pass.destroy(gpu);
        }
        if let Some(mut hud) = self.hud.take() {
            hud.destroy(gpu);
        }
        if let Some(mut vt) = self.velvet_text.take() {
            vt.destroy(gpu);
        }
        if let Some(mut text) = self.text.take() {
            text.destroy(gpu);
        }
        self.select_ring.destroy(gpu);
        self.leash_ring.destroy(gpu);
        unsafe {
            let device = &gpu.device;
            for s in &mut self.arena.upload_slots {
                device.destroy_fence(s.fence, None);
                device.destroy_buffer(s.staging, None);
            }
            device.destroy_command_pool(self.arena.upload_pool, None);
            self.culler.destroy_objects(device);
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline(self.water_pipeline, None);
            self.sky_gradient.destroy(device);
            device.destroy_pipeline(self.select_pipeline, None);
            device.destroy_pipeline_layout(self.select_layout, None);
            device.destroy_pipeline(self.leash_pipeline, None);
            device.destroy_pipeline_layout(self.leash_layout, None);
            for b in self.lm.bufs {
                device.destroy_buffer(b, None);
            }
            device.destroy_pipeline_layout(self.graphics_layout, None);
            device.destroy_descriptor_pool(self.tex_pool, None);
            device.destroy_descriptor_set_layout(self.tex_set_layout, None);
            self.textures.destroy_objects(device);
            for b in [self.arena.vbuf, self.arena.ibuf, self.culler.readback] {
                device.destroy_buffer(b, None);
            }
        }
        let mut slot_allocs: Vec<Option<Allocation>> = self
            .arena
            .upload_slots
            .iter_mut()
            .map(|s| s.staging_alloc.take())
            .collect();
        for a in self.lm.allocs.iter_mut() {
            slot_allocs.push(a.take());
        }
        for a in [
            self.textures.image_alloc.take(),
            self.arena.vbuf_alloc.take(),
            self.arena.ibuf_alloc.take(),
            self.culler.readback_alloc.take(),
        ]
        .into_iter()
        .chain(slot_allocs)
        .chain(self.culler.meta_allocs.iter_mut().map(|a| a.take()))
        .chain(self.culler.indirect_allocs.iter_mut().map(|a| a.take()))
        .chain(self.culler.count_allocs.iter_mut().map(|a| a.take()))
        .flatten()
        {
            let _ = gpu.allocator.free(a);
        }
    }
}

// -- helpers -----------------------------------------------------------------

fn create_device_buffer(
    gpu: &mut Gpu,
    size: u64,
    usage: vk::BufferUsageFlags,
    name: &str,
) -> Result<(vk::Buffer, Allocation), String> {
    unsafe {
        let buffer = gpu
            .device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("{name}: {e}"))?;
        let req = gpu.device.get_buffer_memory_requirements(buffer);
        let alloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name,
                requirements: req,
                location: MemoryLocation::GpuOnly,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("{name} alloc: {e}"))?;
        gpu.device
            .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
            .map_err(|e| format!("{name} bind: {e}"))?;
        Ok((buffer, alloc))
    }
}

fn create_host_buffer(
    gpu: &mut Gpu,
    size: u64,
    usage: vk::BufferUsageFlags,
    name: &str,
) -> Result<(vk::Buffer, Allocation), String> {
    unsafe {
        let buffer = gpu
            .device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("{name}: {e}"))?;
        let req = gpu.device.get_buffer_memory_requirements(buffer);
        let alloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name,
                requirements: req,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("{name} alloc: {e}"))?;
        gpu.device
            .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
            .map_err(|e| format!("{name} bind: {e}"))?;
        Ok((buffer, alloc))
    }
}

/// Line pipeline for the selection outline: LINE_LIST, depth-tested
/// (reversed-Z GREATER) but no depth write, alpha-blended.
fn build_leash_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/leash.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/leash.frag.spv")),
        )?;
        let entry = c"main";
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert)
                .name(entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag)
                .name(entry),
        ];
        // pos vec3 @0, color vec3 @12: one `LeashVertex`.
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<crate::leash::LeashVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(12),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        // Cull NONE: the strip's winding alternates, and the ribbon is meant to
        // be seen from both sides.
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::GREATER); // reversed-Z
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B,
            )];
        let blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [color_format];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&color_formats)
            .depth_attachment_format(DEPTH_FORMAT);
        let ci = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), &[ci], None)
            .map_err(|(_, e)| format!("leash pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

fn build_line_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/line.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/line.frag.spv")),
        )?;
        let entry = c"main";
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert)
                .name(entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag)
                .name(entry),
        ];
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<[f32; 3]>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [vk::VertexInputAttributeDescription::default()
            .location(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(0)];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::LINE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::GREATER); // reversed-Z
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B,
            )];
        let blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [color_format];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&color_formats)
            .depth_attachment_format(DEPTH_FORMAT);
        let ci = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&ci), None)
            .map_err(|(_, e)| format!("line pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

fn build_graphics_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
    translucent: bool,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/world.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            if translucent {
                include_bytes!(concat!(env!("OUT_DIR"), "/water.frag.spv")).as_slice()
            } else {
                include_bytes!(concat!(env!("OUT_DIR"), "/world.frag.spv")).as_slice()
            },
        )?;
        let entry = c"main";
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert)
                .name(entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag)
                .name(entry),
        ];
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(VERTEX_STRIDE as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(12),
            vk::VertexInputAttributeDescription::default()
                .location(2)
                .format(vk::Format::R32_UINT)
                .offset(20),
            // The vertex color, read as vanilla's `Color` attribute is:
            // UNORM bytes.
            vk::VertexInputAttributeDescription::default()
                .location(3)
                .format(vk::Format::R8G8B8A8_UNORM)
                .offset(24),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(!translucent)
            .depth_compare_op(vk::CompareOp::GREATER); // reversed-Z
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(translucent)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B,
            )];
        let blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [color_format];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&color_formats)
            .depth_attachment_format(DEPTH_FORMAT);
        let ci = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&ci), None)
            .map_err(|(_, e)| format!("world pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

// -- texture upload (unchanged from M2/M4) -----------------------------------

// -- depth target (unchanged) ------------------------------------------------

pub struct DepthTarget {
    pub image: vk::Image,
    alloc: Option<Allocation>,
    pub view: vk::ImageView,
    pub extent: vk::Extent2D,
}

impl DepthTarget {
    pub fn new(gpu: &mut Gpu, extent: vk::Extent2D) -> Result<Self, String> {
        unsafe {
            let device = gpu.device.clone();
            let image = device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(DEPTH_FORMAT)
                        .extent(vk::Extent3D {
                            width: extent.width.max(1),
                            height: extent.height.max(1),
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .map_err(|e| format!("depth image: {e}"))?;
            let req = device.get_image_memory_requirements(image);
            let alloc = gpu
                .allocator
                .allocate(&AllocationCreateDesc {
                    name: "depth-target",
                    requirements: req,
                    location: MemoryLocation::GpuOnly,
                    linear: false,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .map_err(|e| format!("depth alloc: {e}"))?;
            device
                .bind_image_memory(image, alloc.memory(), alloc.offset())
                .map_err(|e| format!("depth bind: {e}"))?;
            let view = device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(DEPTH_FORMAT)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::DEPTH)
                                .base_mip_level(0)
                                .level_count(1)
                                .base_array_layer(0)
                                .layer_count(1),
                        ),
                    None,
                )
                .map_err(|e| format!("depth view: {e}"))?;
            Ok(Self {
                image,
                alloc: Some(alloc),
                view,
                extent,
            })
        }
    }

    pub fn barrier_for_use(&self, gpu: &Gpu, cb: vk::CommandBuffer) {
        unsafe {
            // One depth image serves every frame in flight, so the previous
            // frame's depth writes must finish before this frame's clear
            // (a WAW the old TOP_OF_PIPE/no-access source left unordered).
            let barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(
                    vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                )
                .src_access_mask(vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE)
                .dst_stage_mask(
                    vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                )
                .dst_access_mask(
                    vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                        | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
                )
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(self.image)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::DEPTH)
                        .base_mip_level(0)
                        .level_count(1)
                        .base_array_layer(0)
                        .layer_count(1),
                );
            gpu.device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default()
                    .image_memory_barriers(std::slice::from_ref(&barrier)),
            );
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        unsafe {
            gpu.device.destroy_image_view(self.view, None);
            gpu.device.destroy_image(self.image, None);
        }
        if let Some(a) = self.alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

// -- frustum -----------------------------------------------------------------

/// Infinite reversed-Z perspective (RH, Y-up). Maps z=-near→1, z=-∞→0; pair
/// with a 0.0 depth clear + `GREATER`. Column-major.
pub fn perspective_reverse_z(fov_y_rad: f32, aspect: f32, near: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fov_y_rad * 0.5).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, 0.0, -1.0],
        [0.0, 0.0, near, 0.0],
    ]
}

/// Clear the depth attachment over one rectangle, mid-render-pass.
///
/// The preview shares the frame's depth buffer with the world, so without this
/// the model would be depth-tested against whatever terrain happens to be
/// behind the panel and would appear cut in half by a hillside. Reversed-Z, so
/// the cleared value is **0.0** — the far plane — which is what the world pass
/// clears to as well.
fn clear_depth_rect(gpu: &Gpu, cb: vk::CommandBuffer, rect: vk::Rect2D) {
    let attachment = vk::ClearAttachment::default()
        .aspect_mask(vk::ImageAspectFlags::DEPTH)
        .clear_value(vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue {
                depth: 0.0,
                stencil: 0,
            },
        });
    let area = vk::ClearRect::default()
        .rect(rect)
        .base_array_layer(0)
        .layer_count(1);
    unsafe { gpu.device.cmd_clear_attachments(cb, &[attachment], &[area]) };
}
