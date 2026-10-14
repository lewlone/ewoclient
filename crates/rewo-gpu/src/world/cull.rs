//! GPU culling: a per-column metadata SSBO (world-space AABB + draw params)
//! feeds a **compute cull pass** — one invocation per column frustum-tests its
//! AABB and, if visible, appends a `VkDrawIndexedIndirectCommand` and bumps an
//! atomic count. The CPU then issues a single `vkCmdDrawIndexedIndirectCount`,
//! so the draw cost stops scaling with column count.
//!
//! Frame split: [`WorldRenderer::cull`] runs the compute + count reset BEFORE
//! dynamic rendering begins (compute can't run inside a render pass); the
//! indirect draw itself is recorded by `draw` INSIDE the pass.

use ash::vk;
use bytemuck::{Pod, Zeroable};
use gpu_allocator::vulkan::Allocation;

use super::arena::{column_dist2, Arena, MAX_COLUMNS};
use super::resources::{create_device_buffer, create_host_buffer};
use super::WorldRenderer;
use crate::Gpu;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
pub(super) struct ColumnMeta {
    pub(super) aabb_min: [f32; 4],
    pub(super) aabb_max: [f32; 4],
    pub(super) index_count: u32,
    pub(super) first_index: u32,
    pub(super) vertex_offset: i32,
    pub(super) _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CullPush {
    planes: [[f32; 4]; 6],
    column_count: u32,
    _pad: [u32; 3],
}

/// One copy of the cull's inputs and outputs (column metadata, indirect
/// commands, draw count) per frame that can be in flight. `cull` writes them
/// inside `render`, after that frame's fence wait, so — like the lightmap UBO —
/// `>= fif` slots suffice. A single shared set let frame N's `cmd_fill_buffer`
/// and compute writes race frame N-1's `DrawIndexedIndirectCount` reads.
const CULL_RING: usize = crate::MAX_FRAMES_IN_FLIGHT;

/// The column metadata the cull shader sees, at most `max` of them. Under the
/// cap the order is whatever `columns` yields (unchanged behaviour). Over it,
/// the kept set is **the nearest `max` columns to `eye`** (XZ distance to the
/// column centre, ties by coordinate), so which columns vanish is a property
/// of where the camera is, not of `HashMap` iteration order.
fn select_metas(
    columns: impl Iterator<Item = ((i32, i32), ColumnMeta)>,
    eye: [f32; 3],
    max: usize,
) -> Vec<ColumnMeta> {
    let mut all: Vec<((i32, i32), ColumnMeta)> = columns.collect();
    if all.len() > max {
        all.sort_by(|a, b| {
            column_dist2(a.0, eye)
                .total_cmp(&column_dist2(b.0, eye))
                .then(a.0.cmp(&b.0))
        });
        all.truncate(max);
    }
    all.into_iter().map(|(_, m)| m).collect()
}

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

pub(super) fn buf_barrier<'a>(
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

pub(super) fn frustum_planes(m: &[[f32; 4]; 4]) -> [[f32; 4]; 6] {
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

/// The GPU culling state: the compute pipeline, the per-frame-in-flight rings
/// of column-metadata / indirect-command / draw-count buffers it reads and
/// writes, the compacted metadata array they are filled from, and the
/// readback buffer for stats.
pub(super) struct Culler {
    set_layout: vk::DescriptorSetLayout,
    pub(super) layout: vk::PipelineLayout,
    pub(super) pipeline: vk::Pipeline,
    pool: vk::DescriptorPool,
    pub(super) sets: [vk::DescriptorSet; CULL_RING],
    // -- column metadata --
    /// The compacted metadata array, and a version bumped when it changes;
    /// each ring slot re-uploads it lazily when its copy is stale.
    metas: Vec<ColumnMeta>,
    meta_version: u64,
    meta_versions: [u64; CULL_RING],
    meta_bufs: [vk::Buffer; CULL_RING],
    pub(super) meta_allocs: [Option<Allocation>; CULL_RING],
    pub(super) column_count: u32,
    meta_warned: bool,
    // -- cull outputs --
    pub(super) indirect_bufs: [vk::Buffer; CULL_RING],
    pub(super) indirect_allocs: [Option<Allocation>; CULL_RING],
    pub(super) count_bufs: [vk::Buffer; CULL_RING],
    pub(super) count_allocs: [Option<Allocation>; CULL_RING],
    /// The ring slot the current frame's cull wrote and its draw reads.
    pub(super) cursor: usize,
    pub(super) readback: vk::Buffer,
    pub(super) readback_alloc: Option<Allocation>,
}

/// The cull compute pipeline and its layouts, as created by
/// [`create_cull_pipeline`].
pub(super) struct CullPipeline {
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

pub(super) fn create_cull_pipeline(device: &ash::Device) -> Result<CullPipeline, String> {
    unsafe {
        let cull_bindings = [storage_binding(0), storage_binding(1), storage_binding(2)];
        let set_layout = device
            .create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&cull_bindings),
                None,
            )
            .map_err(|e| format!("cull set layout: {e}"))?;
        let cull_pc = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(std::mem::size_of::<CullPush>() as u32)];
        let cull_layouts = [set_layout];
        let layout = device
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&cull_layouts)
                    .push_constant_ranges(&cull_pc),
                None,
            )
            .map_err(|e| format!("cull layout: {e}"))?;
        let cull_module = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/cull.comp.spv")),
        )?;
        let pipeline = device
            .create_compute_pipelines(
                vk::PipelineCache::null(),
                &[vk::ComputePipelineCreateInfo::default()
                    .stage(
                        vk::PipelineShaderStageCreateInfo::default()
                            .stage(vk::ShaderStageFlags::COMPUTE)
                            .module(cull_module)
                            .name(c"main"),
                    )
                    .layout(layout)],
                None,
            )
            .map_err(|(_, e)| format!("cull pipeline: {e}"))?[0];
        device.destroy_shader_module(cull_module, None);
        Ok(CullPipeline {
            set_layout,
            layout,
            pipeline,
        })
    }
}

