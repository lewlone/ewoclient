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

use std::collections::HashMap;

use ash::vk;
use bytemuck::{Pod, Zeroable};
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::entities::{EntityDraw, EntityPass, FontData, MobTextures};
use crate::hud::{HudPass, HudSpritesData};
use crate::text::{TextLine, TextPass};
use crate::Gpu;

/// An owned screen-space text line (the app fills these each frame; the
/// renderer borrows them into `TextLine` at draw time).
pub struct OwnedTextLine {
    pub x: f32,
    pub y: f32,
    pub px: f32,
    pub color: [f32; 3],
    pub alpha: f32,
    pub text: String,
}

pub const DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT;
const VERTEX_STRIDE: u64 = 36;
/// Mega-buffer capacities. 4M verts (144 MB) + 6M indices (24 MB) covers a
/// large render distance; over-cap columns are dropped with a log.
const MAX_VERTS: u64 = 4_000_000;
const MAX_INDICES: u64 = 6_000_000;
const MAX_COLUMNS: usize = 8192;

/// One animated texture-array layer (mirror of rewo-data's
/// `AnimatedLayer` — this crate stays free of a rewo-data dependency).
pub struct LayerAnimation {
    pub layer: u16,
    /// 16×16 RGBA frames (pre-tinted).
    pub frames: Vec<Vec<u8>>,
    pub order: Vec<u32>,
    pub frametime: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct WorldPush {
    view_proj: [[f32; 4]; 4],
    /// xyz camera world pos, w = fog start distance.
    cam_fog: [f32; 4],
    /// xyz fog color (linear = sky horizon), w = fog end distance.
    fog_col: [f32; 4],
    /// x = sky-light factor (the time of day), y = block-light factor.
    /// Vanilla's lightmap scales the sky half of the light by the time of
    /// day; keeping it a uniform is what makes a sunrise cost one push
    /// instead of remeshing the world.
    light: [f32; 4],
    /// xyz sky-light colour — white by day, blue at night.
    sky_col: [f32; 4],
}
// 128 bytes: exactly the push-constant size Vulkan guarantees. Anything
// further has to move into a uniform buffer.
const _: () = assert!(std::mem::size_of::<WorldPush>() == 128);

#[repr(C)]
#[derive(Clone, Copy)]
struct SkyPush {
    inv_view_proj: [[f32; 4]; 4],
    horizon: [f32; 4],
    zenith: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinePush {
    view_proj: [[f32; 4]; 4],
    color: [f32; 4],
}

const SELECT_RING: usize = 2;

/// Sky gradient colors (linear; sRGB pale-blue horizon / deeper zenith).
/// The horizon is also the terrain's fog color for a seamless far edge.
const SKY_HORIZON: [f32; 3] = [0.477, 0.638, 0.890];

/// Vanilla's `blockFactor = blockLightFlicker + 1.4` (the flicker is a small
/// per-frame jitter we do not reproduce, so this is its resting value).
pub const BLOCK_LIGHT_FACTOR: f32 = 1.4;
const SKY_ZENITH: [f32; 3] = [0.147, 0.319, 0.890];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct ColumnMeta {
    aabb_min: [f32; 4],
    aabb_max: [f32; 4],
    index_count: u32,
    first_index: u32,
    vertex_offset: i32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CullPush {
    planes: [[f32; 4]; 6],
    column_count: u32,
    _pad: [u32; 3],
}

struct Slot {
    vtx_off: u64,   // in vertices
    vtx_len: u64,
    idx_off: u64,   // in indices
    idx_len: u64,
    // Translucent (water) region — zero-length when the column has none.
    tvtx_off: u64,
    tvtx_len: u64,
    tidx_off: u64,
    tidx_len: u64,
    meta: ColumnMeta,
}

/// One in-flight upload: its own command buffer, fence, and staging
/// buffer (untouched until the fence says the copy retired).
struct UploadSlot {
    cb: vk::CommandBuffer,
    fence: vk::Fence,
    staging: vk::Buffer,
    staging_alloc: Option<Allocation>,
    staging_cap: u64,
    busy: bool,
}

const UPLOAD_SLOTS: usize = 4;

/// First-fit free-list over a fixed-capacity element range, with coalescing.
struct FreeList {
    free: Vec<(u64, u64)>, // (offset, len), sorted by offset
}

impl FreeList {
    fn new(capacity: u64) -> Self {
        Self {
            free: vec![(0, capacity)],
        }
    }

    fn alloc(&mut self, n: u64) -> Option<u64> {
        if n == 0 {
            return Some(0);
        }
        for i in 0..self.free.len() {
            let (off, len) = self.free[i];
            if len >= n {
                if len == n {
                    self.free.remove(i);
                } else {
                    self.free[i] = (off + n, len - n);
                }
                return Some(off);
            }
        }
        None
    }

    fn free_region(&mut self, off: u64, n: u64) {
        if n == 0 {
            return;
        }
        // Insert sorted, then coalesce neighbors.
        let pos = self.free.partition_point(|&(o, _)| o < off);
        self.free.insert(pos, (off, n));
        let mut i = 0;
        while i + 1 < self.free.len() {
            let (o1, l1) = self.free[i];
            let (o2, l2) = self.free[i + 1];
            if o1 + l1 == o2 {
                self.free[i] = (o1, l1 + l2);
                self.free.remove(i + 1);
            } else {
                i += 1;
            }
        }
    }
}

pub struct WorldRenderer {
    // -- graphics --
    tex_set_layout: vk::DescriptorSetLayout,
    graphics_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    /// Blended, depth-write-off variant for the water pass.
    water_pipeline: vk::Pipeline,
    /// Fullscreen gradient sky (no descriptor set, no vertex input).
    sky_layout: vk::PipelineLayout,
    sky_pipeline: vk::Pipeline,
    /// Block-selection outline (LINE_LIST, depth-tested against terrain).
    select_layout: vk::PipelineLayout,
    select_pipeline: vk::Pipeline,
    select_bufs: [vk::Buffer; SELECT_RING],
    select_allocs: [Option<Allocation>; SELECT_RING],
    select_cursor: usize,
    /// The targeted block (world coords), or `None` — set each frame.
    selection: Option<[i32; 3]>,
    tex_pool: vk::DescriptorPool,
    tex_set: vk::DescriptorSet,
    sampler: vk::Sampler,
    image: vk::Image,
    image_alloc: Option<Allocation>,
    view: vk::ImageView,
    // -- mega-buffers --
    vbuf: vk::Buffer,
    vbuf_alloc: Option<Allocation>,
    vfree: FreeList,
    ibuf: vk::Buffer,
    ibuf_alloc: Option<Allocation>,
    ifree: FreeList,
    // -- columns + metadata --
    columns: HashMap<(i32, i32), Slot>,
    meta_dirty: bool,
    meta_buf: vk::Buffer,
    meta_alloc: Option<Allocation>,
    column_count: u32,
    // -- compute cull --
    cull_set_layout: vk::DescriptorSetLayout,
    cull_layout: vk::PipelineLayout,
    cull_pipeline: vk::Pipeline,
    cull_pool: vk::DescriptorPool,
    cull_set: vk::DescriptorSet,
    indirect_buf: vk::Buffer,
    indirect_alloc: Option<Allocation>,
    count_buf: vk::Buffer,
    count_alloc: Option<Allocation>,
    // -- async upload ring --
    // Uploads submit on the graphics queue WITHOUT a CPU fence wait: queue
    // FIFO ordering guarantees every copy executes before any later-
    // submitted frame that could read it (the same guarantee the old
    // blocking path relied on — minus the stall). The CPU only waits when
    // all slots are still in flight (sustained burst) or to grow staging.
    upload_pool: vk::CommandPool,
    upload_slots: Vec<UploadSlot>,
    count_readback: vk::Buffer,
    count_readback_alloc: Option<Allocation>,
    // -- entities + HUD (optional passes; drawn after terrain in `draw`) --
    color_format: vk::Format,
    entities: Option<EntityPass>,
    hud: Option<HudPass>,
    /// Live HUD state (health 0..20, food 0..20, selected slot 0..8); when
    /// `None`, no HUD draws (view/demo/bench aren't "playing").
    hud_state: Option<(f32, i32, u8)>,
    text: Option<TextPass>,
    /// Screen-space text lines to draw this frame (chat, coords); empty →
    /// nothing (view/demo/bench never set them).
    text_lines: Vec<OwnedTextLine>,
    /// Eye position for translucent sort + fog origin (`set_camera`).
    camera_eye: [f32; 3],
    /// Time-of-day scale on sky light (`SKY_LIGHT_FACTOR`): 1.0 at midday,
    /// 0.24 at midnight. Set by `set_lightmap`.
    sky_factor: f32,
    /// Sky-light colour — white by day, blue at night.
    sky_color: [f32; 3],
    /// Distance-fog band [start, end] (env `REWO_FOG=start,end`).
    fog: [f32; 2],
    // -- animated texture layers (water/lava; `anim_tick`) --
    tex_size: u32,
    mip_levels: u32,
    animations: Vec<LayerAnimation>,
    anim_last: Vec<Option<usize>>,

    pub drawn_last_frame: usize,
    pub culled_last_frame: usize,
}

impl WorldRenderer {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        tex_size: u32,
        layers: &[Vec<u8>],
    ) -> Result<Self, String> {
        unsafe {
            let layer_count = layers.len().max(1) as u32;
            let mip_levels = 32 - tex_size.leading_zeros();

            // ---- texture array ----
            let image = gpu
                .device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_SRGB)
                        .extent(vk::Extent3D {
                            width: tex_size,
                            height: tex_size,
                            depth: 1,
                        })
                        .mip_levels(mip_levels)
                        .array_layers(layer_count)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .map_err(|e| format!("texture array: {e}"))?;
            let req = gpu.device.get_image_memory_requirements(image);
            let image_alloc = gpu
                .allocator
                .allocate(&AllocationCreateDesc {
                    name: "block-texture-array",
                    requirements: req,
                    location: MemoryLocation::GpuOnly,
                    linear: false,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .map_err(|e| format!("texture alloc: {e}"))?;
            gpu.device
                .bind_image_memory(image, image_alloc.memory(), image_alloc.offset())
                .map_err(|e| format!("texture bind: {e}"))?;
            upload_texture_array(gpu, image, tex_size, mip_levels, layers)?;

            let device = gpu.device.clone();
            let view = device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
                        .format(vk::Format::R8G8B8A8_SRGB)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .base_mip_level(0)
                                .level_count(mip_levels)
                                .base_array_layer(0)
                                .layer_count(layer_count),
                        ),
                    None,
                )
                .map_err(|e| format!("texture view: {e}"))?;
            let sampler = device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::REPEAT)
                        .address_mode_v(vk::SamplerAddressMode::REPEAT)
                        .address_mode_w(vk::SamplerAddressMode::REPEAT)
                        .max_lod(mip_levels as f32),
                    None,
                )
                .map_err(|e| format!("sampler: {e}"))?;

