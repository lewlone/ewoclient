//! Frame serials and deferred destruction.
//!
//! Every frame submission (windowed [`crate::renderer::Renderer`] or headless
//! [`crate::offscreen::Offscreen`]) takes a serial from [`FrameClock`]; when its
//! fence is observed signalled, the clock learns that every frame up to it has
//! retired. A resource retired *now* may still be read by any frame already
//! submitted and by the one being recorded, so it is tagged `submitted + 1` and
//! released once `completed` reaches that tag — no `vkDeviceWaitIdle` needed.
//!
//! Frees need `&mut Allocator`, so the queue is drained from
//! [`crate::Gpu::collect_garbage`], which `&mut Gpu` paths call opportunistically
//! (ring growth, uploads, the app's frame loop) and `Gpu::drop` calls last.

use std::cell::{Cell, RefCell};

use ash::vk;
use gpu_allocator::vulkan::{Allocation, Allocator};

type Destroy = Box<dyn FnOnce(&ash::Device, &mut Allocator)>;

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

    /// Run every destructor whose tag has retired (all of them when `all`).
    pub(crate) fn collect(&self, device: &ash::Device, allocator: &mut Allocator, all: bool) {
        let done = self.completed.get();
        let ready: Vec<Destroy> = {
            let mut g = self.garbage.borrow_mut();
            if g.is_empty() {
                return;
            }
            let (ready, keep): (Vec<_>, Vec<_>) =
                g.drain(..).partition(|(tag, _)| all || *tag <= done);
            *g = keep;
            ready.into_iter().map(|(_, f)| f).collect()
        };
        for f in ready {
            f(device, allocator);
        }
    }

    #[cfg(test)]
    fn pending(&self) -> usize {
        self.garbage.borrow().len()
    }
}

impl crate::Gpu {
    /// Serial the next frame submission will get.
    pub fn frame_serial(&self) -> u64 {
        self.clock.submitted()
    }

    /// Frames known to have retired on the GPU.
    pub fn frames_retired(&self) -> u64 {
        self.clock.completed()
    }

    /// Destroy `f`'s objects once every frame that could reference them retired.
    pub(crate) fn defer_destroy(&self, f: impl FnOnce(&ash::Device, &mut Allocator) + 'static) {
        self.clock.defer(Box::new(f));
    }

    pub(crate) fn defer_destroy_buffer(&self, buffer: vk::Buffer, alloc: Option<Allocation>) {
        self.defer_destroy(move |d, a| unsafe {
            d.destroy_buffer(buffer, None);
            if let Some(al) = alloc {
                let _ = a.free(al);
            }
        });
    }

    /// Release garbage whose frames have retired. Cheap when nothing is pending.
    pub fn collect_garbage(&mut self) {
        let device = self.device.clone();
        self.clock.collect(&device, &mut self.allocator, false);
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
        c.defer(Box::new(|_, _| {}));
        // Tag = submitted + 1 = 2: frame 0 retiring is not enough, because the
        // retire may have happened while frame 1 was being recorded.
        c.mark_retired(s0);
        assert_eq!(c.completed(), 1);
        let s1 = c.begin_submit();
        c.mark_retired(s1);
        assert_eq!(c.completed(), 2);
        assert_eq!(c.pending(), 1);
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