/// The metadata / indirect-command / draw-count buffer rings, as created by
/// [`create_cull_buffers`].
pub(super) struct CullBuffers {
    meta_bufs: [vk::Buffer; CULL_RING],
    meta_allocs: [Option<Allocation>; CULL_RING],
    indirect_bufs: [vk::Buffer; CULL_RING],
    indirect_allocs: [Option<Allocation>; CULL_RING],
    count_bufs: [vk::Buffer; CULL_RING],
    count_allocs: [Option<Allocation>; CULL_RING],
}

pub(super) fn create_cull_buffers(gpu: &mut Gpu) -> Result<CullBuffers, String> {
    let mut meta_bufs = [vk::Buffer::null(); CULL_RING];
    let mut meta_allocs: [Option<Allocation>; CULL_RING] = Default::default();
    let mut indirect_bufs = [vk::Buffer::null(); CULL_RING];
    let mut indirect_allocs: [Option<Allocation>; CULL_RING] = Default::default();
    let mut count_bufs = [vk::Buffer::null(); CULL_RING];
    let mut count_allocs: [Option<Allocation>; CULL_RING] = Default::default();
    for i in 0..CULL_RING {
        let (b, a) = create_host_buffer(
            gpu,
            (MAX_COLUMNS * std::mem::size_of::<ColumnMeta>()) as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            "column-meta",
        )?;
        gpu.name(b, &format!("column-meta[{i}]"));
        (meta_bufs[i], meta_allocs[i]) = (b, Some(a));
        let (b, a) = create_device_buffer(
            gpu,
            (MAX_COLUMNS * std::mem::size_of::<vk::DrawIndexedIndirectCommand>()) as u64,
            vk::BufferUsageFlags::INDIRECT_BUFFER | vk::BufferUsageFlags::STORAGE_BUFFER,
            "indirect-cmds",
        )?;
        gpu.name(b, &format!("indirect-cmds[{i}]"));
        (indirect_bufs[i], indirect_allocs[i]) = (b, Some(a));
        let (b, a) = create_device_buffer(
            gpu,
            16,
            vk::BufferUsageFlags::INDIRECT_BUFFER
                | vk::BufferUsageFlags::STORAGE_BUFFER
                | vk::BufferUsageFlags::TRANSFER_DST
                | vk::BufferUsageFlags::TRANSFER_SRC, // readback for stats
            "indirect-count",
        )?;
        gpu.name(b, &format!("indirect-count[{i}]"));
        (count_bufs[i], count_allocs[i]) = (b, Some(a));
    }
    Ok(CullBuffers {
        meta_bufs,
        meta_allocs,
        indirect_bufs,
        indirect_allocs,
        count_bufs,
        count_allocs,
    })
}

