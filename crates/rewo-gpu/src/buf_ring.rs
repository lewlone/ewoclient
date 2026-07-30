//! A per-frame vertex/uniform/storage buffer that is safe to replace while
//! earlier frames are still in flight (M86).
//!
//! Eight passes used to hold a single `Option<Buf>` and open their `set_*` with
//! `free_buf(gpu, self.vbuf.take())` — destroying a buffer that command buffers
//! already submitted still reference. Under validation that is
//! `VUID-vkDestroyBuffer-buffer-00922`, once per destroy per in-flight frame; a
//! ten-second windowed run produced ~35,000 of them the moment M86 re-enabled
//! the passes. It was invisible before only because those passes were never
//! constructed in `rewo live` (the bake was dropped — see the M86 §15 entry) and
//! is invisible headlessly because a one-frame oracle never overlaps itself.
//!
//! # Why `fif + 1` and not `fif`
//!
//! This is the one number worth deriving rather than copying, because the tree
//! already contains the *other* answer and the two are one apart.
//!
//! [`crate::world::LIGHTMAP_UBO_RING`] is sized `>= fif`, and that is correct
//! **for it**: its slot is written inside `WorldRenderer::draw`, i.e. inside
//! `Renderer::render`, i.e. *after* that frame's fence wait. At that point
//! frames `0 ..= n - fif` have retired.
//!
//! A `set_*` runs in the app's frame loop **before** `render`, so the most
//! recent fence wait was the *previous* frame's — frames `0 ..= n - 1 - fif`
//! have retired, and `fif` frames may still be reading their buffers. A slot
//! last written `ring` frames ago is therefore safe exactly when
//! `n - ring <= n - 1 - fif`, i.e. `ring >= fif + 1`. [`ring_slot_is_retired`]
//! states it; `the_ring_outlives_every_frame_in_flight` checks it over every
//! `fif` the `--fif` knob permits.
//!
//! # Why the cursor advances on *use*, not on *call*
//!
//! The cursor moves in [`BufRing::set`] only when the slot it is about to
//! overwrite was actually bound by a draw. Two consequences, both load-bearing:
//!
//! - Calling `set` twice in one frame consumes **one** slot, not two. The
//!   second call frees a buffer no command buffer has seen, which is always
//!   legal. Without this the ring would have to be `calls_per_frame * (fif + 1)`
//!   long, and "how many times does the driver call this per frame?" is exactly
//!   the sort of invariant a later milestone breaks silently — one of the
//!   sibling agents adding an inventory-adjacent screen this same week would
//!   have had to know it.
//! - A pass whose draw is skipped (nothing to draw, or `render` returned
//!   `Skipped`/`NeedsRecreate` before recording) never advances, so it also
//!   never grows a hole. Its slot was not bound, so replacing it in place is
//!   safe by the same argument.
//!
//! The flag is a `Cell` because `draw` takes `&self` throughout this crate.

use std::cell::Cell;

use ash::vk;

use crate::end_sky::{upload_buffer, Buf};
use crate::Gpu;

/// Slots in a [`BufRing`].
///
/// `MAX_FRAMES_IN_FLIGHT + 1`, **not** `MAX_FRAMES_IN_FLIGHT` — see the module
/// docs. Sized for the worst case `--fif` permits rather than for the default,
/// because a pass holds no reference to the `Renderer` driving it and so cannot
/// ask what the knob is set to.
pub(crate) const BUF_RING: usize = crate::MAX_FRAMES_IN_FLIGHT + 1;

/// Is the slot a `set_*` is about to overwrite guaranteed to have been released
/// by the GPU?
///
/// Pure, so the invariant is checkable without a device — which is all it is
/// for, hence `cfg(test)`, matching `world::ring_slot_is_retired`. The
/// production path satisfies it by construction
/// (`BUF_RING == MAX_FRAMES_IN_FLIGHT + 1`) rather than by asking at runtime.
#[cfg(test)]
pub(crate) const fn ring_slot_is_retired(ring: usize, fif: usize) -> bool {
    ring >= fif + 1
}

/// One buffer's worth of per-frame data, rotated across [`BUF_RING`] slots.
pub(crate) struct BufRing {
    slots: [Option<Buf>; BUF_RING],
    cursor: usize,
    /// Whether `slots[cursor]` has been handed to a draw since it was written.
    bound: Cell<bool>,
}

