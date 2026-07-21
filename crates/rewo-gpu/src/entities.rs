//! Entity pass — capsules + floating nametags (REWO_PLAN correction #11's
//! v1 scope: "entities as capsules + nametags"; the player-model port is a
//! later track).
//!
//! Deliberately simple: geometry is a **CPU-built world-space triangle
//! soup** rebuilt every frame — capsule shells (~500 verts each) and
//! camera-billboarded glyph quads. Entity counts are dozens, not thousands,
//! so building on the CPU costs microseconds and avoids instancing +
//! billboard shaders entirely; revisit if entity counts ever grow 100×.
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

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::world::DEPTH_FORMAT;
use crate::Gpu;

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
    /// Render the textured player model instead of a capsule (falls back
    /// to the capsule when no skin was provided).
    pub player: bool,
    /// Body yaw (degrees, MC convention) — rotates the whole model.
    pub yaw: f32,
    /// Look pitch (degrees) — tilts the head about the neck.
    pub pitch: f32,
    /// Walk-cycle phase + amplitude (vanilla limbSwing / limbSwingAmount)
    /// — arms and legs swing about their pivots. 0 amount = neutral pose.
    pub limb_swing: f32,
    pub limb_amount: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 3],
    uv: [f32; 2],
    color: [f32; 4],
}

/// Combined entity atlas layout: the font occupies (0,0)..(128,128), the
/// 64×64 player skin sits at (SKIN_X, 0). One texture, one pipeline family.
const ATLAS_W: u32 = 256;
const ATLAS_H: u32 = 128;
const SKIN_X: u32 = 128;

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
    /// Player model quads (model px, atlas-normalized UVs, baked shade).
    player_quads: Vec<PlayerQuad>,
    // Font metrics (identity values when no font was provided).
    cell: u32,
    advance: [u8; 256],
    white_uv: [f32; 2],
    has_font: bool,
    has_skin: bool,
}

/// Which articulated part a player-model quad belongs to — sets its
/// rotation pivot height and swing formula.
#[derive(Clone, Copy, PartialEq)]
enum LimbPart {
    Body,
    Head,
    ArmRight,
    ArmLeft,
    LegRight,
    LegLeft,
}

/// One face of the player model, pre-unwrapped: corners in model pixels
/// (y 0..32, feet at 0), UVs already atlas-normalized, per-face shade.
struct PlayerQuad {
    pos: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
    shade: f32,
    part: LimbPart,
}

