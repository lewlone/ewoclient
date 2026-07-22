//! Entity pass — textured mob models + capsule fallback + floating
//! nametags.
//!
//! Mob geometry comes from [`crate::mobs`] — a faithful port of vanilla's
//! `ModelPart.Cube` (see that module for the coordinate contract). This
//! module owns everything GPU-side: the shared atlas (font + all mob skins,
//! shelf-packed 64×64 slots), per-frame animation (vanilla `setupAnim`
//! angles about each part's pivot), the vanilla entity transform
//! (`rotY(180−yaw) · scale(−1,−1,1) · translate(0,−1.501,0)`), and the
//! draw pipelines.
//!
//! Deliberately simple: geometry is a **CPU-built world-space triangle
//! soup** rebuilt every frame — model quads, capsule shells (~500 verts
//! each), and camera-billboarded glyph quads. Entity counts are dozens, not
//! thousands, so building on the CPU costs microseconds and avoids
//! instancing + billboard shaders entirely; revisit if entity counts ever
//! grow 100×.
//!
//! One vertex format (pos, uv, rgba) through one shader family: glyphs
//! sample the font atlas, solid geometry (capsules, tag backgrounds)
//! samples the atlas's patched opaque-white texel. Two pipelines split
//! depth behavior: solid writes depth (GREATER — reversed-Z), text blends
//! with depth-write off. Both mask alpha writes (render discipline #2) and
//! take pre-linearized vertex colors (discipline #1).
//!
//! Buffers are a 2-slot ring flipped on each `set_draws`: with the frame
//! driver fence-pacing at most 2 frames in flight, the slot being rewritten
//! retired two submissions ago.
//!
//! Verification knob: `REWO_MOB_DEBUG_TEX=1` replaces every mob texture
//! with facelabel colors (each box-UV face rect painted its
//! [`mobs::Facing::debug_color`]) — `rewo mobshot --check` renders each mob
//! and asserts the rendered dominant color matches the geometric
//! prediction, proving texture-face correspondence end-to-end.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::mobs::{self, Facing};
use crate::world::DEPTH_FORMAT;
use crate::Gpu;

pub use crate::mobs::EntityModelKind;

const VERTEX_STRIDE: u64 = 36; // 3 pos + 2 uv + 4 rgba f32s
/// ~500 capsules' worth (a flat-world slime herd alone reaches 129
/// entities ≈ 65k verts). 9.4 MB × 2 ring slots — cheap; the CPU soup
/// build is the real ceiling long before this is.
const MAX_VERTS: usize = 262_144;
const RING: usize = 2;
/// Capsule tessellation: segments around Y × profile bands.
const SEGMENTS: usize = 12;
/// Nametag world scale per font pixel at cell=8 (vanilla's 0.025).
const TAG_PX: f32 = 0.025;
/// Tag anchor height above the entity's head.
const TAG_LIFT: f32 = 0.4;

/// Borrowed view of `rewo_data::assets::BakedFont` — keeps this crate free
/// of a rewo-data dependency (same pattern as the texture-layer slices).
pub struct FontData<'a> {
    pub atlas: &'a [u8],
    pub size: u32,
    pub cell: u32,
    pub advance: &'a [u8; 256],
    pub white_texel: (u32, u32),
}

/// One entity to draw this frame — position already frame-interpolated.
pub struct EntityDraw<'a> {
    /// Feet-center world position.
    pub pos: [f32; 3],
    pub width: f32,
    pub height: f32,
    /// Linear-space base color (capsules; the player model is textured).
    pub color: [f32; 3],
    /// Nametag text (players); `None` draws no tag.
    pub name: Option<&'a str>,
    /// Which model to draw (falls back to the capsule when the model's
    /// texture wasn't baked).
    pub kind: EntityModelKind,
    /// Body yaw (degrees, MC convention) — rotates the whole model.
    pub yaw: f32,
    /// Head yaw (degrees, absolute). The head part rotates about its own
    /// pivot by the net `head_yaw − yaw` (vanilla `netHeadYaw`); equal to
    /// `yaw` leaves the head aligned with the body.
    pub head_yaw: f32,
    /// Look pitch (degrees) — tilts the head about the neck.
    pub pitch: f32,
    /// Walk-cycle phase + amplitude (vanilla limbSwing / limbSwingAmount)
    /// — arms and legs swing about their pivots. 0 amount = neutral pose.
    pub limb_swing: f32,
    pub limb_amount: f32,
    /// Active pose/state gesture + seconds since it started (drives the
    /// one-shot keyframe rigs — warden roar, sniffer dig, …).
    pub gesture: Option<(mobs::Gesture, f32)>,
    /// Armadillo shell swap (vanilla `isHidingInShell`).
    pub shell: bool,
    /// Player skin: a normalized UV offset that relocates the (default-Steve)
    /// player-model quads onto this player's uploaded skin slot. `None` →
    /// the default skin. Ignored for non-player models.
    pub skin_uv: Option<[f32; 2]>,
    /// Uniform model-scale multiplier on top of the baked scale — vanilla's
    /// per-entity render scale (slime/magma-cube `size`). 1.0 = as baked.
    pub scale_mul: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 3],
    uv: [f32; 2],
    color: [f32; 4],
}

/// Combined entity atlas: the font occupies (0,0)..(128,128); mob textures
/// (16² tadpole up to 192² sniffer-class skins) shelf-pack around it. One
/// texture, one pipeline family.
const ATLAS_W: u32 = 1024;
const ATLAS_H: u32 = 1024;

/// Dynamic player-skin pool: 32 slots of 64×64 in the atlas's bottom two
/// rows (y 896..1024), filled at runtime as players' skins arrive. The mob
/// packer is capped above it so it never collides.
const SKIN_SLOT: u32 = 64;
const SKIN_POOL_COLS: u32 = ATLAS_W / SKIN_SLOT; // 16
const SKIN_POOL_ROWS: u32 = 2;
const SKIN_SLOTS: u32 = SKIN_POOL_COLS * SKIN_POOL_ROWS; // 32
const SKIN_POOL_Y: u32 = ATLAS_H - SKIN_POOL_ROWS * SKIN_SLOT; // 896

/// Atlas origin of dynamic skin slot `i` (0..SKIN_SLOTS).
fn skin_slot_origin(i: u32) -> (u32, u32) {
    (
        (i % SKIN_POOL_COLS) * SKIN_SLOT,
        SKIN_POOL_Y + (i / SKIN_POOL_COLS) * SKIN_SLOT,
    )
}

