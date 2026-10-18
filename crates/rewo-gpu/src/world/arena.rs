//! The terrain arena: the two device-local mega-buffers (vertices + indices)
//! every chunk column is suballocated from, the free-lists over them, and the
//! async upload ring that fills them.
//!
//! Column uploads (rare — only on chunk load / block edit) are staged into a
//! batch recorded on a ring slot's own command buffer and submitted on the
//! graphics queue **without** a CPU fence wait: queue FIFO order guarantees
//! every copy executes before any later-submitted frame that could read it.
//! The CPU only waits when every slot is still in flight (a sustained burst)
//! or to grow a staging buffer.
//!
//! [`Arena`] owns that state; [`WorldRenderer`] keeps the public entry points
//! (`upload_column`, `remove_column`, `retry_deferred_columns`, …) as thin
//! calls into it.

use std::collections::{BTreeMap, HashMap};

use ash::vk;
use gpu_allocator::vulkan::Allocation;

use super::cull::ColumnMeta;
use super::resources::{create_device_buffer, create_host_buffer};
use super::WorldRenderer;
use crate::Gpu;

// -- Arena capacity facts (public so tooling can report utilization without
//    duplicating these numbers; see `rewo bench --mesh-runs`). Changing a value
//    here changes the allocator, not just the report. --------------------------

/// Bytes per uploaded vertex. Must equal `size_of::<rewo_mesh::MeshVertex>()`;
/// `rewo bench` asserts the two agree, since a silent mismatch would corrupt
/// every upload offset. M15 packed layout: pos f32x3 (0), uv f32x2 (12),
/// light u32 (20), color RGBA8 (24). UV stayed f32 because fluid surface UVs
/// (`1 - k/9`) are not representable in f16 — see `rewo_mesh::MeshVertex`.
pub const VERTEX_STRIDE: u64 = 28;

/// Mega-buffer capacities. 4M verts (4M × 28 B = 112 MB) + 6M indices
/// (6M × 4 B = 24 MB) covers a large render distance; over-cap columns are
/// dropped with a log.
///
/// **Opaque and translucent geometry share these pools** — both suballocate
/// from the same vertex and index free lists — so any utilization figure must
/// sum opaque + translucent, not report the opaque set alone.
pub const MAX_VERTS: u64 = 4_000_000;

/// Shared index-arena capacity (see [`MAX_VERTS`] — same sharing rule).
pub const MAX_INDICES: u64 = 6_000_000;

/// Per-column metadata slots (the third capacity pool: the cull/draw SSBO).
pub const MAX_COLUMNS: usize = 8192;

pub(super) struct Slot {
    pub(super) vtx_off: u64, // in vertices
    pub(super) vtx_len: u64,
    pub(super) idx_off: u64, // in indices
    pub(super) idx_len: u64,
    // Translucent (water) region — zero-length when the column has none.
    pub(super) tvtx_off: u64,
    pub(super) tvtx_len: u64,
    pub(super) tidx_off: u64,
    pub(super) tidx_len: u64,
    pub(super) meta: ColumnMeta,
}

/// One in-flight upload: its own command buffer, fence, and staging
/// buffer (untouched until the fence says the copy retired).
pub(super) struct UploadSlot {
    pub(super) cb: vk::CommandBuffer,
    pub(super) fence: vk::Fence,
    pub(super) staging: vk::Buffer,
    pub(super) staging_alloc: Option<Allocation>,
    pub(super) staging_cap: u64,
    pub(super) busy: bool,
    /// Submission order, so a full ring waits on the *oldest* slot.
    pub(super) seq: u64,
    /// Bytes of staging this slot's open batch has used.
    pub(super) used: u64,
}

const UPLOAD_SLOTS: usize = 4;

/// Staging a batch opens with, so a burst of small column uploads shares one
/// command buffer and one submit instead of one each.
const UPLOAD_BATCH_MIN: u64 = 2 * 1024 * 1024;

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

/// Element counts of a column's four arena regions: opaque vertices, opaque
/// indices, translucent vertices, translucent indices.
type RegionCounts = [u64; 4];

