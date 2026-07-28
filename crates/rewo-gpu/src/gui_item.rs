//! Item icons in hotbar and inventory slots (M34).
//!
//! # Where the geometry comes from
//!
//! Nowhere new. M22 already bakes every item to quads in `0..16` model units
//! with UVs into its own texture, through two very different sources — an
//! extruded sprite and a reused block bake — that converge on the same shape.
//! M34 added the `display.gui` transform those quads are placed by. This module
//! is only the placement, the lighting and the pass.
//!
//! # The placement
//!
//! Vanilla renders a GUI item into a 16×16 screen area by scaling the model
//! into block space, applying `display.gui`, then scaling to 16 pixels per
//! block about the slot's centre:
//!
//! ```text
//! screen = slot_centre
//!        + PX_PER_BLOCK * ( R(gui.rotation) · S(gui.scale) · (model/16 - 0.5)
//!                           + gui.translation )
//! ```
//!
//! The `- 0.5` is what centres the model on the slot, and it is why the two
//! `gui` cases land where they do. A sprite's identity transform maps `0..16`
//! straight onto `-8..8` pixels — exactly filling the slot, which is the whole
//! reason an absent `display.gui` is *correct* for a sprite rather than
//! missing. A block's `scale 0.625` with `rotation [30, 225, 0]` reaches about
//! ±8.7 px on the diagonal, so it slightly overflows its slot, which is what
//! vanilla shows too.
//!
//! Y is negated on the way out because model space is Y-up and the HUD's screen
//! space is Y-down.
//!
//! # The lighting
//!
//! `minecraft_compute_light` / `minecraft_mix_light_separate` from
//! `light.glsl`, evaluated per quad because item quads carry flat per-face
//! normals:
//!
//! ```text
//! accum = min(1, (max(0, n·L0) + max(0, n·L1)) * 0.6 + 0.4)
//! ```
//!
//! The two directions are `DIFFUSE_LIGHT_0 = normalize(0.2, 1, -0.7)` and
//! `DIFFUSE_LIGHT_1 = normalize(-0.2, 1, 0.7)`, each transformed by a pose that
//! differs between flat and 3D items — see [`items_flat_lights`] and
//! [`items_3d_lights`]. Rewo's world and hand passes shade an item by its
//! `Direction` ordinal instead; the GUI genuinely uses this other model, which
//! is why a block looks lit from a different angle in the hotbar than in your
//! hand.

use ash::vk;
use glam::{Mat3, Vec3};

use gpu_allocator::vulkan::Allocation;

use crate::end_sky::{upload_buffer, Buf};
use crate::entities::create_texture;
use crate::held::{DisplayTransform, HeldItemModel, HeldItems};
use crate::Gpu;

/// Pixels per block — a GUI item is 16 px across at scale 1.
pub const PX_PER_BLOCK: f32 = 16.0;
/// `MINECRAFT_LIGHT_POWER`.
const LIGHT_POWER: f32 = 0.6;
/// `MINECRAFT_AMBIENT_LIGHT`.
const AMBIENT: f32 = 0.4;
/// The depth half-range the vertex shader maps into `0..1`.
const DEPTH_SCALE: f32 = 64.0;

/// `Lighting.DIFFUSE_LIGHT_0` / `_1`, before any pose.
fn diffuse_lights() -> (Vec3, Vec3) {
    (
        Vec3::new(0.2, 1.0, -0.7).normalize(),
        Vec3::new(-0.2, 1.0, 0.7).normalize(),
    )
}

/// `Lighting.Entry.ITEMS_FLAT` — `new Matrix4f().rotationY(-π/8).rotateX(3π/4)`
/// applied to both diffuse directions.
///
/// JOML's `rotationY` *sets* the matrix and `rotateX` post-multiplies, so this
/// is `Ry(-π/8) · Rx(3π/4)` and not the other order.
pub fn items_flat_lights() -> (Vec3, Vec3) {
    let pose = Mat3::from_rotation_y(-std::f32::consts::FRAC_PI_8)
        * Mat3::from_rotation_x(std::f32::consts::PI * 3.0 / 4.0);
    let (l0, l1) = diffuse_lights();
    (pose * l0, pose * l1)
}