/// Shelf-pack `sizes` into the atlas, avoiding the font block. Deterministic:
/// callers pass entries sorted however they like; shelves grow downward,
/// each shelf as tall as its tallest member. Returns per-entry origins
/// (`None` if the atlas overflowed — practically unreachable at 1024²).
fn pack_shelves(sizes: &[(u32, u32)]) -> Vec<Option<(u32, u32)>> {
    let (mut x, mut y, mut shelf_h) = (128u32, 0u32, 0u32);
    let mut out = Vec::with_capacity(sizes.len());
    for &(w, h) in sizes {
        if w > ATLAS_W || h > ATLAS_H {
            out.push(None);
            continue;
        }
        loop {
            // Inside the font's rows, usable x starts at 128.
            let x_min = if y < 128 { 128 } else { 0 };
            if x < x_min {
                x = x_min;
            }
            if x + w <= ATLAS_W && y + h <= SKIN_POOL_Y {
                break;
            }
            if x + w > ATLAS_W {
                // New shelf.
                y += shelf_h.max(1);
                x = 0;
                shelf_h = 0;
                continue;
            }
            // Out of vertical space (the skin pool caps the packer).
            break;
        }
        if x + w <= ATLAS_W && y + h <= SKIN_POOL_Y {
            out.push(Some((x, y)));
            x += w;
            shelf_h = shelf_h.max(h);
        } else {
            out.push(None);
        }
    }
    out
}

/// One baked mob texture, keyed by the registry names in
/// [`mobs::MobDef::textures`] (e.g. "cow", "sheep_wool").
pub struct MobTexEntry<'a> {
    pub key: &'static str,
    pub w: u32,
    pub h: u32,
    pub rgba: &'a [u8],
}

/// Borrowed view of the baked mob-texture table for `EntityPass::new`. A
/// missing entry degrades that mob to the capsule fallback.
#[derive(Default)]
pub struct MobTextures<'a> {
    pub entries: Vec<MobTexEntry<'a>>,
}

pub struct EntityPass {
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    solid_pipeline: vk::Pipeline,
    text_pipeline: vk::Pipeline,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    image: vk::Image,
    image_alloc: Option<Allocation>,
    view: vk::ImageView,
    bufs: [vk::Buffer; RING],
    allocs: [Option<Allocation>; RING],
    cursor: usize,
    solid_verts: u32,
    text_verts: u32,
    /// Unit capsule shell: (position [0..1 y, ±0.5 xz], normal).
    capsule: Vec<([f32; 3], [f32; 3])>,
    /// Built mob models, indexed by `EntityModelKind::index()`. `None` =
    /// texture missing (or the capsule kind) → capsule fallback.
    models: Vec<Option<MobModel>>,
    /// Facelabel mode: kinds excluded from the color check (see
    /// `debug_ambiguous_kinds`). Empty outside debug-texture mode.
    debug_ambiguous: Vec<EntityModelKind>,
    // Font metrics (identity values when no font was provided).
    cell: u32,
    advance: [u8; 256],
    white_uv: [f32; 2],
    has_font: bool,
    /// Atlas origin of the default "player" skin slot — the reference the
    /// per-player skin UV offset is measured from.
    player_origin: (u32, u32),
    /// Next free dynamic skin slot (wraps at `SKIN_SLOTS`; a small network
    /// never fills 32, and wrap-around just recycles the oldest slot).
    skin_next: u32,
}

/// One mob model ready to draw: quads in part-local model px with
/// atlas-normalized UVs, the animated parts, and the px→block scale.
pub struct MobModel {
    quads: Vec<GpuQuad>,
    parts: Vec<mobs::Part>,
    keyframes: Vec<mobs::KfAnim>,
    /// Model px → world blocks (vanilla 1/16 × the mob's render scale).
    scale: f32,
}

struct GpuQuad {
    pos: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
    shade: f32,
    part: u16,
    /// Vanilla face label + static-folded model-space normal — kept for the
    /// mobshot facelabel verification (`neutral_quads`).
    facing: Facing,
    normal: [f32; 3],
}