/// Allocate a column's four regions from the shared vertex / index
/// free-lists, all or nothing: a partial success is rolled back before
/// returning `None`, so a full arena never leaks the regions that did fit.
fn alloc_regions(vfree: &mut FreeList, ifree: &mut FreeList, n: RegionCounts) -> Option<[u64; 4]> {
    let v = vfree.alloc(n[0])?;
    let Some(i) = ifree.alloc(n[1]) else {
        vfree.free_region(v, n[0]);
        return None;
    };
    let Some(tv) = vfree.alloc(n[2]) else {
        vfree.free_region(v, n[0]);
        ifree.free_region(i, n[1]);
        return None;
    };
    let Some(ti) = ifree.alloc(n[3]) else {
        vfree.free_region(v, n[0]);
        ifree.free_region(i, n[1]);
        vfree.free_region(tv, n[2]);
        return None;
    };
    Some([v, i, tv, ti])
}

/// Return regions from [`alloc_regions`] that no command ever referenced.
fn free_regions(vfree: &mut FreeList, ifree: &mut FreeList, off: [u64; 4], n: RegionCounts) {
    vfree.free_region(off[0], n[0]);
    ifree.free_region(off[1], n[1]);
    vfree.free_region(off[2], n[2]);
    ifree.free_region(off[3], n[3]);
}

/// Squared XZ distance from `eye` to a column's centre.
pub(super) fn column_dist2((cx, cz): (i32, i32), eye: [f32; 3]) -> f64 {
    let dx = (cx as f64 * 16.0 + 8.0) - eye[0] as f64;
    let dz = (cz as f64 * 16.0 + 8.0) - eye[2] as f64;
    dx * dx + dz * dz
}

/// Order column keys nearest to `eye` first, ties by coordinate.
fn sort_nearest_first(keys: &mut [(i32, i32)], eye: [f32; 3]) {
    keys.sort_by(|a, b| {
        column_dist2(*a, eye)
            .total_cmp(&column_dist2(*b, eye))
            .then(a.cmp(b))
    });
}

/// A column whose geometry did not fit the mega-buffers, held (owned) until
/// arena space frees, then re-uploaded by
/// [`WorldRenderer::retry_deferred_columns`].
struct DeferredColumn {
    vertex_bytes: Vec<u8>,
    indices: Vec<u32>,
    tvertex_bytes: Vec<u8>,
    tindices: Vec<u32>,
    y_min: f32,
    y_max: f32,
}

/// The mega-buffers, the free-lists over them, the registered columns and the
/// async upload ring — everything the terrain's geometry storage needs.
pub(super) struct Arena {
    pub(super) vbuf: vk::Buffer,
    pub(super) vbuf_alloc: Option<Allocation>,
    vfree: FreeList,
    pub(super) ibuf: vk::Buffer,
    pub(super) ibuf_alloc: Option<Allocation>,
    ifree: FreeList,
    // -- columns --
    pub(super) columns: HashMap<(i32, i32), Slot>,
    /// `columns` changed since the cull metadata was last rebuilt.
    pub(super) meta_dirty: bool,
    /// Arena ranges freed by `remove_column`, held until every frame that could
    /// still read them has retired (tag = the `Gpu` frame serial that retires
    /// them). Releasing immediately let a re-upload overwrite geometry an
    /// in-flight frame was drawing.
    pending_free: Vec<(u64, [(u64, u64); 2], [(u64, u64); 2])>,
    /// Columns that did not fit the arena, re-uploaded when space frees
    /// (rather than silently never drawn). Keyed by coordinate, so a newer
    /// mesh of the same column replaces the waiting one.
    deferred: BTreeMap<(i32, i32), DeferredColumn>,
    /// Arena ranges were released since the last retry of `deferred`.
    space_freed: bool,
    /// Columns registered whose copies are in the open, unsubmitted upload
    /// batch. If that batch fails to submit, their arena data was never
    /// written, so they are unregistered and reported via `lost_columns`.
    batch_columns: Vec<(i32, i32)>,
    /// Columns unregistered by a failed upload submit; the caller re-meshes
    /// them ([`WorldRenderer::take_lost_columns`]).
    lost_columns: Vec<(i32, i32)>,
    // -- async upload ring --
    // Uploads submit on the graphics queue WITHOUT a CPU fence wait: queue
    // FIFO ordering guarantees every copy executes before any later-
    // submitted frame that could read it (the same guarantee the old
    // blocking path relied on — minus the stall). The CPU only waits when
    // all slots are still in flight (sustained burst) or to grow staging.
    pub(super) upload_pool: vk::CommandPool,
    pub(super) upload_slots: Vec<UploadSlot>,
    /// The slot whose command buffer is recording this frame's uploads, if
    /// any. Submitted by [`Self::flush_uploads`] (from `cull`, i.e. before the
    /// frame that reads it).
    upload_open: Option<usize>,
    upload_seq: u64,
}