/// `Lighting.Entry.ITEMS_3D` —
/// `scaling(1, -1, 1).rotateYXZ(1.0821041, 3.2375858, 0).rotateYXZ(-π/8, 3π/4, 0)`.
///
/// `rotateYXZ(y, x, z)` post-multiplies `Ry · Rx · Rz`, and the whole chain is
/// applied in source order, so the scale is leftmost.
pub fn items_3d_lights() -> (Vec3, Vec3) {
    let yxz = |y: f32, x: f32, z: f32| {
        Mat3::from_rotation_y(y) * Mat3::from_rotation_x(x) * Mat3::from_rotation_z(z)
    };
    let pose = Mat3::from_diagonal(Vec3::new(1.0, -1.0, 1.0))
        * yxz(1.082_104_1, 3.237_585_8, 0.0)
        * yxz(
            -std::f32::consts::FRAC_PI_8,
            std::f32::consts::PI * 3.0 / 4.0,
            0.0,
        );
    let (l0, l1) = diffuse_lights();
    (pose * l0, pose * l1)
}

/// Cached so the two poses are built once rather than per quad.
pub struct ItemLights {
    pub flat: (Vec3, Vec3),
    pub three_d: (Vec3, Vec3),
}

impl Default for ItemLights {
    fn default() -> Self {
        Self {
            flat: items_flat_lights(),
            three_d: items_3d_lights(),
        }
    }
}

impl ItemLights {
    /// `minecraft_mix_light_separate`'s scalar, for a face normal.
    pub fn shade(&self, normal: Vec3, from_block: bool) -> f32 {
        let (l0, l1) = if from_block { self.three_d } else { self.flat };
        let a = normal.dot(l0).max(0.0);
        let b = normal.dot(l1).max(0.0);
        ((a + b) * LIGHT_POWER + AMBIENT).min(1.0)
    }
}

/// The outward normal of a vanilla `Direction` ordinal, which is what
/// `HeldQuad::dir` carries.
pub fn direction_normal(dir: u8) -> Vec3 {
    match dir {
        0 => Vec3::new(0.0, -1.0, 0.0), // DOWN
        1 => Vec3::new(0.0, 1.0, 0.0),  // UP
        2 => Vec3::new(0.0, 0.0, -1.0), // NORTH
        3 => Vec3::new(0.0, 0.0, 1.0),  // SOUTH
        4 => Vec3::new(-1.0, 0.0, 0.0), // WEST
        _ => Vec3::new(1.0, 0.0, 0.0),  // EAST
    }
}

// -- the enchantment glint (M43) ----------------------------------------------
//
// `TextureTransform.setupGlintTexturing`, which is the whole animation:
//
// ```java
// long millis = (long)(Util.getMillis() * glintSpeed * 8.0);
// float o0 = (millis % 110000L) / 110000.0F;
// float o1 = (millis %  30000L) /  30000.0F;
// Matrix4f m = new Matrix4f().translation(-o0, o1, 0.0F);
// m.rotateZ((float)(Math.PI / 18)).scale(scale);
// ```
//
// JOML post-multiplies, so the result is `T * Rz * S` and the shader applies
// it as `TextureMat * vec4(UV0, 0, 1)` — a column vector. Read as operations
// on the UV that is **scale, then rotate, then translate**, which is the
// opposite of the order the calls appear in.

/// `Options.glintSpeed`'s default.
pub const GLINT_SPEED: f32 = 0.5;
/// `Options.glintStrength`'s default — the shader's `GlintAlpha`.
pub const GLINT_STRENGTH: f32 = 0.75;
/// `TextureTransform.GLINT_TEXTURING`'s scale — the item glint, which is what
/// a GUI icon and a first-person hand both use.
pub const GLINT_SCALE_ITEM: f32 = 8.0;
/// `ENTITY_GLINT_TEXTURING` — a dropped or mob-held stack. Not used yet;
/// recorded because the three scales are the only difference between the
/// contexts and picking the wrong one is invisible until compared.
pub const GLINT_SCALE_ENTITY: f32 = 0.5;
/// `ARMOR_ENTITY_GLINT_TEXTURING`.
pub const GLINT_SCALE_ARMOR: f32 = 0.16;