impl EntityPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        font: Option<FontData<'_>>,
        tex: MobTextures<'_>,
    ) -> Result<Self, String> {
        let device = gpu.device.clone();

        // ---- combined atlas: font at (0,0), mob skins in 64×64 slots ----
        let mut atlas = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
        let (cell, advance, white_texel, has_font) = match &font {
            Some(f) => {
                let side = f.size.min(128) as usize;
                for row in 0..side {
                    let src = row * f.size as usize * 4;
                    let dst = row * ATLAS_W as usize * 4;
                    atlas[dst..dst + side * 4].copy_from_slice(&f.atlas[src..src + side * 4]);
                }
                (f.cell, *f.advance, f.white_texel, true)
            }
            None => (8, [4u8; 256], (0, 16), false),
        };
        // The white texel lives in the (blank) space glyph cell — patch it
        // whether or not a font was blitted (the cell is zeroed either way).
        let wi = ((white_texel.1 * ATLAS_W + white_texel.0) * 4) as usize;
        atlas[wi..wi + 4].copy_from_slice(&[255, 255, 255, 255]);

        // Shelf-pack every provided mob texture (tallest first for tight
        // shelves; the key map keeps lookups order-independent).
        let mut order: Vec<usize> = (0..tex.entries.len()).collect();
        order.sort_by_key(|&i| {
            let e = &tex.entries[i];
            (std::cmp::Reverse(e.h), std::cmp::Reverse(e.w), e.key)
        });
        let sizes: Vec<(u32, u32)> = order.iter().map(|&i| (tex.entries[i].w, tex.entries[i].h)).collect();
        let origins = pack_shelves(&sizes);
        let mut slots: std::collections::HashMap<&'static str, (u32, u32, u32, u32)> =
            std::collections::HashMap::new();
        for (slot, &i) in order.iter().enumerate() {
            let e = &tex.entries[i];
            match origins[slot] {
                Some((x, y)) if blit_tex(&mut atlas, Some(e.rgba), x, y, e.w, e.h) => {
                    slots.insert(e.key, (x, y, e.w, e.h));
                }
                _ => log::warn!("entities: mob texture {} ({}×{}) didn't pack", e.key, e.w, e.h),
            }
        }

        // Facelabel verification mode: replace every mob texture with per-
        // face solid colors so a render proves texture-face correspondence.
        let debug_tex = std::env::var("REWO_MOB_DEBUG_TEX").map_or(false, |v| v == "1");

        // Build each registry mob whose textures are all present.
        let mut models: Vec<Option<MobModel>> = (0..EntityModelKind::COUNT).map(|_| None).collect();
        // Facelabel mode: textures where different face labels paint the
        // same texel (vanilla reuses some regions across faces — the breeze
        // wind funnel's concentric shells) can't be verified by color; the
        // checker skips the kinds that sample them.
        let mut painted: std::collections::HashMap<(u32, u32), [u8; 3]> =
            std::collections::HashMap::new();
        let mut ambiguous_tex: std::collections::HashSet<&'static str> =
            std::collections::HashSet::new();
        let mut debug_ambiguous: Vec<EntityModelKind> = Vec::new();
        for def in mobs::MOBS {
            let origins: Option<Vec<(u32, u32, u32, u32)>> =
                def.textures.iter().map(|k| slots.get(k).copied()).collect();
            let Some(origins) = origins else { continue };
            let m = (def.build)();
            if debug_tex {
                for q in &m.quads {
                    if !paint_debug_rect(&mut atlas, origins[q.tex], q, &mut painted) {
                        if ambiguous_tex.insert(def.textures[q.tex]) {
                            log::info!(
                                "mob debug-tex: {} ({:?} {:?} quad) repaints a texel with a new label",
                                def.textures[q.tex],
                                def.kind,
                                q.facing
                            );
                        }
                    }
                }
            }
            let quads = m
                .quads
                .iter()
                .map(|q| {
                    let (ox, oy, tw, th) = origins[q.tex];
                    // Clamp into the texture rect — a couple of vanilla fin
                    // rects stray outside their texture (clamp-sampler
                    // behavior); unclamped they'd bleed into atlas neighbors.
                    let uv = q.uv.map(|[u, v]| {
                        [
                            (ox as f32 + u.clamp(0.0, tw as f32)) / ATLAS_W as f32,
                            (oy as f32 + v.clamp(0.0, th as f32)) / ATLAS_H as f32,
                        ]
                    });
                    GpuQuad {
                        pos: q.pos,
                        uv,
                        shade: q.shade,
                        part: q.part as u16,
                        facing: q.facing,
                        normal: q.normal,
                    }
                })
                .collect();
            models[def.kind.index()] = Some(MobModel {
                quads,
                parts: m.parts,
                keyframes: m.keyframes,
                scale: m.scale / 16.0,
            });
        }
        if debug_tex {
            for def in mobs::MOBS {
                if def.textures.iter().any(|k| ambiguous_tex.contains(k)) {
                    debug_ambiguous.push(def.kind);
                }
            }
        }
        let (image, image_alloc, view) = create_texture(gpu, &atlas, ATLAS_W, ATLAS_H)?;
        let white_uv = [
            (white_texel.0 as f32 + 0.5) / ATLAS_W as f32,
            (white_texel.1 as f32 + 0.5) / ATLAS_H as f32,
        ];

        unsafe {
            let sampler = device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                    None,
                )
                .map_err(|e| format!("entity sampler: {e}"))?;

            let bindings = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let set_layout = device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| format!("entity set layout: {e}"))?;
            let pool_sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)];
            let pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .map_err(|e| format!("entity pool: {e}"))?;
            let set_layouts = [set_layout];
            let set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("entity set: {e}"))?[0];
            let image_info = [vk::DescriptorImageInfo::default()
                .sampler(sampler)
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&image_info)],
                &[],
            );

            let pc = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(64)];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&pc),
                    None,
                )
                .map_err(|e| format!("entity layout: {e}"))?;
            let solid_pipeline = build_pipeline(&device, layout, color_format, true)?;
            let text_pipeline = build_pipeline(&device, layout, color_format, false)?;

            let mut bufs = [vk::Buffer::null(); RING];
            let mut allocs: [Option<Allocation>; RING] = [None, None];
            for i in 0..RING {
                let buffer = device
                    .create_buffer(
                        &vk::BufferCreateInfo::default()
                            .size(MAX_VERTS as u64 * VERTEX_STRIDE)
                            .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
                            .sharing_mode(vk::SharingMode::EXCLUSIVE),
                        None,
                    )
                    .map_err(|e| format!("entity vbuf: {e}"))?;
                let req = device.get_buffer_memory_requirements(buffer);
                let alloc = gpu
                    .allocator
                    .allocate(&AllocationCreateDesc {
                        name: "entity-verts",
                        requirements: req,
                        location: MemoryLocation::CpuToGpu,
                        linear: true,
                        allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                    })
                    .map_err(|e| format!("entity vbuf alloc: {e}"))?;
                device
                    .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
                    .map_err(|e| format!("entity vbuf bind: {e}"))?;
                bufs[i] = buffer;
                allocs[i] = Some(alloc);
            }

            Ok(Self {
                layout,
                set_layout,
                solid_pipeline,
                text_pipeline,
                pool,
                set,
                sampler,
                image,
                image_alloc: Some(image_alloc),
                view,
                bufs,
                allocs,
                cursor: 0,
                solid_verts: 0,
                text_verts: 0,
                capsule: unit_capsule(),
                models,
                debug_ambiguous,
                cell,
                advance,
                white_uv,
                has_font,
                player_origin: slots.get("player").map(|&(x, y, _, _)| (x, y)).unwrap_or((0, 0)),
                skin_next: 0,
            })
        }
    }

    /// Reserve the next dynamic skin slot, upload a 64×64 RGBA skin into it,
    /// and return the normalized UV offset relocating the default player
    /// quads onto it (feed to `EntityDraw::skin_uv`). `rgba` must be
    /// `64*64*4` bytes. Stalls on `wait_idle` — skins arrive rarely (once
    /// per player at join), so the one-off is cheaper than tracking
    /// per-frame fences against the shared atlas.
    pub fn upload_skin(&mut self, gpu: &mut Gpu, rgba: &[u8]) -> Result<[f32; 2], String> {
        let slot = self.skin_next % SKIN_SLOTS;
        self.skin_next += 1;
        let (sx, sy) = skin_slot_origin(slot);
        upload_region(gpu, self.image, rgba, sx, sy, SKIN_SLOT, SKIN_SLOT)?;
        let (px, py) = self.player_origin;
        Ok([
            (sx as f32 - px as f32) / ATLAS_W as f32,
            (sy as f32 - py as f32) / ATLAS_H as f32,
        ])
    }

    /// Facelabel mode only: kinds whose textures paint conflicting face
    /// labels onto shared texels (vanilla region reuse) — the color check
    /// cannot apply to them.
    pub fn debug_ambiguous_kinds(&self) -> &[EntityModelKind] {
        &self.debug_ambiguous
    }

    /// Kinds with a built model (all textures were present).
    pub fn available_kinds(&self) -> Vec<EntityModelKind> {
        EntityModelKind::ALL
            .iter()
            .copied()
            .filter(|k| self.models[k.index()].is_some())
            .collect()
    }

    /// The rest-pose (yaw 0, origin, no walk/look, time 0) world-space
    /// quads of a mob, with each quad's vanilla face label and world normal
    /// — the geometric ground truth `rewo mobshot --check` compares renders
    /// against. Runs the SAME `part_transforms` as `emit_model` (with the
    /// same zeroed inputs the check renders with), so the prediction can
    /// never disagree with the renderer's math.
    pub fn neutral_quads(
        &self,
        kind: EntityModelKind,
    ) -> Option<Vec<([[f32; 3]; 4], Facing, [f32; 3])>> {
        let model = self.models[kind.index()].as_ref()?;
        let s = model.scale;
        let ctx = AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age: 0.0,
            gesture: Option::None,
            shell: false,
        };
        let xf = part_transforms(model, &ctx);
        Some(
            model
                .quads
                .iter()
                // Rest pose: no shell, no gesture — shell-only and
                // gesture-only parts don't render, so exclude them from the
                // geometric prediction too.
                .filter(|q| {
                    matches!(
                        model.parts[q.part as usize].show,
                        mobs::Show::Always | mobs::Show::NotShell
                    )
                })
                .map(|q| {
                    let (m, o) = &xf[q.part as usize];
                    let mut pos = [[0f32; 3]; 4];
                    for (i, c) in q.pos.iter().enumerate() {
                        let r = mat_apply(m, *c);
                        let v = [r[0] + o[0], r[1] + o[1], r[2] + o[2]];
                        // model → entity local, then the yaw-0 rotY(180°).
                        let e = [-v[0], mobs::MODEL_EYE_Y - v[1], v[2]];
                        pos[i] = [-e[0] * s, e[1] * s, -e[2] * s];
                    }
                    // Normals ride the same rotations (no translate).
                    let n = mat_apply(m, q.normal);
                    (pos, q.facing, [n[0], -n[1], -n[2]])
                })
                .collect(),
        )
    }

    /// Rebuild this frame's vertex soup. `cam_right`/`cam_up` orient the
    /// nametag billboards (world-space unit vectors from the camera);
    /// `time` (seconds) drives the ambient animations (wing flutter, rod
    /// orbits, tentacle sway — vanilla's `ageInTicks` = `time · 20`).
    pub fn set_draws(
        &mut self,
        draws: &[EntityDraw<'_>],
        cam_right: [f32; 3],
        cam_up: [f32; 3],
        time: f32,
    ) {
        self.cursor = (self.cursor + 1) % RING;
        let mut verts: Vec<Vertex> = Vec::with_capacity(1024);

        // Fixed sun for capsule shading (matches the terrain's lit look).
        let sun = norm3([0.45, 0.8, 0.35]);
        for d in draws {
            if let Some(model) = &self.models[d.kind.index()] {
                self.emit_model(&mut verts, d, model, time);
                continue;
            }
            let base = d.color;
            for (p, n) in &self.capsule {
                if verts.len() >= MAX_VERTS {
                    break;
                }
                let shade = 0.55 + 0.45 * (n[0] * sun[0] + n[1] * sun[1] + n[2] * sun[2]).max(0.0);
                verts.push(Vertex {
                    pos: [
                        d.pos[0] + p[0] * d.width,
                        d.pos[1] + p[1] * d.height,
                        d.pos[2] + p[2] * d.width,
                    ],
                    uv: self.white_uv,
                    color: [base[0] * shade, base[1] * shade, base[2] * shade, 1.0],
                });
            }
        }
        let solid = verts.len();

        if self.has_font {
            for d in draws {
                let Some(name) = d.name else { continue };
                self.push_tag(&mut verts, d, name, cam_right, cam_up);
            }
        }
        let total = verts.len();
        if total >= MAX_VERTS {
            log::warn!("entities: vertex budget hit — some entities/tags dropped");
        }

        self.solid_verts = solid as u32;
        self.text_verts = (total - solid) as u32;
        if let Some(slice) = self.allocs[self.cursor]
            .as_mut()
            .and_then(|a| a.mapped_slice_mut())
        {
            let bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(verts.as_ptr() as *const u8, total * 36)
            };
            slice[..bytes.len()].copy_from_slice(bytes);
        }
    }

    /// Draw one mob model: per-part vanilla `setupAnim` rotation about the
    /// part's pivot in model space, then vanilla's entity transform —
    /// `rotY(180° − yaw) · scale(−1,−1,1) · translate(0,−1.501,0)` — scaled
    /// px→blocks and placed at the entity's feet. Texels ride the
    /// alpha-test (`discard`) path.
    fn emit_model(
        &self,
        verts: &mut Vec<Vertex>,
        d: &EntityDraw<'_>,
        model: &MobModel,
        time: f32,
    ) {
        let theta = (180.0 - d.yaw).to_radians();
        let (st, ct) = theta.sin_cos();
        let ctx = AnimCtx {
            pitch: d.pitch.to_radians(),
            // Vanilla head yaw is net-of-body (`netHeadYaw`), rotated about
            // the head part's own pivot.
            net: wrap_degrees(d.head_yaw - d.yaw).to_radians(),
            f: d.limb_swing * 0.6662,
            pos: d.limb_swing,
            amt: d.limb_amount,
            age: time * 20.0,
            gesture: d.gesture,
            shell: d.shell,
        };
        let xf = part_transforms(model, &ctx);
        // Per-entity render scale (slime/magma size) on top of the baked px→
        // block scale — vanilla scales the whole model uniformly by `size`.
        let s = model.scale * if d.scale_mul > 0.0 { d.scale_mul } else { 1.0 };
        for q in &model.quads {
            if verts.len() + 6 > MAX_VERTS {
                return;
            }
            match model.parts[q.part as usize].show {
                mobs::Show::Always => {}
                mobs::Show::ShellOnly if !d.shell => continue,
                mobs::Show::NotShell if d.shell => continue,
                mobs::Show::During(g)
                    if !matches!(d.gesture, Some((active, _)) if active == g) =>
                {
                    continue
                }
                _ => {}
            }
            let (m, o) = &xf[q.part as usize];
            let mut p4 = [[0f32; 3]; 4];
            for (i, corner) in q.pos.iter().enumerate() {
                let r = mat_apply(m, *corner);
                let v = [r[0] + o[0], r[1] + o[1], r[2] + o[2]];
                // model → entity local (px): scale(−1,−1,1) after the
                // −1.501-block translate.
                let e = [-v[0], mobs::MODEL_EYE_Y - v[1], v[2]];
                // rotY(180° − yaw).
                let x = e[0] * ct + e[2] * st;
                let z = -e[0] * st + e[2] * ct;
                p4[i] = [
                    d.pos[0] + x * s,
                    d.pos[1] + e[1] * s,
                    d.pos[2] + z * s,
                ];
            }
            let c = q.shade;
            // Player skin: shift the (default-Steve) UVs onto this player's
            // uploaded slot. Same 64² layout, so a constant offset suffices.
            let du = d.skin_uv.unwrap_or([0.0, 0.0]);
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                verts.push(Vertex {
                    pos: p4[i],
                    uv: [q.uv[i][0] + du[0], q.uv[i][1] + du[1]],
                    color: [c, c, c, 1.0],
                });
            }
        }
    }

    /// Glyph + background quads for one nametag, camera-billboarded.
    fn push_tag(
        &self,
        verts: &mut Vec<Vertex>,
        d: &EntityDraw<'_>,
        name: &str,
        right: [f32; 3],
        up: [f32; 3],
    ) {
        let cell = self.cell as f32;
        let scale = TAG_PX * (8.0 / cell);
        let total_px: f32 = name
            .bytes()
            .map(|b| self.advance[b as usize] as f32)
            .sum();
        let anchor = [
            d.pos[0],
            d.pos[1] + d.height + TAG_LIFT,
            d.pos[2],
        ];
        let world = |px: f32, py: f32| -> [f32; 3] {
            [
                anchor[0] + (right[0] * px + up[0] * py) * scale,
                anchor[1] + (right[1] * px + up[1] * py) * scale,
                anchor[2] + (right[2] * px + up[2] * py) * scale,
            ]
        };
        let mut quad = |x0: f32, y0: f32, x1: f32, y1: f32, uv: [f32; 4], color: [f32; 4]| {
            if verts.len() + 6 > MAX_VERTS {
                return;
            }
            let (p00, p10, p11, p01) = (world(x0, y0), world(x1, y0), world(x1, y1), world(x0, y1));
            let [u0, v0, u1, v1] = uv;
            for (p, u, v) in [
                (p00, u0, v0),
                (p10, u1, v0),
                (p11, u1, v1),
                (p00, u0, v0),
                (p11, u1, v1),
                (p01, u0, v1),
            ] {
                verts.push(Vertex {
                    pos: p,
                    uv: [u, v],
                    color,
                });
            }
        };

        // Background: vanilla's 25% black plate, 1px padding.
        let (bx0, bx1) = (-total_px / 2.0 - 1.0, total_px / 2.0 + 1.0);
        let wu = self.white_uv;
        quad(
            bx0,
            -1.0,
            bx1,
            cell + 1.0,
            [wu[0], wu[1], wu[0], wu[1]],
            [0.0, 0.0, 0.0, 0.25],
        );

        // Glyphs, left to right. v axis: image y is down, glyph py is up.
        let (aw, ah) = (ATLAS_W as f32, ATLAS_H as f32);
        let mut pen = -total_px / 2.0;
        for b in name.bytes() {
            let adv = self.advance[b as usize] as f32;
            if b != b' ' {
                let (cx, cy) = (
                    (b as u32 % 16 * self.cell) as f32,
                    (b as u32 / 16 * self.cell) as f32,
                );
                quad(
                    pen,
                    0.0,
                    pen + cell,
                    cell,
                    [cx / aw, (cy + cell) / ah, (cx + cell) / aw, cy / ah],
                    [1.0, 1.0, 1.0, 1.0],
                );
            }
            pen += adv;
        }
    }

    /// Shared draw state (viewport, descriptor, push, vertex buffer).
    unsafe fn bind_common(&self, gpu: &Gpu, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4], extent: vk::Extent2D) {
        let device = &gpu.device;
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
            self.layout,
            0,
            &[self.set],
            &[],
        );
        device.cmd_push_constants(
            cb,
            self.layout,
            vk::ShaderStageFlags::VERTEX,
            0,
            std::slice::from_raw_parts(view_proj.as_ptr() as *const u8, 64),
        );
        device.cmd_bind_vertex_buffers(cb, 0, &[self.bufs[self.cursor]], &[0]);
    }

    /// Opaque capsules — draw before any translucent content.
    pub fn draw_solid(&self, gpu: &Gpu, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4], extent: vk::Extent2D) {
        if self.solid_verts == 0 {
            return;
        }
        unsafe {
            self.bind_common(gpu, cb, view_proj, extent);
            let device = &gpu.device;
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.solid_pipeline);
            device.cmd_draw(cb, self.solid_verts, 1, 0, 0);
        }
    }

    /// Blended nametag text — draw last (after water).
    pub fn draw_text(&self, gpu: &Gpu, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4], extent: vk::Extent2D) {
        if self.text_verts == 0 {
            return;
        }
        unsafe {
            self.bind_common(gpu, cb, view_proj, extent);
            let device = &gpu.device;
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.text_pipeline);
            device.cmd_draw(cb, self.text_verts, 1, self.solid_verts, 0);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        unsafe {
            let device = &gpu.device;
            device.destroy_pipeline(self.solid_pipeline, None);
            device.destroy_pipeline(self.text_pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            for b in self.bufs {
                device.destroy_buffer(b, None);
            }
        }
        for a in self.allocs.iter_mut().filter_map(|a| a.take()) {
            let _ = gpu.allocator.free(a);
        }
        if let Some(a) = self.image_alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

/// Unit capsule triangle soup: y ∈ [0, 1], xz ∈ [−0.5, 0.5]. Profile =
/// quarter-sphere cap (3 bands) + cylinder + cap, swept around Y.
fn unit_capsule() -> Vec<([f32; 3], [f32; 3])> {
    // (radius, y, n_radial, n_y) per profile row.
    let mut profile: Vec<(f32, f32, f32, f32)> = Vec::new();
    for i in 0..=3 {
        let th = (-90.0 + 30.0 * i as f32).to_radians();
        profile.push((
            0.5 * th.cos(),
            0.25 * (1.0 + th.sin()),
            th.cos(),
            th.sin(),
        ));
    }
    profile.push((0.5, 0.75, 1.0, 0.0));
    for i in 1..=3 {
        let th = (30.0 * i as f32).to_radians();
        profile.push((
            0.5 * th.cos(),
            0.75 + 0.25 * th.sin(),
            th.cos(),
            th.sin(),
        ));
    }

    let mut out = Vec::with_capacity((profile.len() - 1) * SEGMENTS * 6);
    let ring = |row: &(f32, f32, f32, f32), s: usize| -> ([f32; 3], [f32; 3]) {
        let a = (s as f32 / SEGMENTS as f32) * std::f32::consts::TAU;
        let (r, y, nr, ny) = *row;
        (
            [r * a.cos(), y, r * a.sin()],
            norm3([nr * a.cos(), ny, nr * a.sin()]),
        )
    };
    for band in profile.windows(2) {
        for s in 0..SEGMENTS {
            let (a0, a1) = (s, (s + 1) % SEGMENTS);
            let (p00, p10) = (ring(&band[0], a0), ring(&band[0], a1));
            let (p01, p11) = (ring(&band[1], a0), ring(&band[1], a1));
            out.extend_from_slice(&[p00, p10, p11, p00, p11, p01]);
        }
    }
    out
}

fn norm3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l, v[2] / l]
}

