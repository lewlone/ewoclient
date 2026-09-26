//! The one per-frame dynamic buffer ring every pass uses (vertex, index,
//! uniform and storage data rewritten each frame).
//!
//! Each of the [`BUF_RING`] slots is a **persistently mapped** host-visible
//! buffer that is kept and reused frame after frame; a frame only memcpys into
//! its slot. A slot is reallocated only when a frame needs more than its
//! capacity, and the outgoing buffer goes through the deferred-destruction
//! queue ([`crate::deferred`]) rather than being destroyed under an in-flight
//! frame — which is what M86's `free_buf(gpu, self.vbuf.take())` did 40,532
//! times (`VUID-vkDestroyBuffer-buffer-00922`).
//!
//! # Why `MAX_FRAMES_IN_FLIGHT + 1` slots
//!
//! A slot written *inside* `Renderer::render` (after that frame's fence wait)
//! is safe at `ring >= fif`; one written by a `set_*` in the app's frame loop
//! *before* `render` runs while the previous frame's fence is the most recent
//! wait, so `fif` frames may still be reading and it needs `ring >= fif + 1`.
//! A pass holds no reference to the `Renderer`, so the ring is sized for the
//! worst case the `--fif` knob permits and serves both kinds of writer.
//! [`ring_slot_is_retired`] states the rule.
//!
//! # Why the cursor advances on *use*, not on *call*
//!
//! The cursor moves only when the slot about to be overwritten was bound by a
//! draw. Several writes in one frame therefore reuse one slot (nothing has read
//! it yet), and a pass whose draw is skipped never burns a slot. The flag is a
//! `Cell` because `draw` takes `&self` throughout this crate.

use std::cell::Cell;

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::Gpu;

/// Slots in a [`BufRing`]: `MAX_FRAMES_IN_FLIGHT + 1` — see the module docs.
pub(crate) const BUF_RING: usize = crate::MAX_FRAMES_IN_FLIGHT + 1;

/// Smallest slot a growing ring allocates, so tiny per-frame writes do not
/// reallocate on every size change.
const MIN_CAPACITY: u64 = 4096;

/// Is the slot a pre-`render` writer is about to overwrite guaranteed to have
/// been released by the GPU? Pure, for the invariant test.
#[cfg(test)]
pub(crate) const fn ring_slot_is_retired(ring: usize, fif: usize) -> bool {
    ring > fif
}

struct Slot {
    buffer: vk::Buffer,
    alloc: Allocation,
    cap: u64,
    usage: vk::BufferUsageFlags,
}

/// One buffer's worth of per-frame data, rotated across [`BUF_RING`] slots.
pub(crate) struct BufRing {
    name: &'static str,
    slots: [Option<Slot>; BUF_RING],
    /// Bytes written into each slot by its latest write; `0` = draw nothing.
    lens: [u64; BUF_RING],
    cursor: usize,
    /// Whether `slots[cursor]` has been handed to a draw since it was written.
    bound: Cell<bool>,
    warned: Cell<bool>,
}

impl BufRing {
    pub(crate) fn new() -> Self {
        Self::named("buf-ring")
    }

    pub(crate) fn named(name: &'static str) -> Self {
        Self {
            name,
            slots: std::array::from_fn(|_| None),
            lens: [0; BUF_RING],
            cursor: 0,
            bound: Cell::new(false),
            warned: Cell::new(false),
        }
    }

    /// A ring whose every slot is pre-allocated at `cap` bytes, for passes that
    /// write from a `&Gpu` draw path and so cannot grow ([`Self::write_fixed`]).
    pub(crate) fn with_capacity(
        gpu: &mut Gpu,
        name: &'static str,
        usage: vk::BufferUsageFlags,
        cap: u64,
    ) -> Result<Self, String> {
        let mut ring = Self::named(name);
        for s in 0..BUF_RING {
            ring.slots[s] = Some(alloc_slot(gpu, name, s, usage, cap)?);
        }
        Ok(ring)
    }