impl BufRing {
    pub(crate) fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
            cursor: 0,
            bound: Cell::new(false),
        }
    }

    /// The slot [`Self::set`] would write next. Pure book-keeping, extracted so
    /// the rotation rule can be tested without a device.
    fn next_cursor(cursor: usize, bound: bool) -> usize {
        if bound {
            (cursor + 1) % BUF_RING
        } else {
            cursor
        }
    }

    /// Replace this frame's contents. An empty `bytes` clears the slot, which
    /// is how every caller expresses "draw nothing this frame" — matching the
    /// pre-M86 behaviour, where an empty upload left `vbuf` as `None` rather
    /// than creating `upload_buffer`'s 4-byte minimum.
    pub(crate) fn set(
        &mut self,
        gpu: &mut Gpu,
        bytes: &[u8],
        usage: vk::BufferUsageFlags,
    ) -> Result<(), String> {
        self.cursor = Self::next_cursor(self.cursor, self.bound.replace(false));
        free_buf(gpu, self.slots[self.cursor].take());
        if bytes.is_empty() {
            return Ok(());
        }
        self.slots[self.cursor] = Some(upload_buffer(gpu, bytes, usage)?);
        Ok(())
    }

    /// Drop this frame's contents without uploading anything.
    pub(crate) fn clear(&mut self, gpu: &mut Gpu) {
        let _ = self.set(gpu, &[], vk::BufferUsageFlags::VERTEX_BUFFER);
    }

    /// The handle a draw should bind, recording that the slot is now in use.
    ///
    /// Call this from every path that actually binds. A path that returns
    /// early without binding must **not** call it — leaving the flag clear is
    /// what lets the next `set` reuse the slot in place, which is correct
    /// precisely because nothing ever read it.
    pub(crate) fn bind(&self) -> Option<vk::Buffer> {
        let h = self.slots[self.cursor].as_ref().map(|b| b.buffer)?;
        self.bound.set(true);
        Some(h)
    }

    /// The current handle without claiming it. For `is_some()`-style guards and
    /// for witnesses that want to observe the rotation without perturbing it.
    pub(crate) fn peek(&self) -> Option<vk::Buffer> {
        self.slots[self.cursor].as_ref().map(|b| b.buffer)
    }

    /// Every buffer this ring is currently keeping alive, as raw handles.
    ///
    /// The witness `live --render-check` is built on, and the reason it is not
    /// simply "consecutive frames return different handles": that is **true
    /// under the bug too**, measured — a driver hands back a fresh
    /// `VkBuffer` even when you destroy one and immediately create another of
    /// the same size, so "the handle changed" says nothing about whether the
    /// old one still exists. What distinguishes a correct ring is that the
    /// buffer a frame bound is *still alive* several frames later, which is
    /// exactly the property whose absence produces
    /// `VUID-vkDestroyBuffer-buffer-00922`.
    pub(crate) fn live(&self) -> Vec<u64> {
        use ash::vk::Handle;
        self.slots
            .iter()
            .filter_map(|s| s.as_ref().map(|b| b.buffer.as_raw()))
            .collect()
    }

    /// Which slot [`Self::peek`] and [`Self::bind`] currently answer with.
    ///
    /// Only [`crate::clouds`] needs this: its buffers reach the shader through
    /// a **descriptor set**, and a descriptor set may not be updated while a
    /// command buffer that has it bound is pending. So it rings its sets too,
    /// on this cursor, which keeps the two rotations one thing rather than two
    /// that have to agree.
    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(crate) fn destroy(&mut self, gpu: &mut Gpu) {
        for s in 0..BUF_RING {
            free_buf(gpu, self.slots[s].take());
        }
        self.cursor = 0;
        self.bound.set(false);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_outlives_every_frame_in_flight() {
        // The `--fif` knob is clamped to `1..=MAX_FRAMES_IN_FLIGHT`, so these
        // are every configuration a run can be in.
        for fif in 1..=crate::MAX_FRAMES_IN_FLIGHT {
            assert!(
                ring_slot_is_retired(BUF_RING, fif),
                "ring {BUF_RING} cannot serve frames-in-flight {fif}"
            );
        }
        // The mutation this is here to catch: `MAX_FRAMES_IN_FLIGHT` on its own
        // is the *other* ring's rule (written inside `render`, after the fence
        // wait) and is one short for a `set_*` ring.
        assert!(
            !ring_slot_is_retired(crate::MAX_FRAMES_IN_FLIGHT, crate::MAX_FRAMES_IN_FLIGHT),
            "a ring of exactly MAX_FRAMES_IN_FLIGHT must NOT pass a set_* ring's test"
        );
        // ...and it is not merely bigger than it needs to be: one slot shorter
        // fails at the largest legal fif.
        assert!(!ring_slot_is_retired(BUF_RING - 1, crate::MAX_FRAMES_IN_FLIGHT));
    }

    #[test]
    fn a_slot_is_reused_only_after_a_whole_ring_of_bound_frames() {
        // One `set` + one bind per frame: the cursor walks the ring and a slot
        // comes round again after exactly BUF_RING frames.
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
        // Several `set`s in one frame, none of them drawn yet: they all land on
        // the same slot, so the ring's depth is spent on frames rather than on
        // however many times a driver happens to call `set`.
        let mut c = 7 % BUF_RING;
        for _ in 0..10 {
            c = BufRing::next_cursor(c, false);
        }
        assert_eq!(c, 7 % BUF_RING);
    }

    #[test]
    fn a_bound_slot_is_never_the_one_just_written() {
        // The property the VUID is about, stated positively: after a bind, the
        // next write goes somewhere else.
        for start in 0..BUF_RING {
            assert_ne!(BufRing::next_cursor(start, true), start);
        }
    }
}