// ---- per-part animation (vanilla setupAnim formulas) ---------------------

/// Per-frame animation inputs, in vanilla's units: look angles in radians,
/// `f` = walkAnimationPos·0.6662, `pos` = raw walkAnimationPos, `amt` =
/// walkAnimationSpeed, `age` = ageInTicks (wall-clock seconds × 20).
struct AnimCtx {
    pitch: f32,
    net: f32,
    f: f32,
    pos: f32,
    amt: f32,
    age: f32,
    /// Active gesture + its age in seconds (one-shot rigs).
    gesture: Option<(mobs::Gesture, f32)>,
    /// Armadillo shell swap.
    shell: bool,
}

const DEG: f32 = std::f32::consts::PI / 180.0;

/// Vanilla `Mth.triangleWave`.
fn triangle_wave(a: f32, b: f32) -> f32 {
    ((a.rem_euclid(b) - b * 0.5).abs() - b * 0.25) / (b * 0.25)
}

/// One part's `setupAnim` output: Euler deltas summed onto the base pose
/// (composed as a single `rotateZYX`, exactly vanilla) + a pivot offset for
/// the anims that move parts (blaze rods, crawler segments).
fn anim_delta(anim: mobs::Anim, amp: f32, c: &AnimCtx) -> ([f32; 3], [f32; 3]) {
    use mobs::Anim::*;
    use std::f32::consts::PI;
    let mut rot = [0.0f32; 3];
    let mut off = [0.0f32; 3];
    match anim {
        None => {}
        Head => {
            rot[0] = c.pitch;
            rot[1] = c.net;
        }
        ArmRight => rot[0] = (c.f + PI).cos() * 2.0 * c.amt * 0.5 * amp,
        ArmLeft => rot[0] = c.f.cos() * 2.0 * c.amt * 0.5 * amp,
        LegRight | QuadHindRight | QuadFrontLeft => rot[0] = c.f.cos() * 1.4 * c.amt * amp,
        LegLeft | QuadHindLeft | QuadFrontRight => rot[0] = (c.f + PI).cos() * 1.4 * c.amt * amp,
        SpiderLeg { left, phase } => {
            let swing = -((c.f * 2.0 + phase).cos()) * 0.4 * c.amt;
            let step = (c.f + phase).sin().abs() * 0.4 * c.amt;
            let dir = if left { -1.0 } else { 1.0 };
            rot[1] = dir * swing;
            rot[2] = dir * step;
        }
        TailWagY => rot[1] = c.f.cos() * 1.4 * c.amt,
        GolemArmRight => rot[0] = (-0.2 + 1.5 * triangle_wave(c.pos, 13.0)) * c.amt,
        GolemArmLeft => rot[0] = (-0.2 - 1.5 * triangle_wave(c.pos, 13.0)) * c.amt,
        GolemLegRight => rot[0] = -1.5 * triangle_wave(c.pos, 13.0) * c.amt,
        GolemLegLeft => rot[0] = 1.5 * triangle_wave(c.pos, 13.0) * c.amt,
        BlazeRod { ring, idx } => {
            // Vanilla repositions each rod: ring parameters (start angle,
            // spin speed, radius, base y) + a per-rod bob. `i` is the rod's
            // global 0..12 index in the y-bob term.
            let i = ring as f32 * 4.0 + idx as f32;
            let (a0, speed, r, y0) = match ring {
                0 => (0.0, -0.1, 9.0, -2.0),
                1 => (PI / 4.0, 0.03, 7.0, 2.0),
                _ => (0.47123894, -0.05, 5.0, 11.0),
            };
            let a = a0 + c.age * PI * speed + idx as f32 * (PI / 2.0);
            let y = if ring < 2 {
                y0 + ((i * 2.0 + c.age) * 0.25).cos()
            } else {
                y0 + ((i * 1.5 + c.age) * 0.5).cos()
            };
            off = [a.cos() * r, y, a.sin() * r];
        }
        GhastTentacle { i } => rot[0] = 0.2 * (c.age * 0.3 + i as f32).sin() + 0.4,
        SquidTentacle => rot[0] = 0.3 * (0.5 + 0.5 * (c.age * 0.2).sin()),
        PhantomWing { left } => {
            let flap = c.age * 7.448451 * DEG;
            let z = flap.cos() * 16.0 * DEG;
            rot[2] = if left { z } else { -z };
        }
        PhantomTail => {
            let flap = c.age * 7.448451 * DEG;
            rot[0] = -(5.0 + (flap * 2.0).cos() * 5.0) * DEG;
        }
        AllayWing { left } => {
            let flap = (c.age * 20.0 * DEG + c.pos).cos() * PI * 0.15 + c.amt;
            let fly = (c.amt / 0.3).min(1.0);
            rot[0] = 0.43633232 * (1.0 - fly);
            rot[1] = if left { PI / 4.0 - flap } else { -PI / 4.0 + flap };
        }
        VexArm { left } => {
            let bob = (c.age * 5.5 * DEG).cos() * 0.1;
            rot[2] = if left { -(PI / 5.0 + bob) } else { PI / 5.0 + bob };
        }
        VexWing { left } => {
            let y = 1.0995574 + (c.age * 45.836624 * DEG).cos() * 16.2 * DEG;
            rot[0] = 0.47123888;
            rot[1] = if left { y } else { -y };
            rot[2] = if left { -0.47123888 } else { 0.47123888 };
        }
        BeeWing { left } => {
            let z = (c.age * 120.32113 * DEG).cos() * PI * 0.15;
            rot[2] = if left { -z } else { z };
        }
        FishTail { amp: a, speed } => rot[1] = -a * (speed * c.age).sin(),
        PufferFin { left } => {
            let s = (c.age * 0.2).sin();
            rot[2] = if left { 0.2 - 0.4 * s } else { -0.2 + 0.4 * s };
        }
        DolphinTail { k } => {
            let gate = (c.amt * 6.0).min(1.0);
            rot[0] = -k * (c.age * 0.3).cos() * gate;
        }
        Crawl { i, ry, tx, with_x } => {
            let ph = c.age * 0.9 + i as f32 * 0.15 * PI;
            let w = (i as i32 - 2).abs() as f32;
            rot[1] = ph.cos() * PI * ry * (1.0 + w);
            if with_x {
                off[0] = ph.sin() * PI * tx * w;
            }
        }
    }
    (rot, off)
}

