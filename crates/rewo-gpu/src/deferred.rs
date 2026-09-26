//! Frame serials and deferred destruction.
//!
//! Every frame submission (windowed [`crate::renderer::Renderer`] or headless
//! [`crate::offscreen::Offscreen`]) takes a serial from [`FrameClock`]; when its
//! fence is observed signalled, the clock learns that every frame up to it has
//! retired. A resource retired *now* may still be read by any frame already
//! submitted and by the one being recorded, so it is tagged `submitted + 1` and
//! released once `completed` reaches that tag — no `vkDeviceWaitIdle` needed.
//! (A fence signal also covers every earlier submission on the queue, so a
//! one-off upload submitted before frame N is retired once frame N is.)
//!
//! Destructors run from [`crate::Gpu::collect_garbage`], which `&mut Gpu` paths
//! call opportunistically (ring growth, uploads, the app's frame loop), and
//! from `Gpu::drop`.

use std::cell::{Cell, RefCell};

use ash::vk;
use gpu_allocator::vulkan::Allocation;

use crate::Gpu;

type Destroy = Box<dyn FnOnce(&mut Gpu)>;

#[derive(Default)]
pub(crate) struct FrameClock {
    /// Frames submitted so far; the next submission gets this serial.
    submitted: Cell<u64>,
    /// Frames known retired: serials `0..completed` have finished on the GPU.
    completed: Cell<u64>,
    garbage: RefCell<Vec<(u64, Destroy)>>,
}

impl FrameClock {
    pub(crate) fn begin_submit(&self) -> u64 {
        let s = self.submitted.get();
        self.submitted.set(s + 1);
        s
    }

    pub(crate) fn mark_retired(&self, serial: u64) {
        if serial + 1 > self.completed.get() {
            self.completed.set(serial + 1);
        }
    }

    pub(crate) fn mark_all_retired(&self) {
        self.completed.set(self.submitted.get());
    }

    pub(crate) fn submitted(&self) -> u64 {
        self.submitted.get()
    }

    pub(crate) fn completed(&self) -> u64 {
        self.completed.get()
    }

    pub(crate) fn defer(&self, f: Destroy) {
        let tag = self.submitted.get() + 1;
        self.garbage.borrow_mut().push((tag, f));
    }

    /// Remove and return every destructor whose tag has retired (all of them
    /// when `all`).
    pub(crate) fn take_ready(&self, all: bool) -> Vec<Destroy> {
        let done = self.completed.get();
        let mut g = self.garbage.borrow_mut();
        if g.is_empty() {
            return Vec::new();
        }
        let (ready, keep): (Vec<_>, Vec<_>) =
            g.drain(..).partition(|(tag, _)| all || *tag <= done);
        *g = keep;
        ready.into_iter().map(|(_, f)| f).collect()
    }

    #[cfg(test)]
    fn pending(&self) -> usize {
        self.garbage.borrow().len()
    }
}

impl Gpu {
    /// Serial the next frame submission will get.
    pub fn frame_serial(&self) -> u64 {
        self.clock.submitted()
    }

    /// Frames known to have retired on the GPU.
    pub fn frames_retired(&self) -> u64 {
        self.clock.completed()
    }

    /// Run `f` once every frame that could reference its objects has retired.
    pub(crate) fn defer_destroy(&self, f: impl FnOnce(&mut Gpu) + 'static) {
        self.clock.defer(Box::new(f));
    }

    pub(crate) fn defer_destroy_buffer(&self, buffer: vk::Buffer, alloc: Option<Allocation>) {
        self.defer_destroy(move |gpu| unsafe {
            gpu.device.destroy_buffer(buffer, None);
            if let Some(a) = alloc {
                let _ = gpu.allocator.free(a);
            }
        });
    }

    /// Release garbage whose frames have retired. Cheap when nothing is pending.
    pub fn collect_garbage(&mut self) {
        for f in self.clock.take_ready(false) {
            f(self);
        }
    }

    /// Release everything, retired or not. Only after the device is idle.
    pub(crate) fn collect_all_garbage(&mut self) {
        loop {
            let ready = self.clock.take_ready(true);
            if ready.is_empty() {
                break;
            }
            for f in ready {
                f(self);
            }
        }
    }

    /// Attach a debug name (no-op without validation / debug utils).
    pub(crate) fn name<H: vk::Handle>(&self, handle: H, name: &str) {
        let Some(du) = &self.debug_device else {
            return;
        };
        let Ok(c) = std::ffi::CString::new(name) else {
            return;
        };
        let info = vk::DebugUtilsObjectNameInfoEXT::default()
            .object_handle(handle)
            .object_name(&c);
        unsafe {
            let _ = du.set_debug_utils_object_name(&info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_waits_for_every_frame_that_could_see_it() {
        let c = FrameClock::default();
        let s0 = c.begin_submit(); // frame 0 in flight
        c.defer(Box::new(|_| {}));
        // Tag = submitted + 1 = 2: frame 0 retiring is not enough, because the
        // retire may have happened while frame 1 was being recorded.
        c.mark_retired(s0);
        assert!(c.take_ready(false).is_empty());
        let s1 = c.begin_submit();
        c.mark_retired(s1);
        assert_eq!(c.take_ready(false).len(), 1);
        assert_eq!(c.pending(), 0);
    }

    #[test]
    fn retirement_never_moves_backwards() {
        let c = FrameClock::default();
        for _ in 0..5 {
            c.begin_submit();
        }
        c.mark_retired(3);
        c.mark_retired(1);
        assert_eq!(c.completed(), 4);
        c.mark_all_retired();
        assert_eq!(c.completed(), 5);
    }
}