impl EntityPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        font: Option<FontData<'_>>,
        skin: Option<&[u8]>,
    ) -> Result<Self, String> {
        let device = gpu.device.clone();

        // ---- combined atlas: font at (0,0), player skin at (SKIN_X,0) ----
        let mut atlas = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
        let (cell, advance, white_texel, has_font) = match &font {
            Some(f) => {
                let side = f.size.min(ATLAS_H).min(SKIN_X) as usize;
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
        let has_skin = match skin {
            Some(px) if px.len() == 64 * 64 * 4 => {
                for row in 0..64usize {
                    let src = row * 64 * 4;
                    let dst = (row * ATLAS_W as usize + SKIN_X as usize) * 4;
                    atlas[dst..dst + 64 * 4].copy_from_slice(&px[src..src + 64 * 4]);
                }
                true
            }
            _ => false,
        };
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
                player_quads: player_model_quads(),
                cell,
                advance,
                white_uv,
                has_font,
                has_skin,
            })
        }
    }

    /// Rebuild this frame's vertex soup. `cam_right`/`cam_up` orient the
    /// nametag billboards (world-space unit vectors from the camera).
    pub fn set_draws(
        &mut self,
        draws: &[EntityDraw<'_>],
        cam_right: [f32; 3],
        cam_up: [f32; 3],
    ) {
        self.cursor = (self.cursor + 1) % RING;
        let mut verts: Vec<Vertex> = Vec::with_capacity(1024);

        // Fixed sun for capsule shading (matches the terrain's lit look).
        let sun = norm3([0.45, 0.8, 0.35]);
        for d in draws {
            if d.player && self.has_skin {
                self.emit_player(&mut verts, d);
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

    /// The textured player model: 12 pre-unwrapped cuboids (6 base + 6
    /// inflated overlay layers). Each quad rotates about its part's pivot
    /// (head pitch, arm/leg walk swing), then the whole model yaws. Skin
    /// texels ride the alpha-test (`discard`) path, so transparent overlay
    /// regions vanish for free.
    fn emit_player(&self, verts: &mut Vec<Vertex>, d: &EntityDraw<'_>) {
        // Vanilla's RenderPlayer scale: 0.9375 model→world, /16 px→blocks.
        const S: f32 = 0.9375 / 16.0;
        let yaw = d.yaw.to_radians();
        let (cy, sy) = (yaw.cos(), yaw.sin());
        let pitch = d.pitch.to_radians();
        // Vanilla HumanoidModel.setupAnim: opposite-phase diagonal gait,
        // arms ±2.0 rad · amount, legs ±1.4 rad · amount, at the 0.6662
        // walk frequency. Head rotation is the look pitch.
        let f = d.limb_swing * 0.6662;
        let amt = d.limb_amount;
        use std::f32::consts::PI;
        for q in &self.player_quads {
            if verts.len() + 6 > MAX_VERTS {
                return;
            }
            let (pivot_y, xrot) = match q.part {
                LimbPart::Body => (0.0, 0.0),
                LimbPart::Head => (24.0, pitch),
                LimbPart::ArmRight => (24.0, (f + PI).cos() * 2.0 * amt),
                LimbPart::ArmLeft => (24.0, f.cos() * 2.0 * amt),
                LimbPart::LegRight => (12.0, f.cos() * 1.4 * amt),
                LimbPart::LegLeft => (12.0, (f + PI).cos() * 1.4 * amt),
            };
            let (cx_, sx_) = (xrot.cos(), xrot.sin());
            let mut p4 = [[0f32; 3]; 4];
            for (i, corner) in q.pos.iter().enumerate() {
                let mut p = *corner;
                // Local rotation about X at the part pivot (+angle tilts the
                // front face toward −Y — look-down / leg-forward).
                if xrot != 0.0 {
                    let (dy, dz) = (p[1] - pivot_y, p[2]);
                    p[1] = pivot_y + dy * cx_ - dz * sx_;
                    p[2] = dy * sx_ + dz * cx_;
                }
                // Whole-model yaw: front (+Z) → MC look dir (−sin, 0, cos).
                let (x, z) = (p[0], p[2]);
                p[0] = x * cy - z * sy;
                p[2] = x * sy + z * cy;
                p4[i] = [
                    d.pos[0] + p[0] * S,
                    d.pos[1] + p[1] * S,
                    d.pos[2] + p[2] * S,
                ];
            }
            let c = q.shade;
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                verts.push(Vertex {
                    pos: p4[i],
                    uv: q.uv[i],
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

/// The wide ("Steve") player model, ported from `ewo-jni/src/skin.rs`
/// (Phase F's battle-tested viewer): 6 base cuboids + 6 inflated overlay
/// layers, standard box-UV unwrap, per-face shades. Model pixels: feet at
/// y=0, head top at y=32; UVs normalized into the skin's atlas slot.
fn player_model_quads() -> Vec<PlayerQuad> {
    struct C {
        min: [f32; 3],
        max: [f32; 3],
        size: [f32; 3],
        uv: (f32, f32),
        part: LimbPart,
    }
    use LimbPart::*;
    #[rustfmt::skip]
    let cuboids = [
        // Base layer: head, body, right/left arm, right/left leg.
        C { min: [-4.0, 24.0, -4.0], max: [4.0, 32.0, 4.0], size: [8.0, 8.0, 8.0], uv: (0.0, 0.0), part: Head },
        C { min: [-4.0, 12.0, -2.0], max: [4.0, 24.0, 2.0], size: [8.0, 12.0, 4.0], uv: (16.0, 16.0), part: Body },
        C { min: [-8.0, 12.0, -2.0], max: [-4.0, 24.0, 2.0], size: [4.0, 12.0, 4.0], uv: (40.0, 16.0), part: ArmRight },
        C { min: [4.0, 12.0, -2.0], max: [8.0, 24.0, 2.0], size: [4.0, 12.0, 4.0], uv: (32.0, 48.0), part: ArmLeft },
        C { min: [-4.0, 0.0, -2.0], max: [0.0, 12.0, 2.0], size: [4.0, 12.0, 4.0], uv: (0.0, 16.0), part: LegRight },
        C { min: [0.0, 0.0, -2.0], max: [4.0, 12.0, 2.0], size: [4.0, 12.0, 4.0], uv: (16.0, 48.0), part: LegLeft },
        // Overlay layer (hat/jacket/sleeves/pants), inflated 0.25.
        C { min: [-4.5, 23.5, -4.5], max: [4.5, 32.5, 4.5], size: [8.0, 8.0, 8.0], uv: (32.0, 0.0), part: Head },
        C { min: [-4.25, 11.75, -2.25], max: [4.25, 24.25, 2.25], size: [8.0, 12.0, 4.0], uv: (16.0, 32.0), part: Body },
        C { min: [-8.25, 11.75, -2.25], max: [-3.75, 24.25, 2.25], size: [4.0, 12.0, 4.0], uv: (40.0, 32.0), part: ArmRight },
        C { min: [3.75, 11.75, -2.25], max: [8.25, 24.25, 2.25], size: [4.0, 12.0, 4.0], uv: (48.0, 48.0), part: ArmLeft },
        C { min: [-4.25, -0.25, -2.25], max: [0.25, 12.25, 2.25], size: [4.0, 12.0, 4.0], uv: (0.0, 32.0), part: LegRight },
        C { min: [-0.25, -0.25, -2.25], max: [4.25, 12.25, 2.25], size: [4.0, 12.0, 4.0], uv: (0.0, 48.0), part: LegLeft },
    ];

    let t = |u: f32, v: f32| -> [f32; 2] {
        [(SKIN_X as f32 + u) / ATLAS_W as f32, v / ATLAS_H as f32]
    };
    let mut out = Vec::with_capacity(cuboids.len() * 6);
    for c in &cuboids {
        let [x0, y0, z0] = c.min;
        let [x1, y1, z1] = c.max;
        let [sw, sh, sd] = c.size;
        let (u, v) = c.uv;
        // Box-UV sub-rect origins (identical to skin.rs::cuboid_faces).
        let (tu, tv) = (u + sd, v); // top
        let (du, dv) = (u + sd + sw, v); // bottom
        let (ru, rv) = (u, v + sd); // right (-X)
        let (fu, fv) = (u + sd, v + sd); // front (+Z)
        let (lu, lv) = (u + sd + sw, v + sd); // left (+X)
        let (bu, bv) = (u + 2.0 * sd + sw, v + sd); // back (-Z)
        let faces: [([[f32; 3]; 4], [[f32; 2]; 4], f32); 6] = [
            (
                [[x0, y1, z1], [x1, y1, z1], [x1, y0, z1], [x0, y0, z1]],
                [t(fu, fv), t(fu + sw, fv), t(fu + sw, fv + sh), t(fu, fv + sh)],
                0.80, // front
            ),
            (
                [[x1, y1, z0], [x0, y1, z0], [x0, y0, z0], [x1, y0, z0]],
                [t(bu, bv), t(bu + sw, bv), t(bu + sw, bv + sh), t(bu, bv + sh)],
                0.62, // back
            ),
            (
                [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
                [t(tu, tv), t(tu + sw, tv), t(tu + sw, tv + sd), t(tu, tv + sd)],
                1.0, // top
            ),
            (
                [[x0, y0, z1], [x1, y0, z1], [x1, y0, z0], [x0, y0, z0]],
                [t(du, dv), t(du + sw, dv), t(du + sw, dv + sd), t(du, dv + sd)],
                0.50, // bottom
            ),
            (
                [[x0, y1, z0], [x0, y1, z1], [x0, y0, z1], [x0, y0, z0]],
                [t(ru, rv), t(ru + sd, rv), t(ru + sd, rv + sh), t(ru, rv + sh)],
                0.68, // right (-X)
            ),
            (
                [[x1, y1, z1], [x1, y1, z0], [x1, y0, z0], [x1, y0, z1]],
                [t(lu, lv), t(lu + sd, lv), t(lu + sd, lv + sh), t(lu, lv + sh)],
                0.68, // left (+X)
            ),
        ];
        for (pos, uv, shade) in faces {
            out.push(PlayerQuad {
                pos,
                uv,
                shade,
                part: c.part,
            });
        }
    }
    out
}

/// sRGB component → linear (CPU-side color prep; discipline #1).
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn create_texture(
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
    fn player_model_unwraps_the_face_where_the_face_is() {
        let quads = player_model_quads();
        assert_eq!(quads.len(), 72, "12 cuboids × 6 faces");
        // Head cuboid (first), front face (first): skin px (8,8)..(16,16),
        // offset into the atlas's skin slot.
        let front = &quads[0];
        assert!(front.part == LimbPart::Head);
        // Legs/arms mapped to distinct parts (drives the swing).
        assert!(quads.iter().any(|q| q.part == LimbPart::LegRight));
        assert!(quads.iter().any(|q| q.part == LimbPart::ArmLeft));
        let u0 = (SKIN_X as f32 + 8.0) / ATLAS_W as f32;
        let v0 = 8.0 / ATLAS_H as f32;
        assert_eq!(front.uv[0], [u0, v0]);
        assert_eq!(
            front.uv[2],
            [(SKIN_X as f32 + 16.0) / ATLAS_W as f32, 16.0 / ATLAS_H as f32]
        );
        // Model bounds: feet at 0, hat top at 32.5 model px.
        let top = quads
            .iter()
            .flat_map(|q| q.pos.iter())
            .map(|p| p[1])
            .fold(f32::MIN, f32::max);
        assert_eq!(top, 32.5);
    }
}
