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

use crate::entities::EntityPass;
use crate::hud::HudPass;
use crate::text::TextPass;
use crate::Gpu;

mod arena;
mod cull;
mod entity_passes;
mod gamma;
mod gui_passes;
mod lightmap;
mod scene_passes;
mod sky;
mod textures;

use arena::Arena;
pub use arena::{MAX_COLUMNS, MAX_INDICES, MAX_VERTS, VERTEX_STRIDE};
use cull::{frustum_planes, Culler};
pub use entity_passes::PlayerTexture;
pub use gamma::{unorm_of, GammaScope};
use gui_passes::GuiFrame;
pub use gui_passes::OwnedTextLine;
use lightmap::{LightmapRing, WorldLightmapExtra, LIGHTMAP_UBO_RING};
pub use lightmap::{WorldLightmapState, BLOCK_LIGHT_FACTOR};
use sky::{parse_fog_env, SkyGradient};
pub use sky::{SkyMode, SKY_HORIZON};
pub(crate) use textures::upload_texture_array;
pub use textures::LayerAnimation;
use textures::TextureArray;

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
        self.draw_scene_passes(gpu, cb, view_proj, extent);
        self.draw_gui_passes(gpu, cb, extent);
    }

    /// The world half of [`Self::draw`]: opaque terrain, the crack overlay,
    /// leashes and the selection outline, solid entities and their glint, end
    /// portals, clouds, translucent water, particles, weather, the world border
    /// and nametags — in blend-correct order.
    fn draw_scene_passes(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
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