/// Sample a keyframe channel at `t` seconds — vanilla `Entry.apply`: prev =
/// last frame at-or-before, interpolation mode from the NEXT frame,
/// catmull-rom over the surrounding four.
fn kf_sample(frames: &[mobs::KfFrame], t: f32) -> [f32; 3] {
    let mut prev = 0usize;
    for (i, f) in frames.iter().enumerate() {
        if t <= f.t {
            break;
        }
        prev = i;
    }
    let next = (prev + 1).min(frames.len() - 1);
    let (fp, fn_) = (&frames[prev], &frames[next]);
    let alpha = if next != prev && fn_.t > fp.t {
        ((t - fp.t) / (fn_.t - fp.t)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if fn_.catmullrom {
        let p0 = &frames[prev.saturating_sub(1)].v;
        let p3 = &frames[(next + 1).min(frames.len() - 1)].v;
        let mut out = [0.0f32; 3];
        for i in 0..3 {
            out[i] = catmullrom(alpha, p0[i], fp.v[i], fn_.v[i], p3[i]);
        }
        out
    } else {
        let mut out = [0.0f32; 3];
        for i in 0..3 {
            out[i] = fp.v[i] + (fn_.v[i] - fp.v[i]) * alpha;
        }
        out
    }
}

/// Vanilla `Mth.catmullrom`.
fn catmullrom(a: f32, p0: f32, p1: f32, p2: f32, p3: f32) -> f32 {
    0.5 * (2.0 * p1
        + (p2 - p0) * a
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * a * a
        + (3.0 * p1 - p0 - 3.0 * p2 + p3) * a * a * a)
}

/// A keyframe driver's (time seconds, value scale) — vanilla's
/// `applyWalk`/`apply` call sites.
fn kf_drive(driver: mobs::KfDriver, c: &AnimCtx) -> (f32, f32) {
    use mobs::KfDriver::*;
    match driver {
        Walk { speed_f, scale_f } => (c.pos * 0.05 * speed_f, (c.amt * scale_f).min(1.0)),
        WalkPlusAge { age_div, amt_add, speed_f, scale_f } => (
            (c.pos + c.age / age_div) * 0.05 * speed_f,
            ((c.amt + amt_add) * scale_f).min(1.0),
        ),
        Age => (c.age * 0.05, 1.0),
        AgeGatedByWalk { gate } => (c.age * 0.05, (c.amt * gate).min(1.0)),
        // Only reachable under KfGate::During — the gesture is active.
        GestureAge => match c.gesture {
            Some((_, age)) => (age, 1.0),
            Option::None => (0.0, 0.0),
        },
    }
}

/// Compose every part's global transform `(M, o)` — child-in-parent
/// hierarchy with vanilla's `translate(pivot); rotateZYX(base + delta)`
/// per level, plus any keyframe-rig contributions (which ADD onto the
/// procedural deltas, exactly like vanilla's `offsetRotation`/`offsetPos`).
/// Shared by rendering and the mobshot geometric prediction so the two can
/// never disagree.
fn part_transforms(model: &MobModel, ctx: &AnimCtx) -> Vec<([[f32; 3]; 3], [f32; 3])> {
    let n = model.parts.len();
    let mut drots = vec![[0.0f32; 3]; n];
    let mut doffs = vec![[0.0f32; 3]; n];
    for (i, p) in model.parts.iter().enumerate() {
        let (r, o) = anim_delta(p.anim, p.amp, ctx);
        drots[i] = r;
        doffs[i] = o;
    }
    for kf in &model.keyframes {
        // The gate decides whether the rig plays this frame (vanilla's
        // `setupAnim` call patterns); the driver decides where in time.
        match kf.gate {
            mobs::KfGate::Always => {}
            mobs::KfGate::During(g) => {
                if !matches!(ctx.gesture, Some((active, _)) if active == g) {
                    continue;
                }
            }
            mobs::KfGate::Unless(g) => {
                if matches!(ctx.gesture, Some((active, _)) if active == g) {
                    continue;
                }
            }
            mobs::KfGate::NotShell => {
                if ctx.shell {
                    continue;
                }
            }
        }
        let (mut t, scale) = kf_drive(kf.driver, ctx);
        if scale <= 0.0 {
            continue;
        }
        if kf.def.looping {
            t = t.rem_euclid(kf.def.length);
        }
        for (ch, &pi) in kf.def.channels.iter().zip(&kf.parts) {
            let v = kf_sample(ch.frames, t);
            let dst = match ch.target {
                mobs::KfTarget::Rot => &mut drots[pi as usize],
                mobs::KfTarget::Pos => &mut doffs[pi as usize],
            };
            for i in 0..3 {
                dst[i] += v[i] * scale;
            }
        }
    }
    let mut out: Vec<([[f32; 3]; 3], [f32; 3])> = Vec::with_capacity(n);
    for (i, p) in model.parts.iter().enumerate() {
        let e = [
            p.rot[0] + drots[i][0],
            p.rot[1] + drots[i][1],
            p.rot[2] + drots[i][2],
        ];
        let r = mat_zyx(e);
        let pivot = [
            p.pivot[0] + doffs[i][0],
            p.pivot[1] + doffs[i][1],
            p.pivot[2] + doffs[i][2],
        ];
        let (m, o) = match p.parent {
            Some(par) => {
                let (pm, po) = &out[par as usize];
                let gp = mat_apply(pm, pivot);
                (mat_mul(*pm, r), [gp[0] + po[0], gp[1] + po[1], gp[2] + po[2]])
            }
            Option::None => (r, pivot),
        };
        out.push((m, o));
    }
    out
}

// ---- small math helpers for the per-part animation ----------------------

const IDENTITY3: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// `rotateZYX(z, y, x)` as a matrix: `Rz·Ry·Rx` (x applied first) —
/// matches `mobs::rotate_zyx`.
fn mat_zyx(e: [f32; 3]) -> [[f32; 3]; 3] {
    if e == [0.0; 3] {
        return IDENTITY3;
    }
    mat_mul(mat_mul(rot_z(e[2]), rot_y(e[1])), rot_x(e[0]))
}

/// Rotation about Z.
fn rot_z(a: f32) -> [[f32; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// Rotation about X (y-down model space, matches `mobs::rotate_zyx`).
fn rot_x(a: f32) -> [[f32; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

/// Rotation about Y.
fn rot_y(a: f32) -> [[f32; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

/// `a · b` (row-major; `b` applies to the vector first).
fn mat_mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}

fn mat_apply(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Wrap to [−180, 180) — vanilla `Mth.wrapDegrees` for the net head yaw.
fn wrap_degrees(deg: f32) -> f32 {
    (deg + 180.0).rem_euclid(360.0) - 180.0
}

/// Fill a quad's UV bounding rect in the atlas with its face-label color
/// (facelabel verification mode). Sub-pixel rects round outward; rects are
/// clamped to the texture's slot. Returns `false` if a texel was already
/// painted a *different* label (rect reuse across faces — the check can't
/// apply to that texture).
fn paint_debug_rect(
    atlas: &mut [u8],
    slot: (u32, u32, u32, u32),
    q: &mobs::RawQuad,
    painted: &mut std::collections::HashMap<(u32, u32), [u8; 3]>,
) -> bool {
    let (ox, oy, tw, th) = slot;
    let (min_u, max_u) = q
        .uv
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
    let (min_v, max_v) = q
        .uv
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[1]), hi.max(p[1])));
    let color = q.facing.debug_color();
    let [r, g, b] = color;
    let (x0, x1) = ((min_u.floor().max(0.0)) as u32, (max_u.ceil().max(0.0) as u32).min(tw));
    let (y0, y1) = ((min_v.floor().max(0.0)) as u32, (max_v.ceil().max(0.0) as u32).min(th));
    let mut clean = true;
    for y in y0..y1 {
        for x in x0..x1 {
            match painted.insert((ox + x, oy + y), color) {
                Some(prev) if prev != color => clean = false,
                _ => {}
            }
            let i = (((oy + y) * ATLAS_W + ox + x) * 4) as usize;
            atlas[i..i + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    clean
}

/// sRGB component → linear (CPU-side color prep; discipline #1).
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Blit a `w`×`h` RGBA entity skin into the atlas at (x, y). Returns whether
/// it was present + correctly sized.
fn blit_tex(atlas: &mut [u8], tex: Option<&[u8]>, x: u32, y: u32, w: u32, h: u32) -> bool {
    let (w, h) = (w as usize, h as usize);
    match tex {
        Some(px) if px.len() == w * h * 4 => {
            for row in 0..h {
                let src = row * w * 4;
                let dst = ((y as usize + row) * ATLAS_W as usize + x as usize) * 4;
                atlas[dst..dst + w * 4].copy_from_slice(&px[src..src + w * 4]);
            }
            true
        }
        _ => false,
    }
}

/// Copy `rgba` (`width*height*4`) into an already-initialized, already-
/// sampled atlas `image` at offset (x, y): SHADER_READ_ONLY → TRANSFER_DST
/// → SHADER_READ_ONLY, fence-waited. `wait_idle` first so no in-flight
/// frame samples the atlas mid-write (rare call — see `upload_skin`).
fn upload_region(
    gpu: &mut Gpu,
    image: vk::Image,
    rgba: &[u8],
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let expect = (width * height * 4) as usize;
    if rgba.len() != expect {
        return Err(format!("upload_region: {} bytes, want {expect}", rgba.len()));
    }
    gpu.wait_idle();
    unsafe {
        let device = gpu.device.clone();
        let staging = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(rgba.len() as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("skin staging: {e}"))?;
        let sreq = device.get_buffer_memory_requirements(staging);
        let mut salloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name: "skin-staging",
                requirements: sreq,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("skin staging alloc: {e}"))?;
        device
            .bind_buffer_memory(staging, salloc.memory(), salloc.offset())
            .map_err(|e| format!("skin staging bind: {e}"))?;
        salloc.mapped_slice_mut().ok_or("skin staging not mapped")?[..rgba.len()]
            .copy_from_slice(rgba);

        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.graphics_family),
                None,
            )
            .map_err(|e| format!("skin pool: {e}"))?;
        let cb = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .map_err(|e| format!("skin cb: {e}"))?[0];
        device
            .begin_command_buffer(
                cb,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|e| format!("skin begin: {e}"))?;
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let to_dst = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .src_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .image(image)
            .subresource_range(range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_dst)),
        );
        device.cmd_copy_buffer_to_image(
            cb,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_offset(vk::Offset3D { x: x as i32, y: y as i32, z: 0 })
                .image_extent(vk::Extent3D { width, height, depth: 1 })],
        );
        let to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(image)
            .subresource_range(range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_read)),
        );
        device.end_command_buffer(cb).map_err(|e| format!("skin end: {e}"))?;
        let fence = device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .map_err(|e| format!("skin fence: {e}"))?;
        let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(cb)];
        device
            .queue_submit2(
                gpu.graphics_queue,
                &[vk::SubmitInfo2::default().command_buffer_infos(&cbs)],
                fence,
            )
            .map_err(|e| format!("skin submit: {e}"))?;
        device
            .wait_for_fences(&[fence], true, u64::MAX)
            .map_err(|e| format!("skin wait: {e}"))?;
        device.destroy_fence(fence, None);
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(staging, None);
        let _ = gpu.allocator.free(salloc);
    }
    Ok(())
}