    /// The slot [`Self::set`] would write next. Pure book-keeping.
    fn next_cursor(cursor: usize, bound: bool) -> usize {
        if bound {
            (cursor + 1) % BUF_RING
        } else {
            cursor
        }
    }

    fn advance(&mut self) {
        self.cursor = Self::next_cursor(self.cursor, self.bound.replace(false));
    }

    /// Replace this frame's contents, growing the slot if it is too small. An
    /// empty `bytes` means "draw nothing this frame".
    pub(crate) fn set(
        &mut self,
        gpu: &mut Gpu,
        bytes: &[u8],
        usage: vk::BufferUsageFlags,
    ) -> Result<(), String> {
        self.advance();
        let c = self.cursor;
        self.lens[c] = 0;
        if bytes.is_empty() {
            return Ok(());
        }
        let need = bytes.len() as u64;
        let fits = self.slots[c]
            .as_ref()
            .is_some_and(|s| s.cap >= need && s.usage.contains(usage));
        if !fits {
            if let Some(old) = self.slots[c].take() {
                gpu.defer_destroy_buffer(old.buffer, Some(old.alloc));
            }
            gpu.collect_garbage();
            let cap = need.next_power_of_two().max(MIN_CAPACITY);
            self.slots[c] = Some(alloc_slot(gpu, self.name, c, usage, cap)?);
        }
        let slot = self.slots[c].as_mut().expect("slot allocated above");
        let dst = slot
            .alloc
            .mapped_slice_mut()
            .ok_or_else(|| format!("{}: ring slot not host-mapped", self.name))?;
        dst[..bytes.len()].copy_from_slice(bytes);
        self.lens[c] = need;
        Ok(())
    }

    /// Replace this frame's contents without the ability to grow — for writers
    /// that only hold `&Gpu`. Data past the slot's capacity is dropped whole
    /// `stride` elements at a time (warned once). Returns the bytes kept.
    pub(crate) fn write_fixed(&mut self, bytes: &[u8], stride: usize) -> usize {
        self.advance();
        let c = self.cursor;
        self.lens[c] = 0;
        let Some(slot) = self.slots[c].as_mut() else {
            return 0;
        };
        let stride = stride.max(1);
        let cap = (slot.cap as usize / stride) * stride;
        let n = if bytes.len() > cap {
            if !self.warned.replace(true) {
                log::warn!(
                    "{}: {} bytes exceed the {} byte ring slot — truncating",
                    self.name,
                    bytes.len(),
                    cap
                );
            }
            cap
        } else {
            (bytes.len() / stride) * stride
        };
        if n == 0 {
            return 0;
        }
        let Some(dst) = slot.alloc.mapped_slice_mut() else {
            return 0;
        };
        dst[..n].copy_from_slice(&bytes[..n]);
        self.lens[c] = n as u64;
        n
    }

    /// Drop this frame's contents without uploading anything.
    pub(crate) fn clear(&mut self, gpu: &mut Gpu) {
        let _ = self.set(gpu, &[], vk::BufferUsageFlags::VERTEX_BUFFER);
    }

    /// The handle a draw should bind, recording that the slot is now in use.
    /// `None` when this frame wrote nothing. A path that returns early without
    /// binding must not call it.
    pub(crate) fn bind(&self) -> Option<vk::Buffer> {
        let h = self.peek()?;
        self.bound.set(true);
        Some(h)
    }

    /// The current handle without claiming it.
    pub(crate) fn peek(&self) -> Option<vk::Buffer> {
        if self.lens[self.cursor] == 0 {
            return None;
        }
        self.slots[self.cursor].as_ref().map(|s| s.buffer)
    }

    /// Bytes this frame wrote into the current slot.
    pub(crate) fn len(&self) -> u64 {
        self.lens[self.cursor]
    }

    /// Every buffer this ring keeps alive, as raw handles — the property
    /// `live --render-check` asserts is that a bound buffer is still alive
    /// several frames later.
    pub(crate) fn live(&self) -> Vec<u64> {
        use ash::vk::Handle;
        self.slots
            .iter()
            .filter_map(|s| s.as_ref().map(|b| b.buffer.as_raw()))
            .collect()
    }