/// The two scrolling offsets at a wall-clock time in **milliseconds**.
///
/// The two periods are deliberately coprime-ish (110 s and 30 s) so the
/// pattern does not visibly repeat; the first runs **negative** in u and the
/// second positive in v, which is what makes the sheen travel diagonally.
///
/// `millis` is a `long` before the modulo, so this truncates the same way —
/// doing the remainder in floating point drifts once the session has been up
/// for a few hours.
pub fn glint_offsets(millis_since_start: f64, speed: f32) -> (f32, f32) {
    let millis = (millis_since_start * speed as f64 * 8.0) as i64;
    let o0 = (millis.rem_euclid(110_000)) as f32 / 110_000.0;
    let o1 = (millis.rem_euclid(30_000)) as f32 / 30_000.0;
    (o0, o1)
}

/// Apply the glint texture matrix to one model-space UV.
///
/// The input is the quad's **own** `0..1` texture coordinate, not its place in
/// Rewo's atlas: vanilla feeds the glint pass the same `UV0` the item pass
/// uses, and that is a coordinate in the item's own sprite. Feeding an atlas
/// coordinate instead would make the pattern depend on where the packer
/// happened to put the item.
pub fn glint_uv(uv: [f32; 2], offsets: (f32, f32), scale: f32) -> [f32; 2] {
    // Scale, then rotate by 10 degrees, then translate — see the note above on
    // JOML's post-multiplication.
    let (s, c) = (std::f32::consts::PI / 18.0).sin_cos();
    let (x, y) = (uv[0] * scale, uv[1] * scale);
    [
        c * x - s * y - offsets.0,
        s * x + c * y + offsets.1,
    ]
}

/// One item to draw, at a slot's top-left corner in screen pixels.
#[derive(Clone, Debug)]
pub struct GuiItem {
    /// Item model name, e.g. `minecraft:diamond_sword`.
    pub model: String,
    /// The slot's top-left, in screen pixels.
    pub x: f32,
    pub y: f32,
    /// Slot size in pixels — 16 for a vanilla hotbar at GUI scale 1, larger
    /// when the HUD is scaled up.
    pub size: f32,
    /// Whether this stack draws an enchantment glint (M43).
    ///
    /// `ItemStack.hasFoil()`, which for a plain item is
    /// `!getEnchantments().isEmpty()` unless `ENCHANTMENT_GLINT_OVERRIDE` says
    /// otherwise. Carried per-item rather than looked up here because the pass
    /// has no idea what a stack is — it draws rectangles.
    pub glint: bool,
}