/// The cull descriptor pool and its per-ring-slot sets (meta, indirect cmds,
/// count), as created by [`create_cull_descriptors`].
pub(super) struct CullDescriptors {
    pool: vk::DescriptorPool,
    sets: [vk::DescriptorSet; CULL_RING],
}

pub(super) fn create_cull_descriptors(
    device: &ash::Device,
    pipeline: &CullPipeline,
    bufs: &CullBuffers,
) -> Result<CullDescriptors, String> {
    unsafe {
        let cull_pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(3 * CULL_RING as u32)];
        let pool = device
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(CULL_RING as u32)
                    .pool_sizes(&cull_pool_sizes),
                None,
            )
            .map_err(|e| format!("cull pool: {e}"))?;
        let cull_ring_layouts = [pipeline.set_layout; CULL_RING];
        let cull_set_vec = device
            .allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&cull_ring_layouts),
            )
            .map_err(|e| format!("cull set: {e}"))?;
        let mut sets = [vk::DescriptorSet::null(); CULL_RING];
        for i in 0..CULL_RING {
            sets[i] = cull_set_vec[i];
            write_storage(device, sets[i], 0, bufs.meta_bufs[i]);
            write_storage(device, sets[i], 1, bufs.indirect_bufs[i]);
            write_storage(device, sets[i], 2, bufs.count_bufs[i]);
        }
        Ok(CullDescriptors { pool, sets })
    }
}

impl Culler {
    pub(super) fn new(
        pipeline: CullPipeline,
        bufs: CullBuffers,
        descriptors: CullDescriptors,
        readback: (vk::Buffer, Allocation),
    ) -> Self {
        Self {
            set_layout: pipeline.set_layout,
            layout: pipeline.layout,
            pipeline: pipeline.pipeline,
            pool: descriptors.pool,
            sets: descriptors.sets,
            metas: Vec::new(),
            meta_version: 0,
            meta_versions: [0; CULL_RING],
            meta_bufs: bufs.meta_bufs,
            meta_allocs: bufs.meta_allocs,
            column_count: 0,
            meta_warned: false,
            indirect_bufs: bufs.indirect_bufs,
            indirect_allocs: bufs.indirect_allocs,
            count_bufs: bufs.count_bufs,
            count_allocs: bufs.count_allocs,
            cursor: 0,
            readback: readback.0,
            readback_alloc: Some(readback.1),
        }
    }