            // ---- graphics descriptor + pipeline ----
            let tex_bindings = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let tex_set_layout = device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&tex_bindings),
                    None,
                )
                .map_err(|e| format!("tex set layout: {e}"))?;
            let tex_pool_sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)];
            let tex_pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&tex_pool_sizes),
                    None,
                )
                .map_err(|e| format!("tex pool: {e}"))?;
            let tex_layouts = [tex_set_layout];
            let tex_set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(tex_pool)
                        .set_layouts(&tex_layouts),
                )
                .map_err(|e| format!("tex set: {e}"))?[0];
            let image_info = [vk::DescriptorImageInfo::default()
                .sampler(sampler)
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(tex_set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&image_info)],
                &[],
            );

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

            // ---- sky pipeline (fullscreen, no sets / vertex input) ----
            let sky_pc = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .offset(0)
                .size(std::mem::size_of::<SkyPush>() as u32)];
            let sky_layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&sky_pc),
                    None,
                )
                .map_err(|e| format!("sky layout: {e}"))?;
            let sky_pipeline = build_sky_pipeline(&device, sky_layout, color_format)?;

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
            let mut select_bufs = [vk::Buffer::null(); SELECT_RING];
            let mut select_allocs: [Option<Allocation>; SELECT_RING] = [None, None];
            for i in 0..SELECT_RING {
                let (b, a) = create_host_buffer(
                    gpu,
                    24 * 12, // 24 line verts × vec3
                    vk::BufferUsageFlags::VERTEX_BUFFER,
                    "selection-lines",
                )?;
                select_bufs[i] = b;
                select_allocs[i] = Some(a);
            }

            // ---- compute cull descriptor + pipeline ----
            let cull_bindings = [
                storage_binding(0),
                storage_binding(1),
                storage_binding(2),
            ];
            let cull_set_layout = device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&cull_bindings),
                    None,
                )
                .map_err(|e| format!("cull set layout: {e}"))?;
            let cull_pc = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
                .offset(0)
                .size(std::mem::size_of::<CullPush>() as u32)];
            let cull_layouts = [cull_set_layout];
            let cull_layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&cull_layouts)
                        .push_constant_ranges(&cull_pc),
                    None,
                )
                .map_err(|e| format!("cull layout: {e}"))?;
            let cull_module = crate::overlay::create_shader(
                &device,
                include_bytes!(concat!(env!("OUT_DIR"), "/cull.comp.spv")),
            )?;
            let cull_pipeline = device
                .create_compute_pipelines(
                    vk::PipelineCache::null(),
                    &[vk::ComputePipelineCreateInfo::default()
                        .stage(
                            vk::PipelineShaderStageCreateInfo::default()
                                .stage(vk::ShaderStageFlags::COMPUTE)
                                .module(cull_module)
                                .name(c"main"),
                        )
                        .layout(cull_layout)],
                    None,
                )
                .map_err(|(_, e)| format!("cull pipeline: {e}"))?[0];
            device.destroy_shader_module(cull_module, None);

            // ---- mega-buffers + metadata + indirect + count ----
            let (vbuf, vbuf_alloc) = create_device_buffer(
                gpu,
                MAX_VERTS * VERTEX_STRIDE,
                vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
                "mega-vertices",
            )?;
            let (ibuf, ibuf_alloc) = create_device_buffer(
                gpu,
                MAX_INDICES * 4,
                vk::BufferUsageFlags::INDEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
                "mega-indices",
            )?;
            let (meta_buf, meta_alloc) = create_host_buffer(
                gpu,
                (MAX_COLUMNS * std::mem::size_of::<ColumnMeta>()) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                "column-meta",
            )?;
            let (indirect_buf, indirect_alloc) = create_device_buffer(
                gpu,
                (MAX_COLUMNS * std::mem::size_of::<vk::DrawIndexedIndirectCommand>()) as u64,
                vk::BufferUsageFlags::INDIRECT_BUFFER | vk::BufferUsageFlags::STORAGE_BUFFER,
                "indirect-cmds",
            )?;
            let (count_buf, count_alloc) = create_device_buffer(
                gpu,
                16,
                vk::BufferUsageFlags::INDIRECT_BUFFER
                    | vk::BufferUsageFlags::STORAGE_BUFFER
                    | vk::BufferUsageFlags::TRANSFER_DST
                    | vk::BufferUsageFlags::TRANSFER_SRC, // readback for stats
                "indirect-count",
            )?;

            // Cull descriptor set (meta, indirect cmds, count).
            let cull_pool_sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(3)];
            let cull_pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&cull_pool_sizes),
                    None,
                )
                .map_err(|e| format!("cull pool: {e}"))?;
            let cull_set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(cull_pool)
                        .set_layouts(&cull_layouts),
                )
                .map_err(|e| format!("cull set: {e}"))?[0];
            write_storage(&device, cull_set, 0, meta_buf);
            write_storage(&device, cull_set, 1, indirect_buf);
            write_storage(&device, cull_set, 2, count_buf);

            // ---- async upload ring ----
            // RESET_COMMAND_BUFFER so each slot's cb re-begins on its own
            // while its siblings stay in flight.
            let upload_pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(gpu.graphics_family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .map_err(|e| format!("upload pool: {e}"))?;
            let slot_cbs = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(upload_pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(UPLOAD_SLOTS as u32),
                )
                .map_err(|e| format!("upload cbs: {e}"))?;
            let mut upload_slots = Vec::with_capacity(UPLOAD_SLOTS);
            for cb in slot_cbs {
                let fence = device
                    .create_fence(&vk::FenceCreateInfo::default(), None)
                    .map_err(|e| format!("upload fence: {e}"))?;
                let staging_cap = 2 * 1024 * 1024; // grows on demand
                let (staging, staging_alloc) = create_host_buffer(
                    gpu,
                    staging_cap,
                    vk::BufferUsageFlags::TRANSFER_SRC,
                    "upload-staging",
                )?;
                upload_slots.push(UploadSlot {
                    cb,
                    fence,
                    staging,
                    staging_alloc: Some(staging_alloc),
                    staging_cap,
                    busy: false,
                });
            }
            let (count_readback, count_readback_alloc) = create_host_buffer(
                gpu,
                16,
                vk::BufferUsageFlags::TRANSFER_DST,
                "count-readback",
            )?;

            Ok(Self {
                tex_set_layout,
                graphics_layout,
                pipeline,
                water_pipeline,
                sky_layout,
                sky_pipeline,
                select_layout,
                select_pipeline,
                select_bufs,
                select_allocs,
                select_cursor: 0,
                selection: None,
                tex_pool,
                tex_set,
                sampler,
                image,
                image_alloc: Some(image_alloc),
                view,
                vbuf,
                vbuf_alloc: Some(vbuf_alloc),
                vfree: FreeList::new(MAX_VERTS),
                ibuf,
                ibuf_alloc: Some(ibuf_alloc),
                ifree: FreeList::new(MAX_INDICES),
                columns: HashMap::new(),
                meta_dirty: false,
                meta_buf,
                meta_alloc: Some(meta_alloc),
                column_count: 0,
                cull_set_layout,
                cull_layout,
                cull_pipeline,
                cull_pool,
                cull_set,
                indirect_buf,
                indirect_alloc: Some(indirect_alloc),
                count_buf,
                count_alloc: Some(count_alloc),
                upload_pool,
                upload_slots,
                count_readback,
                count_readback_alloc: Some(count_readback_alloc),
                color_format,
                entities: None,
                hud: None,
                hud_state: None,
                text: None,
                text_lines: Vec::new(),
                camera_eye: [0.0; 3],
                sky_factor: 1.0,
                sky_color: [1.0, 1.0, 1.0],
                fog: parse_fog_env(),
                tex_size,
                mip_levels,
                animations: Vec::new(),
                anim_last: Vec::new(),
                drawn_last_frame: 0,
                culled_last_frame: 0,
            })
        }
    }

    /// Register animated texture layers (from the bake). Frames are
    /// re-uploaded into the texture array as `anim_tick` advances.
    pub fn set_animations(&mut self, animations: Vec<LayerAnimation>) {
        self.anim_last = vec![None; animations.len()];
        self.animations = animations;
    }

    /// Advance texture animations to the given game tick (20 Hz), and
    /// re-upload any layer whose frame changed. A few KB per change —
    /// water/lava tick every `frametime` (2) ticks.
    pub fn anim_tick(&mut self, gpu: &mut Gpu, tick: u64) -> Result<(), String> {
        for i in 0..self.animations.len() {
            let a = &self.animations[i];
            if a.order.is_empty() || a.frames.is_empty() {
                continue;
            }
            let step = (tick / a.frametime as u64) as usize % a.order.len();
            let frame = a.order[step] as usize % a.frames.len();
            if self.anim_last[i] == Some(frame) {
                continue;
            }
            self.anim_last[i] = Some(frame);
            let layer = a.layer;
            let rgba = self.animations[i].frames[frame].clone();
            self.upload_layer_frame(gpu, layer, &rgba)?;
        }
        Ok(())
    }

    /// Replace one texture-array layer's content (all mip levels) via an
    /// async slot upload (no CPU wait — same-queue ordering keeps the
    /// frame's sampling safe, and the in-cb barriers order the layer's
    /// shader reads around the copy).
    fn upload_layer_frame(&mut self, gpu: &mut Gpu, layer: u16, rgba: &[u8]) -> Result<(), String> {
        let mips = generate_mips(self.tex_size, self.mip_levels, rgba);
        let total: usize = mips.iter().map(|m| m.len()).sum();
        let slot = self.acquire_slot(gpu, total as u64)?;
        unsafe {
            let alloc = self.upload_slots[slot].staging_alloc.as_mut().unwrap();
            let slice = alloc.mapped_slice_mut().ok_or("staging not mapped")?;
            let mut off = 0usize;
            let mut copies = Vec::with_capacity(mips.len());
            let mut size = self.tex_size;
            for (level, mip) in mips.iter().enumerate() {
                slice[off..off + mip.len()].copy_from_slice(mip);
                copies.push(
                    vk::BufferImageCopy::default()
                        .buffer_offset(off as u64)
                        .image_subresource(
                            vk::ImageSubresourceLayers::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .mip_level(level as u32)
                                .base_array_layer(layer as u32)
                                .layer_count(1),
                        )
                        .image_extent(vk::Extent3D {
                            width: size,
                            height: size,
                            depth: 1,
                        }),
                );
                off += mip.len();
                size = (size / 2).max(1);
            }

            let cb = self.begin_slot(gpu, slot)?;
            let staging = self.upload_slots[slot].staging;
            let device = &gpu.device;
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(0)
                .level_count(self.mip_levels)
                .base_array_layer(layer as u32)
                .layer_count(1);
            let to_dst = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .src_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .image(self.image)
                .subresource_range(range);
            device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default()
                    .image_memory_barriers(std::slice::from_ref(&to_dst)),
            );
            device.cmd_copy_buffer_to_image(
                cb,
                staging,
                self.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &copies,
            );
            let to_read = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image(self.image)
                .subresource_range(range);
            device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default()
                    .image_memory_barriers(std::slice::from_ref(&to_read)),
            );
        }
        self.submit_slot(gpu, slot)
    }

    /// Set the frame's eye position — drives translucent back-to-front
    /// column sorting. Callers that never render water can skip it.
    pub fn set_camera(&mut self, eye: [f32; 3]) {
        self.camera_eye = eye;
    }

    /// Set the time-of-day lightmap state: `sky_factor` scales the sky half
    /// of the light (vanilla `SKY_LIGHT_FACTOR`, 1.0 midday → 0.24 midnight)
    /// and `sky_color` tints it (white → blue). Block light is unaffected —
    /// a torch is as bright at midnight as at noon, which is the whole point
    /// of keeping the two channels separate through to the shader.
    pub fn set_lightmap(&mut self, sky_factor: f32, sky_color: [f32; 3]) {
        self.sky_factor = sky_factor;
        self.sky_color = sky_color;
    }

    /// Attach the entity pass (mob models / capsules + nametags).
    /// Callers that never attach it (demo/view/bench snapshots) pay
    /// nothing. Mobs whose textures are missing fall back to capsules.
    pub fn init_entities(
        &mut self,
        gpu: &mut Gpu,
        font: Option<FontData<'_>>,
        tex: MobTextures<'_>,
    ) -> Result<(), String> {
        self.entities = Some(EntityPass::new(gpu, self.color_format, font, tex)?);
        Ok(())
    }

    /// `init_entities` with resource-pack CEM model overrides (M9).
    pub fn init_entities_with_cem(
        &mut self,
        gpu: &mut Gpu,
        font: Option<FontData<'_>>,
        tex: MobTextures<'_>,
        cem: std::collections::HashMap<crate::mobs::EntityModelKind, crate::mobs::Model>,
    ) -> Result<(), String> {
        self.entities = Some(EntityPass::new_with_cem(gpu, self.color_format, font, tex, cem)?);
        Ok(())
    }

    /// The attached entity pass, if any — `rewo mobshot` uses it to iterate
    /// available mob models and their geometric ground truth.
    pub fn entity_pass(&self) -> Option<&EntityPass> {
        self.entities.as_ref()
    }

    /// Upload a player's 64×64 skin into the entity atlas and return the UV
    /// offset for its `EntityDraw::skin_uv`. `None` if the entity pass isn't
    /// initialized (view/demo/bench paths).
    pub fn upload_player_skin(&mut self, gpu: &mut Gpu, rgba: &[u8]) -> Option<[f32; 2]> {
        let pass = self.entities.as_mut()?;
        match pass.upload_skin(gpu, rgba) {
            Ok(uv) => Some(uv),
            Err(e) => {
                log::warn!("entities: skin upload failed: {e}");
                None
            }
        }
    }

    /// Attach the in-game HUD pass (crosshair/hotbar/hearts/hunger).
    pub fn init_hud(&mut self, gpu: &mut Gpu, sprites: &HudSpritesData<'_>) -> Result<(), String> {
        self.hud = Some(HudPass::new(gpu, self.color_format, sprites)?);
        Ok(())
    }

    /// Set this frame's HUD state (health/food 0..20, selected slot 0..8).
    /// Never called → no HUD (view/demo/bench).
    pub fn set_hud(&mut self, health: f32, food: i32, slot: u8) {
        self.hud_state = Some((health, food, slot));
    }

    /// Attach the screen-space text pass (chat + coords overlay).
    pub fn init_text(&mut self, gpu: &mut Gpu, font: &FontData<'_>) -> Result<(), String> {
        self.text = Some(TextPass::new(gpu, self.color_format, font)?);
        Ok(())
    }

    /// Replace this frame's screen-space text lines (chat, coords).
    pub fn set_text(&mut self, lines: Vec<OwnedTextLine>) {
        self.text_lines = lines;
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
        if let Some(pass) = self.entities.as_mut() {
            pass.set_draws(draws, cam_right, cam_up, time, self.camera_eye);
        }
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// Find (or wait for) a free upload slot with at least `need` staging
    /// bytes. Retired fences are harvested opportunistically; the CPU only
    /// blocks when every slot is still in flight (sustained burst).
    fn acquire_slot(&mut self, gpu: &mut Gpu, need: u64) -> Result<usize, String> {
        unsafe {
            for s in &mut self.upload_slots {
                if s.busy && gpu.device.get_fence_status(s.fence).unwrap_or(false) {
                    gpu.device
                        .reset_fences(&[s.fence])
                        .map_err(|e| format!("slot fence reset: {e}"))?;
                    s.busy = false;
                }
            }
            let idx = match self.upload_slots.iter().position(|s| !s.busy) {
                Some(i) => i,
                None => {
                    let fence = self.upload_slots[0].fence;
                    gpu.device
                        .wait_for_fences(&[fence], true, u64::MAX)
                        .map_err(|e| format!("slot wait: {e}"))?;
                    gpu.device
                        .reset_fences(&[fence])
                        .map_err(|e| format!("slot reset: {e}"))?;
                    self.upload_slots[0].busy = false;
                    0
                }
            };
            if self.upload_slots[idx].staging_cap < need {
                let new_cap = need.next_power_of_two();
                let old = self.upload_slots[idx].staging;
                gpu.device.destroy_buffer(old, None);
                if let Some(a) = self.upload_slots[idx].staging_alloc.take() {
                    let _ = gpu.allocator.free(a);
                }
                let (buf, alloc) = create_host_buffer(
                    gpu,
                    new_cap,
                    vk::BufferUsageFlags::TRANSFER_SRC,
                    "upload-staging",
                )?;
                let s = &mut self.upload_slots[idx];
                s.staging = buf;
                s.staging_alloc = Some(alloc);
                s.staging_cap = new_cap;
            }
            Ok(idx)
        }
    }

    /// Begin the slot's command buffer (implicitly reset — the pool has
    /// RESET_COMMAND_BUFFER).
    fn begin_slot(&self, gpu: &Gpu, idx: usize) -> Result<vk::CommandBuffer, String> {
        let cb = self.upload_slots[idx].cb;
        unsafe {
            gpu.device
                .begin_command_buffer(
                    cb,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("slot begin: {e}"))?;
        }
        Ok(cb)
    }

    /// End + submit the slot on the graphics queue, signaling its fence —
    /// and RETURN, no CPU wait. Queue FIFO order makes the copy execute
    /// before any later-submitted frame that reads the regions.
    fn submit_slot(&mut self, gpu: &Gpu, idx: usize) -> Result<(), String> {
        let s = &mut self.upload_slots[idx];
        unsafe {
            gpu.device
                .end_command_buffer(s.cb)
                .map_err(|e| format!("slot end: {e}"))?;
            let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(s.cb)];
            gpu.device
                .queue_submit2(
                    gpu.graphics_queue,
                    &[vk::SubmitInfo2::default().command_buffer_infos(&cbs)],
                    s.fence,
                )
                .map_err(|e| format!("slot submit: {e}"))?;
        }
        s.busy = true;
        Ok(())
    }

    /// Suballocate + upload a column's geometry into the mega-buffers via
    /// an async staged copy (slot ring — the CPU does not wait for the
    /// copy). Opaque and translucent (water) sets ride the same buffers as
    /// two regions; either may be empty.
    #[allow(clippy::too_many_arguments)]
    pub fn upload_column(
        &mut self,
        gpu: &mut Gpu,
        cx: i32,
        cz: i32,
        vertex_bytes: &[u8],
        indices: &[u32],
        tvertex_bytes: &[u8],
        tindices: &[u32],
        y_min: f32,
        y_max: f32,
    ) -> Result<(), String> {
        self.remove_column(gpu, cx, cz);
        let vtx_count = (vertex_bytes.len() as u64) / VERTEX_STRIDE;
        let idx_count = indices.len() as u64;
        let tvtx_count = (tvertex_bytes.len() as u64) / VERTEX_STRIDE;
        let tidx_count = tindices.len() as u64;
        if idx_count == 0 && tidx_count == 0 {
            return Ok(());
        }
        // Four regions from the two shared free-lists (zero-length allocs
        // are free). Roll back everything on any failure.
        let allocs = (|| {
            let v = self.vfree.alloc(vtx_count)?;
            let Some(i) = self.ifree.alloc(idx_count) else {
                self.vfree.free_region(v, vtx_count);
                return None;
            };
            let Some(tv) = self.vfree.alloc(tvtx_count) else {
                self.vfree.free_region(v, vtx_count);
                self.ifree.free_region(i, idx_count);
                return None;
            };
            let Some(ti) = self.ifree.alloc(tidx_count) else {
                self.vfree.free_region(v, vtx_count);
                self.ifree.free_region(i, idx_count);
                self.vfree.free_region(tv, tvtx_count);
                return None;
            };
            Some((v, i, tv, ti))
        })();
        let Some((vtx_off, idx_off, tvtx_off, tidx_off)) = allocs else {
            log::warn!("world: mega-buffer full — dropping column ({cx},{cz})");
            return Ok(());
        };

        // Stage all four spans contiguously, copy each non-empty one.
        let index_bytes: &[u8] = bytemuck::cast_slice(indices);
        let tindex_bytes: &[u8] = bytemuck::cast_slice(tindices);
        let spans = [
            (vertex_bytes, self.vbuf, vtx_off * VERTEX_STRIDE),
            (index_bytes, self.ibuf, idx_off * 4),
            (tvertex_bytes, self.vbuf, tvtx_off * VERTEX_STRIDE),
            (tindex_bytes, self.ibuf, tidx_off * 4),
        ];
        let total: usize = spans.iter().map(|(b, _, _)| b.len()).sum();
        let slot = self.acquire_slot(gpu, total as u64)?;
        unsafe {
            let alloc = self.upload_slots[slot].staging_alloc.as_mut().unwrap();
            let slice = alloc.mapped_slice_mut().ok_or("staging not mapped")?;
            let mut off = 0usize;
            for (bytes, _, _) in &spans {
                slice[off..off + bytes.len()].copy_from_slice(bytes);
                off += bytes.len();
            }
            let cb = self.begin_slot(gpu, slot)?;
            let staging = self.upload_slots[slot].staging;
            let mut src = 0u64;
            for (bytes, dst_buf, dst_off) in &spans {
                if !bytes.is_empty() {
                    gpu.device.cmd_copy_buffer(
                        cb,
                        staging,
                        *dst_buf,
                        &[vk::BufferCopy::default()
                            .src_offset(src)
                            .dst_offset(*dst_off)
                            .size(bytes.len() as u64)],
                    );
                }
                src += bytes.len() as u64;
            }
            // Make the copies visible to later frames' vertex/index reads
            // without relying on fence-signal domain semantics alone.
            let vis = vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(
                    vk::PipelineStageFlags2::VERTEX_INPUT
                        | vk::PipelineStageFlags2::INDEX_INPUT,
                )
                .dst_access_mask(
                    vk::AccessFlags2::VERTEX_ATTRIBUTE_READ | vk::AccessFlags2::INDEX_READ,
                );
            gpu.device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default().memory_barriers(std::slice::from_ref(&vis)),
            );
        }
        self.submit_slot(gpu, slot)?;

        let meta = ColumnMeta {
            aabb_min: [cx as f32 * 16.0, y_min, cz as f32 * 16.0, 0.0],
            aabb_max: [cx as f32 * 16.0 + 16.0, y_max, cz as f32 * 16.0 + 16.0, 0.0],
            index_count: idx_count as u32,
            first_index: idx_off as u32,
            vertex_offset: vtx_off as i32,
            _pad: 0,
        };
        self.columns.insert(
            (cx, cz),
            Slot {
                vtx_off,
                vtx_len: vtx_count,
                idx_off,
                idx_len: idx_count,
                tvtx_off,
                tvtx_len: tvtx_count,
                tidx_off,
                tidx_len: tidx_count,
                meta,
            },
        );
        self.meta_dirty = true;
        Ok(())
    }

    pub fn remove_column(&mut self, _gpu: &mut Gpu, cx: i32, cz: i32) {
        if let Some(slot) = self.columns.remove(&(cx, cz)) {
            self.vfree.free_region(slot.vtx_off, slot.vtx_len);
            self.ifree.free_region(slot.idx_off, slot.idx_len);
            self.vfree.free_region(slot.tvtx_off, slot.tvtx_len);
            self.ifree.free_region(slot.tidx_off, slot.tidx_len);
            self.meta_dirty = true;
        }
    }

    /// Rebuild the compacted metadata array (only when columns changed) and
    /// map-write it into the host-visible SSBO.
    fn sync_meta(&mut self) {
        if !self.meta_dirty {
            return;
        }
        let metas: Vec<ColumnMeta> = self.columns.values().map(|s| s.meta).collect();
        self.column_count = metas.len().min(MAX_COLUMNS) as u32;
        if let Some(alloc) = self.meta_alloc.as_mut() {
            if let Some(slice) = alloc.mapped_slice_mut() {
                let bytes: &[u8] = bytemuck::cast_slice(&metas[..self.column_count as usize]);
                slice[..bytes.len()].copy_from_slice(bytes);
            }
        }
        self.meta_dirty = false;
    }

    /// Pre-pass (OUTSIDE dynamic rendering): reset the draw count, dispatch
    /// the compute cull, and barrier its writes for the indirect draw.
    pub fn cull(&mut self, gpu: &Gpu, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4]) {
        self.sync_meta();
        let device = &gpu.device;
        unsafe {
            // Reset the atomic count to 0.
            device.cmd_fill_buffer(cb, self.count_buf, 0, 4, 0);
            let reset_barrier = vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.count_buf)
                .offset(0)
                .size(vk::WHOLE_SIZE);
            device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default()
                    .buffer_memory_barriers(std::slice::from_ref(&reset_barrier)),
            );

            if self.column_count > 0 {
                device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::COMPUTE, self.cull_pipeline);
                device.cmd_bind_descriptor_sets(
                    cb,
                    vk::PipelineBindPoint::COMPUTE,
                    self.cull_layout,
                    0,
                    &[self.cull_set],
                    &[],
                );
                let push = CullPush {
                    planes: frustum_planes(&view_proj),
                    column_count: self.column_count,
                    _pad: [0; 3],
                };
                device.cmd_push_constants(
                    cb,
                    self.cull_layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    bytemuck::bytes_of(&push),
                );
                let groups = self.column_count.div_ceil(64);
                device.cmd_dispatch(cb, groups, 1, 1);

                // Compute writes → indirect draw reads (cmds + count).
                let barriers = [
                    buf_barrier(
                        self.indirect_buf,
                        vk::PipelineStageFlags2::COMPUTE_SHADER,
                        vk::AccessFlags2::SHADER_STORAGE_WRITE,
                        vk::PipelineStageFlags2::DRAW_INDIRECT,
                        vk::AccessFlags2::INDIRECT_COMMAND_READ,
                    ),
                    buf_barrier(
                        self.count_buf,
                        vk::PipelineStageFlags2::COMPUTE_SHADER,
                        vk::AccessFlags2::SHADER_STORAGE_WRITE,
                        vk::PipelineStageFlags2::DRAW_INDIRECT,
                        vk::AccessFlags2::INDIRECT_COMMAND_READ,
                    ),
                ];
                device.cmd_pipeline_barrier2(
                    cb,
                    &vk::DependencyInfo::default().buffer_memory_barriers(&barriers),
                );
            }
        }
        self.drawn_last_frame = self.column_count as usize; // pre-cull estimate
        self.culled_last_frame = 0;
    }

    /// Override the distance-fog band (defaults from `REWO_FOG`).
    pub fn set_fog(&mut self, start: f32, end: f32) {
        self.fog = [start, end.max(start + 1.0)];
    }

    /// Set the targeted block for the selection outline (`None` = hidden).
    pub fn set_selection(&mut self, block: Option<[i32; 3]>) {
        self.selection = block;
    }

    /// Push the world/fog constants (view_proj + camera + fog band).
    fn push_world(&self, device: &ash::Device, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4]) {
        let push = WorldPush {
            view_proj,
            cam_fog: [self.camera_eye[0], self.camera_eye[1], self.camera_eye[2], self.fog[0]],
            fog_col: [SKY_HORIZON[0], SKY_HORIZON[1], SKY_HORIZON[2], self.fog[1]],
            light: [self.sky_factor, BLOCK_LIGHT_FACTOR, 0.0, 0.0],
            sky_col: [self.sky_color[0], self.sky_color[1], self.sky_color[2], 0.0],
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
    pub fn draw(&mut self, gpu: &Gpu, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4], extent: vk::Extent2D) {
        self.draw_sky(gpu, cb, view_proj, extent);
        if self.column_count > 0 {
            self.draw_terrain(gpu, cb, view_proj, extent);
        }
        // Selection outline over the terrain (depth-tested against it).
        self.draw_selection(gpu, cb, view_proj, extent);
        if let Some(pass) = &self.entities {
            pass.draw_solid(gpu, cb, view_proj, extent);
        }
        self.draw_translucent(gpu, cb, view_proj, extent);
        if let Some(pass) = &self.entities {
            pass.draw_text(gpu, cb, view_proj, extent);
        }
        // HUD, then text (chat/coords), last — all screen space.
        if let (Some(hud), Some((health, food, slot))) = (self.hud.as_mut(), self.hud_state) {
            hud.draw(gpu, cb, extent, health, food, slot);
        }
        if let Some(text) = self.text.as_mut() {
            if !self.text_lines.is_empty() {
                let lines: Vec<TextLine> = self
                    .text_lines
                    .iter()
                    .map(|l| TextLine {
                        x: l.x,
                        y: l.y,
                        px: l.px,
                        color: l.color,
                        alpha: l.alpha,
                        text: &l.text,
                    })
                    .collect();
                text.draw(gpu, cb, extent, &lines);
            }
        }
    }

    /// The block-selection wireframe: the 12 edges of the targeted block,
    /// slightly inflated to avoid z-fighting, depth-tested against terrain.
    fn draw_selection(&mut self, gpu: &Gpu, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4], extent: vk::Extent2D) {
        let Some(b) = self.selection else { return };
        let (x0, y0, z0) = (b[0] as f32 - 0.002, b[1] as f32 - 0.002, b[2] as f32 - 0.002);
        let (x1, y1, z1) = (b[0] as f32 + 1.002, b[1] as f32 + 1.002, b[2] as f32 + 1.002);
        // 8 corners → 12 edges → 24 line vertices.
        let c = [
            [x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1], // bottom
            [x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1], // top
        ];
        let edges = [
            (0, 1), (1, 2), (2, 3), (3, 0), // bottom loop
            (4, 5), (5, 6), (6, 7), (7, 4), // top loop
            (0, 4), (1, 5), (2, 6), (3, 7), // verticals
        ];
        let mut verts = [[0f32; 3]; 24];
        for (i, &(a, bb)) in edges.iter().enumerate() {
            verts[i * 2] = c[a];
            verts[i * 2 + 1] = c[bb];
        }

        self.select_cursor = (self.select_cursor + 1) % SELECT_RING;
        if let Some(slice) = self.select_allocs[self.select_cursor]
            .as_mut()
            .and_then(|a| a.mapped_slice_mut())
        {
            let bytes: &[u8] = bytemuck::cast_slice(&verts);
            slice[..bytes.len()].copy_from_slice(bytes);
        }
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
            device.cmd_bind_vertex_buffers(cb, 0, &[self.select_bufs[self.select_cursor]], &[0]);
            device.cmd_draw(cb, 24, 1, 0, 0);
        }
    }

    /// Fullscreen gradient sky — drawn first, no depth test/write, so the
    /// terrain (reversed-Z GREATER, cleared 0.0) draws over it and the fog
    /// fade meets a matching horizon color.
    fn draw_sky(&self, gpu: &Gpu, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4], extent: vk::Extent2D) {
        let inv = glam::Mat4::from_cols_array_2d(&view_proj)
            .inverse()
            .to_cols_array_2d();
        let push = SkyPush {
            inv_view_proj: inv,
            horizon: [SKY_HORIZON[0], SKY_HORIZON[1], SKY_HORIZON[2], 1.0],
            zenith: [SKY_ZENITH[0], SKY_ZENITH[1], SKY_ZENITH[2], 1.0],
        };
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.sky_pipeline);
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            device.cmd_push_constants(
                cb,
                self.sky_layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                std::slice::from_raw_parts(
                    (&push as *const SkyPush) as *const u8,
                    std::mem::size_of::<SkyPush>(),
                ),
            );
            device.cmd_draw(cb, 3, 1, 0, 0);
        }
    }

    /// Water: per-column direct draws, CPU frustum-culled (mirroring
    /// cull.comp's positive-vertex test) and sorted far→near — the plan's
    /// "per-section back-to-front CPU sort; intra-section artifacts
    /// accepted v1". Column counts with water are small; the indirect path
    /// stays opaque-only.
    fn draw_translucent(&mut self, gpu: &Gpu, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4], extent: vk::Extent2D) {
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
        let eye = self.camera_eye;
        let mut cols: Vec<(f32, u32, u32, i32)> = self
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
        cols.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

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
                &[self.tex_set],
                &[],
            );
            self.push_world(device, cb, view_proj);
            device.cmd_bind_vertex_buffers(cb, 0, &[self.vbuf], &[0]);
            device.cmd_bind_index_buffer(cb, self.ibuf, 0, vk::IndexType::UINT32);
            for (_, count, first, voff) in cols {
                device.cmd_draw_indexed(cb, count, 1, first, voff, 0);
            }
        }
    }

    fn draw_terrain(&mut self, gpu: &Gpu, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4], extent: vk::Extent2D) {
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
                &[self.tex_set],
                &[],
            );
            self.push_world(device, cb, view_proj);
            device.cmd_bind_vertex_buffers(cb, 0, &[self.vbuf], &[0]);
            device.cmd_bind_index_buffer(cb, self.ibuf, 0, vk::IndexType::UINT32);
            device.cmd_draw_indexed_indirect_count(
                cb,
                self.indirect_buf,
                0,
                self.count_buf,
                0,
                self.column_count,
                std::mem::size_of::<vk::DrawIndexedIndirectCommand>() as u32,
            );
        }
    }

    /// Read back the GPU cull's draw count from the last frame (one-shot
    /// copy + a blocking wait — for verification/stats, not per-frame use).
    pub fn read_draw_count(&mut self, gpu: &mut Gpu) -> u32 {
        let Ok(slot) = self.acquire_slot(gpu, 0) else {
            return 0;
        };
        let Ok(cb) = self.begin_slot(gpu, slot) else {
            return 0;
        };
        unsafe {
            gpu.device.cmd_copy_buffer(
                cb,
                self.count_buf,
                self.count_readback,
                &[vk::BufferCopy::default().size(4)],
            );
        }
        if self.submit_slot(gpu, slot).is_err() {
            return 0;
        }
        unsafe {
            let fence = self.upload_slots[slot].fence;
            let _ = gpu.device.wait_for_fences(&[fence], true, u64::MAX);
            let _ = gpu.device.reset_fences(&[fence]);
            self.upload_slots[slot].busy = false;
            self.count_readback_alloc
                .as_ref()
                .and_then(|a| a.mapped_slice())
                .map(|s| u32::from_ne_bytes(s[..4].try_into().unwrap()))
                .unwrap_or(0)
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        if let Some(mut pass) = self.entities.take() {
            pass.destroy(gpu);
        }
        if let Some(mut hud) = self.hud.take() {
            hud.destroy(gpu);
        }
        if let Some(mut text) = self.text.take() {
            text.destroy(gpu);
        }
        unsafe {
            let device = &gpu.device;
            for s in &mut self.upload_slots {
                device.destroy_fence(s.fence, None);
                device.destroy_buffer(s.staging, None);
            }
            device.destroy_command_pool(self.upload_pool, None);
            device.destroy_pipeline(self.cull_pipeline, None);
            device.destroy_pipeline_layout(self.cull_layout, None);
            device.destroy_descriptor_pool(self.cull_pool, None);
            device.destroy_descriptor_set_layout(self.cull_set_layout, None);
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline(self.water_pipeline, None);
            device.destroy_pipeline(self.sky_pipeline, None);
            device.destroy_pipeline_layout(self.sky_layout, None);
            device.destroy_pipeline(self.select_pipeline, None);
            device.destroy_pipeline_layout(self.select_layout, None);
            for b in self.select_bufs {
                device.destroy_buffer(b, None);
            }
            device.destroy_pipeline_layout(self.graphics_layout, None);
            device.destroy_descriptor_pool(self.tex_pool, None);
            device.destroy_descriptor_set_layout(self.tex_set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            for b in [
                self.vbuf,
                self.ibuf,
                self.meta_buf,
                self.indirect_buf,
                self.count_buf,
                self.count_readback,
            ] {
                device.destroy_buffer(b, None);
            }
        }
        let mut slot_allocs: Vec<Option<Allocation>> = self
            .upload_slots
            .iter_mut()
            .map(|s| s.staging_alloc.take())
            .collect();
        for a in self.select_allocs.iter_mut() {
            slot_allocs.push(a.take());
        }
        for a in [
            self.image_alloc.take(),
            self.vbuf_alloc.take(),
            self.ibuf_alloc.take(),
            self.meta_alloc.take(),
            self.indirect_alloc.take(),
            self.count_alloc.take(),
            self.count_readback_alloc.take(),
        ]
        .into_iter()
        .chain(slot_allocs)
        .flatten()
        {
            let _ = gpu.allocator.free(a);
        }
    }
}

// -- helpers -----------------------------------------------------------------

fn storage_binding(binding: u32) -> vk::DescriptorSetLayoutBinding<'static> {
    vk::DescriptorSetLayoutBinding::default()
        .binding(binding)
        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
}