struct GlintParts {
    pipeline: vk::Pipeline,
    sampler: vk::Sampler,
    image: vk::Image,
    image_alloc: Option<Allocation>,
    view: vk::ImageView,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GuiItemVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub shade: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Push {
    screen: [f32; 2],
    depth_scale: f32,
    _pad: f32,
}

/// Apply `display.gui` to a model-space point and land it in screen pixels.
///
/// `p` is in `0..16` model units. The rotation is XYZ in degrees, matching
/// `DisplayTransform` and vanilla's `ItemTransform`, whose translation has
/// already been through the deserializer's `×0.0625` and clamp.
pub fn place(t: &DisplayTransform, p: [f32; 3], slot_centre: [f32; 2], px_per_block: f32) -> [f32; 3] {
    // Model units → block space, centred on the slot.
    let c = Vec3::new(p[0] / 16.0 - 0.5, p[1] / 16.0 - 0.5, p[2] / 16.0 - 0.5);
    let scaled = Vec3::new(c.x * t.scale[0], c.y * t.scale[1], c.z * t.scale[2]);
    let r = Mat3::from_rotation_z(t.rotation[2].to_radians())
        * Mat3::from_rotation_y(t.rotation[1].to_radians())
        * Mat3::from_rotation_x(t.rotation[0].to_radians());
    let v = r * scaled + Vec3::from_array(t.translation);
    [
        slot_centre[0] + v.x * px_per_block,
        // Model space is Y-up; the HUD's screen space is Y-down.
        slot_centre[1] - v.y * px_per_block,
        v.z * px_per_block,
    ]
}

/// Build the vertices for one frame's slots.
///
/// An item whose model is not baked contributes nothing — the same
/// "draw nothing rather than something wrong" rule the rest of the item path
/// uses for the 147 state-dependent definitions M22 suppresses.
/// The glint quads for a set of items, in the same screen positions the icons
/// occupy (M43).
///
/// Vanilla draws the glint as a **second pass over the same geometry** — the
/// item's own quads, re-fed to a pipeline that samples the glint sheet through
/// a scrolling matrix. That is why this takes the same inputs as
/// [`build_vertices`] and differs only in what it puts in the UV slot: the
/// positions have to match exactly, or the depth-equal test the glint pass
/// relies on would reject every fragment.
pub fn build_glint_vertices(
    items: &HeldItems,
    slots: &[GuiItem],
    millis: f64,
) -> Vec<GuiItemVertex> {
    let offsets = glint_offsets(millis, GLINT_SPEED);
    let mut out = Vec::new();
    for slot in slots {
        if !slot.glint {
            continue;
        }
        let Some(model): Option<&HeldItemModel> = items.any(&slot.model) else {
            continue;
        };
        let centre = [slot.x + slot.size / 2.0, slot.y + slot.size / 2.0];
        let px = slot.size / 16.0 * PX_PER_BLOCK;
        for q in model.quads_for_gui() {
            let p: Vec<[f32; 3]> = q
                .verts
                .iter()
                .map(|v| place(&model.gui, *v, centre, px))
                .collect();
            // The quad's **own** 0..1 UV, not its place in the atlas — see
            // [`glint_uv`].
            let uvs: Vec<[f32; 2]> = q
                .uv
                .iter()
                .map(|t| glint_uv(*t, offsets, GLINT_SCALE_ITEM))
                .collect();
            let v = |i: usize| GuiItemVertex {
                pos: p[i],
                uv: uvs[i],
                // The item pass's diffuse slot carries `GlintAlpha` here.
                shade: GLINT_STRENGTH,
            };
            out.extend_from_slice(&[v(0), v(1), v(2), v(0), v(2), v(3)]);
        }
    }
    out
}

pub fn build_vertices(
    items: &HeldItems,
    slots: &[GuiItem],
    lights: &ItemLights,
    atlas: &dyn Fn(u16) -> Option<[f32; 4]>,
) -> Vec<GuiItemVertex> {
    let mut out = Vec::new();
    for slot in slots {
        let Some(model): Option<&HeldItemModel> = items.any(&slot.model) else {
            continue;
        };
        let centre = [slot.x + slot.size / 2.0, slot.y + slot.size / 2.0];
        let px = slot.size / 16.0 * PX_PER_BLOCK;
        // A slot draws `gui_quads` when the item's definition selects
        // different geometry there — a spear is a flat sprite in a slot and a
        // 3D model in the hand (M40).
        for q in model.quads_for_gui() {
            let Some(uv) = atlas(q.tex) else {
                continue; // texture not resident — nothing, never garbage
            };
            let shade = lights.shade(direction_normal(q.dir), model.from_block);
            let p: Vec<[f32; 3]> = q
                .verts
                .iter()
                .map(|v| place(&model.gui, *v, centre, px))
                .collect();
            let uvs: Vec<[f32; 2]> = q
                .uv
                .iter()
                .map(|t| [uv[0] + t[0] * uv[2], uv[1] + t[1] * uv[3]])
                .collect();
            let v = |i: usize| GuiItemVertex {
                pos: p[i],
                uv: uvs[i],
                shade,
            };
            out.extend_from_slice(&[v(0), v(1), v(2), v(0), v(2), v(3)]);
        }
    }
    out
}

pub struct GuiItemPass {
    /// The glint's own pipeline, sampler, image and descriptor set (M43).
    ///
    /// It shares the vertex shader and the pipeline layout — the geometry and
    /// the push constants are identical — and differs in three things that all
    /// have to change together: a fragment shader that ignores the item atlas,
    /// a `SRC_COLOR / ONE` blend, and a depth test of **EQUAL with no write**,
    /// which is what lands the sheen exactly on the item's own fragments and
    /// nowhere else.
    glint: Option<GlintParts>,
    glint_verts: u32,
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    atlas: (vk::Image, vk::ImageView),
    allocs: Vec<gpu_allocator::vulkan::Allocation>,
    vbuf: Option<Buf>,
    vert_count: u32,
}

impl GuiItemPass {
    /// `atlas` is an RGBA8 image the item UVs index into. The caller owns its
    /// packing; this pass only samples it.
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        atlas_rgba: &[u8],
        atlas_w: u32,
        atlas_h: u32,
    ) -> Result<Self, String> {
        let (img, alloc, view) = crate::entities::create_texture(gpu, atlas_rgba, atlas_w, atlas_h)?;
        let device = gpu.device.clone();
        // NEAREST: an item icon is pixel art shown near 1:1, and linear
        // filtering would blur the 16×16 sprites the whole look depends on.
        let sampler = unsafe {
            device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                    None,
                )
                .map_err(|e| format!("gui-item sampler: {e}"))?
        };
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let set_layout = unsafe {
            device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| format!("gui-item set layout: {e}"))?
        };
        let sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)];
        let pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(|e| format!("gui-item pool: {e}"))?
        };
        let set_layouts = [set_layout];
        let set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("gui-item set: {e}"))?[0]
        };
        let info = [vk::DescriptorImageInfo::default()
            .sampler(sampler)
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        unsafe {
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&info)],
                &[],
            );
        }
        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(std::mem::size_of::<Push>() as u32)];
        let layout = unsafe {
            device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&push),
                    None,
                )
                .map_err(|e| format!("gui-item layout: {e}"))?
        };
        let pipeline = build_pipeline(&device, layout, color_format)?;
        Ok(Self {
            pipeline,
            layout,
            set_layout,
            pool,
            set,
            sampler,
            atlas: (img, view),
            allocs: vec![alloc],
            vbuf: None,
            vert_count: 0,
            glint: None,
            glint_verts: 0,
        })
    }

    pub fn set_vertices(&mut self, gpu: &mut Gpu, verts: &[GuiItemVertex]) -> Result<(), String> {
        free_buf(gpu, self.vbuf.take());
        self.vert_count = verts.len() as u32;
        if verts.is_empty() {
            return Ok(());
        }
        self.vbuf = Some(upload_buffer(
            gpu,
            bytemuck::cast_slice(verts),
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?);
        Ok(())
    }

    /// Build the glint's pipeline and upload its sheet (M43).
    ///
    /// Separate from [`Self::new`] because the glint is optional: a jar
    /// without `misc/enchanted_glint_item.png` draws no shimmer rather than
    /// failing to start, the same rule every other optional texture follows.
    pub fn init_glint(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
        color_format: vk::Format,
    ) -> Result<(), String> {
        if self.glint.is_some() {
            return Ok(());
        }
        let device = gpu.device.clone();
        let (image, image_alloc, view) = create_texture(gpu, rgba, w, h)?;
        // **REPEAT and LINEAR**, both load-bearing. The scrolling matrix
        // scales the UV by 8, so the sheet is sampled far outside `0..1` and
        // clamping would smear one edge texel across the whole item; and the
        // texture's own `.mcmeta` sets `blur: true`, which is the one place
        // Minecraft asks for a filtered GUI texture.
        let sampler = unsafe {
            device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::LINEAR)
                        .min_filter(vk::Filter::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::REPEAT)
                        .address_mode_v(vk::SamplerAddressMode::REPEAT)
                        .address_mode_w(vk::SamplerAddressMode::REPEAT),
                    None,
                )
                .map_err(|e| format!("glint sampler: {e}"))?
        };
        let sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)];
        let pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(|e| format!("glint pool: {e}"))?
        };
        let set_layouts = [self.set_layout];
        let set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("glint set: {e}"))?[0]
        };
        let info = [vk::DescriptorImageInfo::default()
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image_view(view)
            .sampler(sampler)];
        unsafe {
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&info)],
                &[],
            );
        }
        let pipeline = build_glint_pipeline(gpu, self.layout, color_format)?;
        self.glint = Some(GlintParts {
            pipeline,
            sampler,
            image,
            image_alloc: Some(image_alloc),
            view,
            pool,
            set,
        });
        Ok(())
    }

    pub fn glint_ready(&self) -> bool {
        self.glint.is_some()
    }

    /// This frame's glint geometry. Appended to the same buffer after the item
    /// vertices, so one upload serves both draws.
    pub fn set_vertices_with_glint(
        &mut self,
        gpu: &mut Gpu,
        verts: &[GuiItemVertex],
        glint: &[GuiItemVertex],
    ) -> Result<(), String> {
        let mut all = Vec::with_capacity(verts.len() + glint.len());
        all.extend_from_slice(verts);
        all.extend_from_slice(glint);
        self.set_vertices(gpu, &all)?;
        self.vert_count = verts.len() as u32;
        self.glint_verts = glint.len() as u32;
        Ok(())
    }

    /// The glint, over the icons it belongs to.
    pub fn draw_glint(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D) {
        let (Some(parts), Some(vbuf)) = (self.glint.as_ref(), self.vbuf.as_ref()) else {
            return;
        };
        if self.glint_verts == 0 {
            return;
        }
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, parts.pipeline);
            let viewport = vk::Viewport::default()
                .width(extent.width as f32)
                .height(extent.height as f32)
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[parts.set],
                &[],
            );
            let push = Push {
                screen: [extent.width as f32, extent.height as f32],
                depth_scale: DEPTH_SCALE,
                _pad: 0.0,
            };
            device.cmd_push_constants(
                cb,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytemuck::bytes_of(&push),
            );
            device.cmd_bind_vertex_buffers(cb, 0, &[vbuf.buffer], &[0]);
            device.cmd_draw(cb, self.glint_verts, 1, self.vert_count, 0);
        }
    }

    pub fn draw(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D) {
        let Some(vbuf) = self.vbuf.as_ref() else {
            return;
        };
        if self.vert_count == 0 {
            return;
        }
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            // Positive viewport — top-left screen coords, matching the HUD.
            let viewport = vk::Viewport::default()
                .width(extent.width as f32)
                .height(extent.height as f32)
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
            let push = Push {
                screen: [extent.width as f32, extent.height as f32],
                depth_scale: DEPTH_SCALE,
                _pad: 0.0,
            };
            device.cmd_push_constants(
                cb,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytemuck::bytes_of(&push),
            );
            device.cmd_bind_vertex_buffers(cb, 0, &[vbuf.buffer], &[0]);
            device.cmd_draw(cb, self.vert_count, 1, 0, 0);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        free_buf(gpu, self.vbuf.take());
        let device = gpu.device.clone();
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.atlas.1, None);
            device.destroy_image(self.atlas.0, None);
            if let Some(mut g) = self.glint.take() {
                device.destroy_pipeline(g.pipeline, None);
                device.destroy_descriptor_pool(g.pool, None);
                device.destroy_sampler(g.sampler, None);
                device.destroy_image_view(g.view, None);
                device.destroy_image(g.image, None);
                if let Some(a) = g.image_alloc.take() {
                    self.allocs.push(a);
                }
            }
        }
        for a in self.allocs.drain(..) {
            let _ = gpu.allocator.free(a);
        }
    }
}