/// The two device-local mega-buffers and their allocations, as created by
/// [`create_mega_buffers`].
pub(super) struct MegaBuffers {
    vbuf: vk::Buffer,
    vbuf_alloc: Allocation,
    ibuf: vk::Buffer,
    ibuf_alloc: Allocation,
}

pub(super) fn create_mega_buffers(gpu: &mut Gpu) -> Result<MegaBuffers, String> {
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
    Ok(MegaBuffers {
        vbuf,
        vbuf_alloc,
        ibuf,
        ibuf_alloc,
    })
}

/// The upload ring's command pool and its slots (command buffer, fence and
/// staging buffer each), as created by [`create_upload_ring`].
pub(super) struct UploadRing {
    pool: vk::CommandPool,
    slots: Vec<UploadSlot>,
}

pub(super) fn create_upload_ring(gpu: &mut Gpu) -> Result<UploadRing, String> {
    unsafe {
        let device = gpu.device.clone();
        // RESET_COMMAND_BUFFER so each slot's cb re-begins on its own
        // while its siblings stay in flight.
        let pool = device
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
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(UPLOAD_SLOTS as u32),
            )
            .map_err(|e| format!("upload cbs: {e}"))?;
        let mut slots = Vec::with_capacity(UPLOAD_SLOTS);
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
            slots.push(UploadSlot {
                cb,
                fence,
                staging,
                staging_alloc: Some(staging_alloc),
                staging_cap,
                busy: false,
                seq: 0,
                used: 0,
            });
        }
        Ok(UploadRing { pool, slots })
    }
}

impl Arena {
    pub(super) fn new(mega: MegaBuffers, ring: UploadRing) -> Self {
        Self {
            vbuf: mega.vbuf,
            vbuf_alloc: Some(mega.vbuf_alloc),
            vfree: FreeList::new(MAX_VERTS),
            ibuf: mega.ibuf,
            ibuf_alloc: Some(mega.ibuf_alloc),
            ifree: FreeList::new(MAX_INDICES),
            columns: HashMap::new(),
            meta_dirty: false,
            pending_free: Vec::new(),
            deferred: BTreeMap::new(),
            space_freed: false,
            batch_columns: Vec::new(),
            lost_columns: Vec::new(),
            upload_pool: ring.pool,
            upload_slots: ring.slots,
            upload_open: None,
            upload_seq: 0,
        }
    }
}

impl Arena {
    /// Find (or wait for) a free upload slot with at least `need` staging
    /// bytes. Retired fences are harvested opportunistically; the CPU only
    /// blocks when every slot is still in flight (sustained burst).
    pub(super) fn acquire_slot(&mut self, gpu: &mut Gpu, need: u64) -> Result<usize, String> {
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
                    // Every slot in flight: wait on the one submitted first,
                    // which is the one most likely already done.
                    let oldest = (0..self.upload_slots.len())
                        .min_by_key(|&i| self.upload_slots[i].seq)
                        .unwrap_or(0);
                    let fence = self.upload_slots[oldest].fence;
                    gpu.device
                        .wait_for_fences(&[fence], true, u64::MAX)
                        .map_err(|e| format!("slot wait: {e}"))?;
                    gpu.device
                        .reset_fences(&[fence])
                        .map_err(|e| format!("slot reset: {e}"))?;
                    self.upload_slots[oldest].busy = false;
                    oldest
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
    pub(super) fn begin_slot(&self, gpu: &Gpu, idx: usize) -> Result<vk::CommandBuffer, String> {
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
    pub(super) fn submit_slot(&mut self, gpu: &Gpu, idx: usize) -> Result<(), String> {
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
        self.upload_seq += 1;
        s.seq = self.upload_seq;
        Ok(())
    }

    /// Reserve `need` bytes of staging in this frame's open upload batch,
    /// opening one if there is none or it is full. Returns the slot, the byte
    /// offset to write at, and the recording command buffer.
    pub(super) fn stage(
        &mut self,
        gpu: &mut Gpu,
        need: u64,
    ) -> Result<(usize, u64, vk::CommandBuffer), String> {
        // `bufferOffset` of a buffer→image copy must be a multiple of 4; 16
        // keeps every staged span aligned for either copy kind.
        let need_aligned = need.div_ceil(16) * 16;
        if let Some(slot) = self.upload_open {
            let s = &mut self.upload_slots[slot];
            if s.used + need_aligned <= s.staging_cap {
                let off = s.used;
                s.used += need_aligned;
                return Ok((slot, off, s.cb));
            }
            self.flush_uploads(gpu)?;
        }
        let slot = self.acquire_slot(gpu, need_aligned.max(UPLOAD_BATCH_MIN))?;
        let cb = self.begin_slot(gpu, slot)?;
        unsafe {
            // Order this batch's writes after every earlier frame's reads of
            // the arena and the texture array (queue order alone is not a
            // memory dependency).
            let war = vk::MemoryBarrier2::default()
                .src_stage_mask(
                    vk::PipelineStageFlags2::VERTEX_INPUT
                        | vk::PipelineStageFlags2::INDEX_INPUT
                        | vk::PipelineStageFlags2::FRAGMENT_SHADER,
                )
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER);
            gpu.device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default().memory_barriers(std::slice::from_ref(&war)),
            );
        }
        self.upload_slots[slot].used = need_aligned;
        self.upload_open = Some(slot);
        Ok((slot, 0, cb))
    }