fn write_storage(device: &ash::Device, set: vk::DescriptorSet, binding: u32, buf: vk::Buffer) {
    let info = [vk::DescriptorBufferInfo::default()
        .buffer(buf)
        .offset(0)
        .range(vk::WHOLE_SIZE)];
    unsafe {
        device.update_descriptor_sets(
            &[vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(binding)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&info)],
            &[],
        );
    }
}

fn buf_barrier<'a>(
    buf: vk::Buffer,
    src_stage: vk::PipelineStageFlags2,
    src_access: vk::AccessFlags2,
    dst_stage: vk::PipelineStageFlags2,
    dst_access: vk::AccessFlags2,
) -> vk::BufferMemoryBarrier2<'a> {
    vk::BufferMemoryBarrier2::default()
        .src_stage_mask(src_stage)
        .src_access_mask(src_access)
        .dst_stage_mask(dst_stage)
        .dst_access_mask(dst_access)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .buffer(buf)
        .offset(0)
        .size(vk::WHOLE_SIZE)
}

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

/// Default fog band, overridable via `REWO_FOG=start,end`. The default
/// end (~180) sits at the flat-world test's loaded radius so the far
/// chunk boundary fully dissolves into the horizon (fog color = sky
/// horizon) instead of ending hard. Ideally derived from render distance;
/// a fixed default + override is the current compromise.
fn parse_fog_env() -> [f32; 2] {
    let default = [80.0, 180.0];
    std::env::var("REWO_FOG")
        .ok()
        .and_then(|s| {
            let mut it = s.split(',');
            let a: f32 = it.next()?.trim().parse().ok()?;
            let b: f32 = it.next()?.trim().parse().ok()?;
            Some([a, b.max(a + 1.0)])
        })
        .unwrap_or(default)
}