fn free_buf(gpu: &mut Gpu, buf: Option<Buf>) {
    if let Some(mut b) = buf {
        unsafe { gpu.device.destroy_buffer(b.buffer, None) };
        if let Some(a) = b.alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

/// The glint's pipeline (M43) — the item pass's vertex shader with a different
/// fragment shader, blend and depth rule.
///
/// Three differences from [`build_pipeline`], all of them from
/// `RenderPipelines.GLINT`:
///
/// - `BlendFunction.GLINT` is `(SRC_COLOR, ONE, ZERO, ONE)`. The colour source
///   factor is the **source colour itself**, so a dark glint texel contributes
///   nothing and a bright one blooms; and it only ever adds. Alpha takes the
///   destination's, leaving the frame's alpha alone — which matters because
///   the headless gates read it back.
/// - `DepthStencilState(CompareOp.EQUAL, false)`: test equal, **do not
///   write**. This is what puts the sheen exactly on the item's own fragments
///   and nowhere else — a `LESS`/`GREATER` test would paint it over faces the
///   item itself had hidden.
/// - Culling off, matching the item pass, because a rotated block shows faces
///   of both windings.
fn build_glint_pipeline(
    gpu: &Gpu,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    let device = &gpu.device;
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/gui_item.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/gui_glint.frag.spv")),
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
            .stride(std::mem::size_of::<GuiItemVertex>() as u32)
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
                .format(vk::Format::R32_SFLOAT)
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
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::EQUAL);
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_COLOR)
            .dst_color_blend_factor(vk::BlendFactor::ONE)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ZERO)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B
                    | vk::ColorComponentFlags::A,
            )];
        let blend_state =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [color_format];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&color_formats)
            .depth_attachment_format(crate::world::DEPTH_FORMAT);
        let ci = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend_state)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), &[ci], None)
            .map_err(|(_, e)| format!("glint pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/gui_item.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/gui_item.frag.spv")),
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
            .stride(std::mem::size_of::<GuiItemVertex>() as u32)
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
                .format(vk::Format::R32_SFLOAT)
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
        // A rotated block shows faces of both windings, and the sprite
        // extrusion's side quads are not consistently wound either.
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // Depth against this pass's own buffer only, so a block item's faces
        // sort against each other. Reversed-Z, matching the world pass.
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::GREATER);
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B
                    | vk::ColorComponentFlags::A,
            )];
        let blend_state =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [color_format];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&color_formats)
            .depth_attachment_format(crate::world::DEPTH_FORMAT);
        let ci = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend_state)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), &[ci], None)
            .map_err(|(_, e)| format!("gui-item pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sprite's identity `gui` maps `0..16` model units onto exactly the
    /// slot — this is what makes the absent transform *right* rather than
    /// missing.
    #[test]
    fn a_sprite_fills_its_slot_exactly() {
        let t = DisplayTransform::default();
        let centre = [108.0, 200.0];
        let a = place(&t, [0.0, 0.0, 0.0], centre, PX_PER_BLOCK);
        let b = place(&t, [16.0, 16.0, 0.0], centre, PX_PER_BLOCK);
        assert_eq!(a[0], centre[0] - 8.0);
        assert_eq!(b[0], centre[0] + 8.0);
        // Y is flipped: model +Y is screen -Y.
        assert_eq!(a[1], centre[1] + 8.0);
        assert_eq!(b[1], centre[1] - 8.0);
    }

    /// A block's transform tilts it and shrinks it to 0.625, so it reaches
    /// further than the slot on the diagonal — as vanilla's does.
    #[test]
    fn a_block_tilts_and_slightly_overflows() {
        let t = DisplayTransform {
            rotation: [30.0, 225.0, 0.0],
            translation: [0.0; 3],
            scale: [0.625; 3],
        };
        let centre = [0.0, 0.0];
        let mut max = 0.0f32;
        for &x in &[0.0f32, 16.0] {
            for &y in &[0.0f32, 16.0] {
                for &z in &[0.0f32, 16.0] {
                    let p = place(&t, [x, y, z], centre, PX_PER_BLOCK);
                    max = max.max(p[0].abs()).max(p[1].abs());
                }
            }
        }
        assert!(
            max > 8.0 && max < 10.0,
            "a block reaches just past the 8 px half-slot: {max}"
        );
    }

    /// The scale actually shrinks: a block's corners are nearer the centre than
    /// a sprite's edges in the axis-aligned sense.
    #[test]
    fn the_gui_scale_is_applied() {
        let ident = DisplayTransform::default();
        let scaled = DisplayTransform {
            scale: [0.5; 3],
            ..Default::default()
        };
        let a = place(&ident, [16.0, 8.0, 8.0], [0.0, 0.0], PX_PER_BLOCK);
        let b = place(&scaled, [16.0, 8.0, 8.0], [0.0, 0.0], PX_PER_BLOCK);
        assert_eq!(a[0], 8.0);
        assert_eq!(b[0], 4.0);
    }

    /// The two lighting poses are genuinely different, and both stay inside the
    /// formula's range. A shared pose would light a block in the hotbar exactly
    /// as a flat sprite, which is not what vanilla shows.
    #[test]
    fn the_flat_and_3d_light_poses_differ() {
        let l = ItemLights::default();
        assert!(
            (l.flat.0 - l.three_d.0).length() > 0.1,
            "flat {:?} vs 3d {:?}",
            l.flat.0,
            l.three_d.0
        );
        // Both directions stay unit length — the poses are rotations (and one
        // reflection), so they must not scale.
        for v in [l.flat.0, l.flat.1, l.three_d.0, l.three_d.1] {
            assert!((v.length() - 1.0).abs() < 1e-5, "{v:?}");
        }
    }

    /// `min(1, (a+b) * 0.6 + 0.4)` — the floor is the ambient and the ceiling
    /// is one, so no face is ever black and none blows out.
    #[test]
    fn the_diffuse_is_bounded_by_ambient_and_one() {
        let l = ItemLights::default();
        for dir in 0..6u8 {
            for from_block in [false, true] {
                let s = l.shade(direction_normal(dir), from_block);
                assert!(
                    (AMBIENT..=1.0).contains(&s),
                    "dir {dir} block {from_block} -> {s}"
                );
            }
        }
    }

    /// A face lit by both lights is brighter than one lit by neither.
    #[test]
    fn facing_the_lights_is_brighter_than_facing_away() {
        let l = ItemLights::default();
        let (a, b) = l.flat;
        let toward = (a + b).normalize();
        assert!(l.shade(toward, false) > l.shade(-toward, false));
        assert_eq!(l.shade(-toward, false), AMBIENT, "fully away is ambient");
    }
}