    /// Submit this frame's open upload batch, if any. Called from `cull`, so
    /// the batch is on the queue ahead of the frame that draws from it.
    pub(super) fn flush_uploads(&mut self, gpu: &Gpu) -> Result<(), String> {
        let Some(slot) = self.upload_open.take() else {
            return Ok(());
        };
        let cb = self.upload_slots[slot].cb;
        unsafe {
            // Make the copies visible to later frames' vertex/index reads.
            let vis = vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(
                    vk::PipelineStageFlags2::VERTEX_INPUT | vk::PipelineStageFlags2::INDEX_INPUT,
                )
                .dst_access_mask(
                    vk::AccessFlags2::VERTEX_ATTRIBUTE_READ | vk::AccessFlags2::INDEX_READ,
                );
            gpu.device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default().memory_barriers(std::slice::from_ref(&vis)),
            );
        }
        let submitted = self.submit_slot(gpu, slot);
        let staged = std::mem::take(&mut self.batch_columns);
        if submitted.is_err() {
            // The batch never reached the queue, so every column registered
            // with copies in it points at arena data that was never written.
            // Unregister them (no submitted frame has drawn them: metadata is
            // synced only after this flush) and hand them back for re-mesh.
            for key in staged {
                if self.columns.contains_key(&key) {
                    self.unregister_column(gpu, key);
                    self.lost_columns.push(key);
                }
            }
        }
        submitted
    }

    /// Release arena ranges whose last possible reader has retired.
    fn reclaim_ranges(&mut self, gpu: &Gpu) {
        let done = gpu.frames_retired();
        let (vfree, ifree) = (&mut self.vfree, &mut self.ifree);
        let mut freed = false;
        self.pending_free.retain(|(tag, vr, ir)| {
            if *tag > done {
                return true;
            }
            for &(o, n) in vr {
                vfree.free_region(o, n);
                freed |= n > 0;
            }
            for &(o, n) in ir {
                ifree.free_region(o, n);
                freed |= n > 0;
            }
            false
        });
        self.space_freed |= freed;
    }

    /// Suballocate + upload a column's geometry into the mega-buffers via
    /// an async staged copy (slot ring — the CPU does not wait for the
    /// copy). Opaque and translucent (water) sets ride the same buffers as
    /// two regions; either may be empty.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn upload_column(
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
        self.reclaim_ranges(gpu);
        gpu.collect_garbage();
        let placed = self.place_column(
            gpu,
            cx,
            cz,
            vertex_bytes,
            indices,
            tvertex_bytes,
            tindices,
            y_min,
            y_max,
        )?;
        if !placed {
            // Keep it rather than drop it: it is re-uploaded by
            // `retry_deferred_columns` once removals free arena space.
            if self.deferred.is_empty() {
                log::warn!(
                    "world: mega-buffer full — deferring column ({cx},{cz}) until space frees"
                );
            }
            self.deferred.insert(
                (cx, cz),
                DeferredColumn {
                    vertex_bytes: vertex_bytes.to_vec(),
                    indices: indices.to_vec(),
                    tvertex_bytes: tvertex_bytes.to_vec(),
                    tindices: tindices.to_vec(),
                    y_min,
                    y_max,
                },
            );
        }
        Ok(())
    }

    /// Allocate arena space for a column (assumed not registered), stage its
    /// copies into the open upload batch, and register it. `Ok(false)` is "the
    /// arena is full" with nothing changed; an `Err` rolls back the
    /// allocation, so neither path leaks arena space.
    #[allow(clippy::too_many_arguments)]
    fn place_column(
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
    ) -> Result<bool, String> {
        let vtx_count = (vertex_bytes.len() as u64) / VERTEX_STRIDE;
        let idx_count = indices.len() as u64;
        let tvtx_count = (tvertex_bytes.len() as u64) / VERTEX_STRIDE;
        let tidx_count = tindices.len() as u64;
        if idx_count == 0 && tidx_count == 0 {
            return Ok(true);
        }
        // Four regions from the two shared free-lists (zero-length allocs
        // are free), all or nothing.
        let counts = [vtx_count, idx_count, tvtx_count, tidx_count];
        let Some(offs) = alloc_regions(&mut self.vfree, &mut self.ifree, counts) else {
            return Ok(false);
        };
        let [vtx_off, idx_off, tvtx_off, tidx_off] = offs;

        // Stage all four spans contiguously, copy each non-empty one.
        let index_bytes: &[u8] = bytemuck::cast_slice(indices);
        let tindex_bytes: &[u8] = bytemuck::cast_slice(tindices);
        let spans = [
            (vertex_bytes, self.vbuf, vtx_off * VERTEX_STRIDE),
            (index_bytes, self.ibuf, idx_off * 4),
            (tvertex_bytes, self.vbuf, tvtx_off * VERTEX_STRIDE),
            (tindex_bytes, self.ibuf, tidx_off * 4),
        ];
        if let Err(e) = self.record_copies(gpu, &spans) {
            // No copy for these regions was recorded, so nothing can read
            // them: return them now rather than leak them.
            free_regions(&mut self.vfree, &mut self.ifree, offs, counts);
            self.space_freed = true;
            return Err(e);
        }

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
        self.batch_columns.push((cx, cz));
        self.meta_dirty = true;
        Ok(true)
    }

    /// Stage `spans` (bytes, destination buffer, destination byte offset)
    /// contiguously into the open upload batch and record one copy per
    /// non-empty span.
    fn record_copies(
        &mut self,
        gpu: &mut Gpu,
        spans: &[(&[u8], vk::Buffer, u64); 4],
    ) -> Result<(), String> {
        let total: usize = spans.iter().map(|(b, _, _)| b.len()).sum();
        let (slot, base, cb) = self.stage(gpu, total as u64)?;
        unsafe {
            let alloc = self.upload_slots[slot]
                .staging_alloc
                .as_mut()
                .ok_or("staging not allocated")?;
            let slice = alloc.mapped_slice_mut().ok_or("staging not mapped")?;
            let mut off = base as usize;
            for (bytes, _, _) in spans {
                slice[off..off + bytes.len()].copy_from_slice(bytes);
                off += bytes.len();
            }
            let staging = self.upload_slots[slot].staging;
            let mut src = base;
            for (bytes, dst_buf, dst_off) in spans {
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
        }
        Ok(())
    }

    pub(super) fn remove_column(&mut self, gpu: &mut Gpu, cx: i32, cz: i32) {
        // A column waiting for arena space is superseded too: the caller is
        // either forgetting it or about to upload a newer mesh.
        self.deferred.remove(&(cx, cz));
        self.unregister_column(gpu, (cx, cz));
    }

    /// Drop a registered column, parking its arena ranges until every frame
    /// that could still draw them has retired.
    fn unregister_column(&mut self, gpu: &Gpu, key: (i32, i32)) {
        if let Some(slot) = self.columns.remove(&key) {
            // Frames already submitted (and the one being built) may still
            // draw these ranges; park them until those frames retire.
            let tag = gpu.frame_serial() + 1;
            self.pending_free.push((
                tag,
                [(slot.vtx_off, slot.vtx_len), (slot.tvtx_off, slot.tvtx_len)],
                [(slot.idx_off, slot.idx_len), (slot.tidx_off, slot.tidx_len)],
            ));
            self.meta_dirty = true;
        }
    }

    /// Re-upload columns that did not fit the mega-buffers, nearest to the
    /// camera first, once arena space has been released since the last try.
    /// Call once per frame; cheap when nothing waits. Returns how many landed.
    pub(super) fn retry_deferred_columns(
        &mut self,
        gpu: &mut Gpu,
        eye: [f32; 3],
    ) -> Result<usize, String> {
        self.reclaim_ranges(gpu);
        if self.deferred.is_empty() || !self.space_freed {
            return Ok(0);
        }
        self.space_freed = false;
        let mut keys: Vec<(i32, i32)> = self.deferred.keys().copied().collect();
        sort_nearest_first(&mut keys, eye);
        let mut landed = 0;
        for key in keys {
            let Some(d) = self.deferred.remove(&key) else {
                continue;
            };
            let placed = self.place_column(
                gpu,
                key.0,
                key.1,
                &d.vertex_bytes,
                &d.indices,
                &d.tvertex_bytes,
                &d.tindices,
                d.y_min,
                d.y_max,
            );
            match placed {
                Ok(true) => landed += 1,
                // Still no room for this one; a smaller one may fit.
                Ok(false) => {
                    self.deferred.insert(key, d);
                }
                Err(e) => {
                    self.deferred.insert(key, d);
                    return Err(e);
                }
            }
        }
        if landed > 0 {
            log::info!(
                "world: {landed} deferred column(s) uploaded, {} still waiting",
                self.deferred.len()
            );
        }
        Ok(landed)
    }
}