/// Line pipeline for the selection outline: LINE_LIST, depth-tested
/// (reversed-Z GREATER) but no depth write, alpha-blended.
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
            .stride(12)
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
                vk::ColorComponentFlags::R | vk::ColorComponentFlags::G | vk::ColorComponentFlags::B,
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
            .map_err(|(_, e)| format!("line pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

/// Fullscreen gradient-sky pipeline: no vertex input, no depth, no blend.
fn build_sky_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/sky.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/sky.frag.spv")),
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
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
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
        // No depth test/write — the sky is pure background.
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(false)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B,
            )];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [color_format];
        // The pass carries a depth attachment (terrain uses it); the sky
        // pipeline must declare the same format to match, even though its
        // depth test/write are disabled.
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
            .map_err(|(_, e)| format!("sky pipeline: {e}"))?[0];
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
            vk::VertexInputAttributeDescription::default()
                .location(3)
                .format(vk::Format::R32G32B32_SFLOAT)
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
            .map_err(|(_, e)| format!("world pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

// -- texture upload (unchanged from M2/M4) -----------------------------------

fn generate_mips(tex_size: u32, mip_levels: u32, base: &[u8]) -> Vec<Vec<u8>> {
    let mut out = vec![base.to_vec()];
    let mut prev_size = tex_size as usize;
    for _ in 1..mip_levels {
        let size = (prev_size / 2).max(1);
        let prev = out.last().unwrap();
        let mut mip = vec![0u8; size * size * 4];
        for y in 0..size {
            for x in 0..size {
                for c in 0..4 {
                    let mut sum = 0u32;
                    for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                        let sy = (y * 2 + dy).min(prev_size - 1);
                        let sx = (x * 2 + dx).min(prev_size - 1);
                        sum += prev[(sy * prev_size + sx) * 4 + c] as u32;
                    }
                    mip[(y * size + x) * 4 + c] = (sum / 4) as u8;
                }
            }
        }
        out.push(mip);
        prev_size = size;
    }
    out
}

fn upload_texture_array(
    gpu: &mut Gpu,
    image: vk::Image,
    tex_size: u32,
    mip_levels: u32,
    layers: &[Vec<u8>],
) -> Result<(), String> {
    let layer_count = layers.len().max(1);
    let mut staging_data = Vec::new();
    let mut mip_offsets = Vec::new();
    let per_layer_mips: Vec<Vec<Vec<u8>>> = if layers.is_empty() {
        vec![generate_mips(
            tex_size,
            mip_levels,
            &vec![255u8; (tex_size * tex_size * 4) as usize],
        )]
    } else {
        layers.iter().map(|l| generate_mips(tex_size, mip_levels, l)).collect()
    };
    for mip in 0..mip_levels as usize {
        mip_offsets.push(staging_data.len() as u64);
        for layer_mips in &per_layer_mips {
            staging_data.extend_from_slice(&layer_mips[mip]);
        }
    }

    unsafe {
        let device = gpu.device.clone();
        let staging = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(staging_data.len() as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("staging: {e}"))?;
        let req = device.get_buffer_memory_requirements(staging);
        let mut staging_alloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name: "texture-staging",
                requirements: req,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("staging alloc: {e}"))?;
        device
            .bind_buffer_memory(staging, staging_alloc.memory(), staging_alloc.offset())
            .map_err(|e| format!("staging bind: {e}"))?;
        staging_alloc.mapped_slice_mut().ok_or("staging not mapped")?[..staging_data.len()]
            .copy_from_slice(&staging_data);

        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.graphics_family),
                None,
            )
            .map_err(|e| format!("upload pool: {e}"))?;
        let cb = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .map_err(|e| format!("upload cb: {e}"))?[0];
        device
            .begin_command_buffer(
                cb,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|e| format!("begin: {e}"))?;
        let full_range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .base_mip_level(0)
            .level_count(mip_levels)
            .base_array_layer(0)
            .layer_count(layer_count as u32);
        let to_dst = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TOP_OF_PIPE)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(full_range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_dst)),
        );
        let mut regions = Vec::new();
        for mip in 0..mip_levels {
            let size = (tex_size >> mip).max(1);
            regions.push(
                vk::BufferImageCopy::default()
                    .buffer_offset(mip_offsets[mip as usize])
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .mip_level(mip)
                            .base_array_layer(0)
                            .layer_count(layer_count as u32),
                    )
                    .image_extent(vk::Extent3D {
                        width: size,
                        height: size,
                        depth: 1,
                    }),
            );
        }
        device.cmd_copy_buffer_to_image(
            cb,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &regions,
        );
        let to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(full_range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_read)),
        );
        device.end_command_buffer(cb).map_err(|e| format!("end: {e}"))?;
        let fence = device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .map_err(|e| format!("fence: {e}"))?;
        let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(cb)];
        device
            .queue_submit2(
                gpu.graphics_queue,
                &[vk::SubmitInfo2::default().command_buffer_infos(&cbs)],
                fence,
            )
            .map_err(|e| format!("submit: {e}"))?;
        device.wait_for_fences(&[fence], true, u64::MAX).map_err(|e| format!("wait: {e}"))?;
        device.destroy_fence(fence, None);
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(staging, None);
        let _ = gpu.allocator.free(staging_alloc);
    }
    Ok(())
}

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
            let barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TOP_OF_PIPE)
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
                &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&barrier)),
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

fn frustum_planes(m: &[[f32; 4]; 4]) -> [[f32; 4]; 6] {
    let row = |i: usize| -> [f32; 4] { [m[0][i], m[1][i], m[2][i], m[3][i]] };
    let r0 = row(0);
    let r1 = row(1);
    let r2 = row(2);
    let r3 = row(3);
    let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
    let sub = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
    [
        add(r3, r0),
        sub(r3, r0),
        add(r3, r1),
        sub(r3, r1),
        r2,
        sub(r3, r2),
    ]
}

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