    /// Which slot [`Self::peek`] and [`Self::bind`] currently answer with —
    /// for passes that ring descriptor sets on the same rotation.
    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    /// Destroy every slot. Teardown only: the caller has idled the device.
    pub(crate) fn destroy(&mut self, gpu: &mut Gpu) {
        for s in 0..BUF_RING {
            if let Some(slot) = self.slots[s].take() {
                unsafe { gpu.device.destroy_buffer(slot.buffer, None) };
                let _ = gpu.allocator.free(slot.alloc);
            }
            self.lens[s] = 0;
        }
        self.cursor = 0;
        self.bound.set(false);
    }
}

/// Warn the first time a pass drops geometry past its fixed budget — silent
/// truncation is how a missing HUD row goes unnoticed.
pub(crate) fn warn_truncated(flag: &std::sync::atomic::AtomicBool, what: &str, cap: usize) {
    if !flag.swap(true, std::sync::atomic::Ordering::Relaxed) {
        log::warn!("{what}: over the {cap}-element budget — truncating (reported once)");
    }
}

fn alloc_slot(
    gpu: &mut Gpu,
    name: &'static str,
    index: usize,
    usage: vk::BufferUsageFlags,
    cap: u64,
) -> Result<Slot, String> {
    unsafe {
        let buffer = gpu
            .device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(cap)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("{name} buffer: {e}"))?;
        let req = gpu.device.get_buffer_memory_requirements(buffer);
        let alloc = match gpu.allocator.allocate(&AllocationCreateDesc {
            name,
            requirements: req,
            location: MemoryLocation::CpuToGpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        }) {
            Ok(a) => a,
            Err(e) => {
                gpu.device.destroy_buffer(buffer, None);
                return Err(format!("{name} alloc: {e}"));
            }
        };
        gpu.device
            .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
            .map_err(|e| format!("{name} bind: {e}"))?;
        gpu.name(buffer, &format!("{name}[{index}]"));
        Ok(Slot {
            buffer,
            alloc,
            cap,
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_outlives_every_frame_in_flight() {
        for fif in 1..=crate::MAX_FRAMES_IN_FLIGHT {
            assert!(
                ring_slot_is_retired(BUF_RING, fif),
                "ring {BUF_RING} cannot serve frames-in-flight {fif}"
            );
        }
        // `MAX_FRAMES_IN_FLIGHT` alone is the in-`render` writer's rule and is
        // one short for a `set_*` writer.
        assert!(!ring_slot_is_retired(
            crate::MAX_FRAMES_IN_FLIGHT,
            crate::MAX_FRAMES_IN_FLIGHT
        ));
        assert!(!ring_slot_is_retired(BUF_RING - 1, crate::MAX_FRAMES_IN_FLIGHT));
    }

    #[test]
    fn a_slot_is_reused_only_after_a_whole_ring_of_bound_frames() {
        let mut c = 0usize;
        let mut seen = vec![c];
        for _ in 0..BUF_RING {
            c = BufRing::next_cursor(c, true);
            seen.push(c);
        }
        assert_eq!(seen[0], seen[BUF_RING], "the ring must close on itself");
        assert_eq!(
            seen[..BUF_RING].iter().collect::<std::collections::HashSet<_>>().len(),
            BUF_RING,
            "every slot must be distinct before one repeats"
        );
    }

    #[test]
    fn an_unbound_slot_is_reused_in_place() {
        let mut c = 7 % BUF_RING;
        for _ in 0..10 {
            c = BufRing::next_cursor(c, false);
        }
        assert_eq!(c, 7 % BUF_RING);
    }

    #[test]
    fn a_bound_slot_is_never_the_one_just_written() {
        for start in 0..BUF_RING {
            assert_ne!(BufRing::next_cursor(start, true), start);
        }
    }

    #[test]
    fn a_fixed_write_without_a_slot_keeps_nothing() {
        let mut r = BufRing::named("t");
        assert_eq!(r.write_fixed(&[1, 2, 3, 4], 4), 0);
        assert!(r.peek().is_none());
    }
}