impl WorldRenderer {
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
        self.arena.upload_column(
            gpu,
            cx,
            cz,
            vertex_bytes,
            indices,
            tvertex_bytes,
            tindices,
            y_min,
            y_max,
        )
    }

    pub fn remove_column(&mut self, gpu: &mut Gpu, cx: i32, cz: i32) {
        self.arena.remove_column(gpu, cx, cz);
    }

    pub fn column_count(&self) -> usize {
        self.arena.columns.len()
    }

    /// Columns whose upload batch failed to submit since the last call. They
    /// are no longer drawn; the caller should mark them dirty to re-mesh.
    pub fn take_lost_columns(&mut self) -> Vec<(i32, i32)> {
        std::mem::take(&mut self.arena.lost_columns)
    }

    /// Columns waiting for arena space (see [`Self::retry_deferred_columns`]).
    pub fn deferred_column_count(&self) -> usize {
        self.arena.deferred.len()
    }

    /// The arena's fill: `(vertices, indices)` held by registered columns,
    /// opaque and translucent summed (they share both pools; see
    /// [`MAX_VERTS`]), against `(MAX_VERTS, MAX_INDICES)`.
    pub fn arena_usage(&self) -> ((u64, u64), (u64, u64)) {
        let (v, i) = self.arena.columns.values().fold((0, 0), |(v, i), s| {
            (v + s.vtx_len + s.tvtx_len, i + s.idx_len + s.tidx_len)
        });
        ((v, MAX_VERTS), (i, MAX_INDICES))
    }

    /// Re-upload columns that did not fit the mega-buffers, nearest to the
    /// camera first, once arena space has been released since the last try.
    /// Call once per frame; cheap when nothing waits. Returns how many landed.
    pub fn retry_deferred_columns(&mut self, gpu: &mut Gpu) -> Result<usize, String> {
        self.arena
            .retry_deferred_columns(gpu, self.scene.camera_eye)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failure on any of the four regions returns every region that did
    /// fit: the free-lists after a refused column are exactly what they were
    /// before it.
    #[test]
    fn alloc_regions_is_all_or_nothing() {
        for fail_at in 0..4 {
            let mut vfree = FreeList::new(100);
            let mut ifree = FreeList::new(100);
            let mut n: RegionCounts = [10, 20, 30, 40];
            n[fail_at] = 101; // only this region cannot fit
            assert_eq!(alloc_regions(&mut vfree, &mut ifree, n), None);
            assert_eq!(vfree.free, vec![(0, 100)], "fail at {fail_at}");
            assert_eq!(ifree.free, vec![(0, 100)], "fail at {fail_at}");
        }
        let mut vfree = FreeList::new(100);
        let mut ifree = FreeList::new(100);
        let n = [10, 20, 30, 40];
        let offs = alloc_regions(&mut vfree, &mut ifree, n).expect("fits");
        assert_eq!(offs, [0, 0, 10, 20]);
        free_regions(&mut vfree, &mut ifree, offs, n);
        assert_eq!((vfree.free, ifree.free), (vec![(0, 100)], vec![(0, 100)]));
    }
}