    /// Destroy the pipeline, layouts, descriptor pool and the three buffer
    /// rings (the allocations are taken back separately, by `destroy`).
    pub(super) fn destroy_objects(&self, device: &ash::Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            for b in self
                .meta_bufs
                .iter()
                .chain(&self.indirect_bufs)
                .chain(&self.count_bufs)
            {
                device.destroy_buffer(*b, None);
            }
            device.destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}

impl Culler {
    /// Rebuild the compacted metadata array (only when columns changed) and
    /// map-write it into the host-visible SSBO.
    fn sync_meta(&mut self, slot: usize, arena: &mut Arena, eye: [f32; 3]) {
        if arena.meta_dirty {
            if arena.columns.len() > MAX_COLUMNS && !self.meta_warned {
                self.meta_warned = true;
                log::warn!(
                    "world: {} columns exceed MAX_COLUMNS {MAX_COLUMNS} — the farthest are not drawn",
                    arena.columns.len()
                );
            }
            self.metas = select_metas(
                arena.columns.iter().map(|(k, s)| (*k, s.meta)),
                eye,
                MAX_COLUMNS,
            );
            self.column_count = self.metas.len() as u32;
            self.meta_version += 1;
            arena.meta_dirty = false;
        }
        // Each slot's copy is written only when this frame uses it, i.e.
        // after the fence that retired its previous reader.
        if self.meta_versions[slot] != self.meta_version {
            if let Some(slice) = self.meta_allocs[slot]
                .as_mut()
                .and_then(|a| a.mapped_slice_mut())
            {
                let bytes: &[u8] = bytemuck::cast_slice(&self.metas);
                slice[..bytes.len()].copy_from_slice(bytes);
            }
            self.meta_versions[slot] = self.meta_version;
        }
    }
}

impl WorldRenderer {
    /// Pre-pass (OUTSIDE dynamic rendering): reset the draw count, dispatch
    /// the compute cull, and barrier its writes for the indirect draw.
    pub fn cull(&mut self, gpu: &Gpu, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4]) {
        if let Err(e) = self.arena.flush_uploads(gpu) {
            log::error!("world: upload flush failed: {e}");
        }
        self.culler.cursor = (self.culler.cursor + 1) % CULL_RING;
        let slot = self.culler.cursor;
        self.culler
            .sync_meta(slot, &mut self.arena, self.scene.camera_eye);
        let count_buf = self.culler.count_bufs[slot];
        let indirect_buf = self.culler.indirect_bufs[slot];
        let device = &gpu.device;
        unsafe {
            // Reset the atomic count to 0.
            device.cmd_fill_buffer(cb, count_buf, 0, 4, 0);
            let reset_barrier = vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(count_buf)
                .offset(0)
                .size(vk::WHOLE_SIZE);
            device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default()
                    .buffer_memory_barriers(std::slice::from_ref(&reset_barrier)),
            );

            if self.culler.column_count > 0 {
                device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::COMPUTE, self.culler.pipeline);
                device.cmd_bind_descriptor_sets(
                    cb,
                    vk::PipelineBindPoint::COMPUTE,
                    self.culler.layout,
                    0,
                    &[self.culler.sets[slot]],
                    &[],
                );
                let push = CullPush {
                    planes: frustum_planes(&view_proj),
                    column_count: self.culler.column_count,
                    _pad: [0; 3],
                };
                device.cmd_push_constants(
                    cb,
                    self.culler.layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    bytemuck::bytes_of(&push),
                );
                let groups = self.culler.column_count.div_ceil(64);
                device.cmd_dispatch(cb, groups, 1, 1);

                // Compute writes → indirect draw reads (cmds + count).
                let barriers = [
                    buf_barrier(
                        indirect_buf,
                        vk::PipelineStageFlags2::COMPUTE_SHADER,
                        vk::AccessFlags2::SHADER_STORAGE_WRITE,
                        vk::PipelineStageFlags2::DRAW_INDIRECT,
                        vk::AccessFlags2::INDIRECT_COMMAND_READ,
                    ),
                    buf_barrier(
                        count_buf,
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
        self.drawn_last_frame = self.culler.column_count as usize; // pre-cull estimate
        self.culled_last_frame = 0;
    }

    /// Read back the GPU cull's draw count from the last frame (one-shot
    /// copy + a blocking wait — for verification/stats, not per-frame use).
    pub fn read_draw_count(&mut self, gpu: &mut Gpu) -> u32 {
        if self.arena.flush_uploads(gpu).is_err() {
            return 0;
        }
        let Ok(slot) = self.arena.acquire_slot(gpu, 0) else {
            return 0;
        };
        let Ok(cb) = self.arena.begin_slot(gpu, slot) else {
            return 0;
        };
        let count_buf = self.culler.count_bufs[self.culler.cursor];
        unsafe {
            // The last frame's cull wrote (and its draw read) the count.
            let before = buf_barrier(
                count_buf,
                vk::PipelineStageFlags2::COMPUTE_SHADER | vk::PipelineStageFlags2::DRAW_INDIRECT,
                vk::AccessFlags2::SHADER_STORAGE_WRITE,
                vk::PipelineStageFlags2::TRANSFER,
                vk::AccessFlags2::TRANSFER_READ,
            );
            gpu.device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default()
                    .buffer_memory_barriers(std::slice::from_ref(&before)),
            );
            let after = vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                .dst_access_mask(vk::AccessFlags2::HOST_READ);
            gpu.device.cmd_copy_buffer(
                cb,
                count_buf,
                self.culler.readback,
                &[vk::BufferCopy::default().size(4)],
            );
            gpu.device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default().memory_barriers(std::slice::from_ref(&after)),
            );
        }
        if self.arena.submit_slot(gpu, slot).is_err() {
            return 0;
        }
        unsafe {
            let fence = self.arena.upload_slots[slot].fence;
            let _ = gpu.device.wait_for_fences(&[fence], true, u64::MAX);
            let _ = gpu.device.reset_fences(&[fence]);
            self.arena.upload_slots[slot].busy = false;
            self.culler
                .readback_alloc
                .as_ref()
                .and_then(|a| a.mapped_slice())
                .map(|s| u32::from_ne_bytes(s[..4].try_into().unwrap()))
                .unwrap_or(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_at(cx: i32, cz: i32) -> ((i32, i32), ColumnMeta) {
        let m = ColumnMeta {
            aabb_min: [cx as f32 * 16.0, 0.0, cz as f32 * 16.0, 0.0],
            ..Default::default()
        };
        ((cx, cz), m)
    }

    /// Over the cap, the kept columns are the nearest to the eye whatever
    /// order the map yields them in; under it, nothing is dropped or moved.
    #[test]
    fn select_metas_truncates_nearest_first_and_deterministically() {
        let keys: Vec<(i32, i32)> = (-4..=4)
            .flat_map(|x| (-4..=4).map(move |z| (x, z)))
            .collect();
        let eye = [8.0, 70.0, 8.0]; // centre of column (0, 0)
        let kept = |order: &[(i32, i32)]| -> Vec<[f32; 4]> {
            select_metas(order.iter().map(|&(x, z)| meta_at(x, z)), eye, 5)
                .iter()
                .map(|m| m.aabb_min)
                .collect()
        };
        let forward = kept(&keys);
        let mut reversed = keys.clone();
        reversed.reverse();
        assert_eq!(forward, kept(&reversed), "input order must not matter");
        let expect: Vec<[f32; 4]> = [(0, 0), (-1, 0), (0, -1), (0, 1), (1, 0)]
            .iter()
            .map(|&(x, z)| meta_at(x, z).1.aabb_min)
            .collect();
        assert_eq!(forward, expect, "the centre plus its four edge neighbours");

        let few = [(3, 3), (-2, 1), (0, 0)];
        let all = select_metas(few.iter().map(|&(x, z)| meta_at(x, z)), eye, 5);
        let got: Vec<[f32; 4]> = all.iter().map(|m| m.aabb_min).collect();
        let want: Vec<[f32; 4]> = few.iter().map(|&(x, z)| meta_at(x, z).1.aabb_min).collect();
        assert_eq!(got, want, "under the cap the order is untouched");
    }
}