pub(crate) fn create_texture(
    gpu: &mut Gpu,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<(vk::Image, Allocation, vk::ImageView), String> {
    unsafe {
        let device = gpu.device.clone();
        let image = device
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_SRGB)
                    .extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
            .map_err(|e| format!("font image: {e}"))?;
        let req = device.get_image_memory_requirements(image);
        let alloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name: "font-atlas",
                requirements: req,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("font alloc: {e}"))?;
        device
            .bind_image_memory(image, alloc.memory(), alloc.offset())
            .map_err(|e| format!("font bind: {e}"))?;

        // One-shot staged upload (transient pool — init-time only).
        let staging = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(rgba.len() as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("font staging: {e}"))?;
        let sreq = device.get_buffer_memory_requirements(staging);
        let mut salloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name: "font-staging",
                requirements: sreq,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("font staging alloc: {e}"))?;
        device
            .bind_buffer_memory(staging, salloc.memory(), salloc.offset())
            .map_err(|e| format!("font staging bind: {e}"))?;
        salloc
            .mapped_slice_mut()
            .ok_or("font staging not mapped")?[..rgba.len()]
            .copy_from_slice(rgba);

        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.graphics_family),
                None,
            )
            .map_err(|e| format!("font pool: {e}"))?;
        let cb = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .map_err(|e| format!("font cb: {e}"))?[0];
        device
            .begin_command_buffer(
                cb,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|e| format!("font begin: {e}"))?;
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let to_dst = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::NONE)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .image(image)
            .subresource_range(range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_dst)),
        );
        device.cmd_copy_buffer_to_image(
            cb,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                })],
        );
        let to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(image)
            .subresource_range(range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_read)),
        );
        device
            .end_command_buffer(cb)
            .map_err(|e| format!("font end: {e}"))?;
        let fence = device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .map_err(|e| format!("font fence: {e}"))?;
        let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(cb)];
        device
            .queue_submit2(
                gpu.graphics_queue,
                &[vk::SubmitInfo2::default().command_buffer_infos(&cbs)],
                fence,
            )
            .map_err(|e| format!("font submit: {e}"))?;
        device
            .wait_for_fences(&[fence], true, u64::MAX)
            .map_err(|e| format!("font wait: {e}"))?;
        device.destroy_fence(fence, None);
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(staging, None);
        let _ = gpu.allocator.free(salloc);

        let view = device
            .create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_SRGB)
                    .subresource_range(range),
                None,
            )
            .map_err(|e| format!("font view: {e}"))?;
        Ok((image, alloc, view))
    }
}

fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
    solid: bool,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/entity.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/entity.frag.spv")),
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
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(20),
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
            .depth_write_enable(solid)
            .depth_compare_op(vk::CompareOp::GREATER); // reversed-Z
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(!solid)
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
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
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
            .map_err(|(_, e)| format!("entity pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capsule_stays_inside_the_unit_bounds() {
        for (p, n) in unit_capsule() {
            assert!((0.0..=1.0).contains(&p[1]), "y {}", p[1]);
            assert!(p[0].abs() <= 0.5 + 1e-5 && p[2].abs() <= 0.5 + 1e-5);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-3, "normal not unit: {len}");
        }
    }

    #[test]
    fn srgb_linearize_endpoints() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        assert!(srgb_to_linear(0.5) < 0.5, "midtones darken in linear");
    }

    #[test]
    fn wrap_degrees_matches_vanilla() {
        assert_eq!(wrap_degrees(0.0), 0.0);
        assert_eq!(wrap_degrees(190.0), -170.0);
        assert_eq!(wrap_degrees(-190.0), 170.0);
        assert_eq!(wrap_degrees(360.0), 0.0);
        // body 350, head 10 → net +20 (turned left of body), not −340.
        assert_eq!(wrap_degrees(10.0 - 350.0), 20.0);
    }

    #[test]
    fn head_anim_matrix_matches_vanilla_rotate_zyx() {
        // Ry(net)·Rx(pitch) must equal mobs::rotate_zyx(v, [pitch, net, 0]).
        let (pitch, net) = (0.6f32, -1.1f32);
        let m = mat_mul(rot_y(net), rot_x(pitch));
        for v in [[1.0, 0.0, 0.0], [0.3, -0.7, 0.9], [0.0, 1.0, -1.0]] {
            let a = mat_apply(&m, v);
            let b = mobs::rotate_zyx(v, [pitch, net, 0.0]);
            for i in 0..3 {
                assert!((a[i] - b[i]).abs() < 1e-5, "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn shelf_packing_avoids_font_and_overlaps() {
        // A realistic mix: several 64², 64×32, a couple of 128², one 192².
        let mut sizes: Vec<(u32, u32)> = Vec::new();
        sizes.push((192, 192));
        for _ in 0..8 {
            sizes.push((128, 128));
        }
        for _ in 0..30 {
            sizes.push((64, 64));
        }
        for _ in 0..25 {
            sizes.push((64, 32));
        }
        sizes.push((16, 16));
        // Sorted tallest-first like the constructor does.
        sizes.sort_by_key(|&(w, h)| (std::cmp::Reverse(h), std::cmp::Reverse(w)));
        let placed = pack_shelves(&sizes);
        let mut rects: Vec<(u32, u32, u32, u32)> = Vec::new();
        for (i, p) in placed.iter().enumerate() {
            let (w, h) = sizes[i];
            let (x, y) = p.expect("everything must pack at 1024²");
            assert!(x + w <= ATLAS_W && y + h <= ATLAS_H);
            // Never inside the font block.
            assert!(x >= 128 || y >= 128, "({x},{y}) overlaps font");
            for &(rx, ry, rw, rh) in &rects {
                let overlap = x < rx + rw && rx < x + w && y < ry + rh && ry < y + h;
                assert!(!overlap, "({x},{y},{w},{h}) overlaps ({rx},{ry},{rw},{rh})");
            }
            rects.push((x, y, w, h));
        }
    }
}
