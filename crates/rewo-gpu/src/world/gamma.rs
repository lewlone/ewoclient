//! The gamma-space scope (M50, M52b): the UNORM twin of the colour attachment
//! and the machinery that closes the frame's dynamic rendering instance,
//! reopens it against that view for a pass that must blend in gamma-encoded
//! numbers, and hands the sRGB instance back unchanged.

use ash::vk;

use super::WorldRenderer;
use crate::Gpu;

/// The attachment views the glint's gamma-space scope needs (M50).
///
/// `color_srgb` is the view the surrounding scope was opened with and the one
/// `draw` must leave open; `color_unorm` is a second view of the **same image**
/// in the non-sRGB counterpart format, which is where the glint blends.
#[derive(Clone, Copy)]
pub struct GammaScope {
    pub color_srgb: vk::ImageView,
    pub color_unorm: vk::ImageView,
    pub depth: vk::ImageView,
}

/// The non-sRGB counterpart of an sRGB attachment format.
///
/// Same bit layout, no transfer function on store — so a shader writing the
/// identical value produces the identical bytes, and the fixed-function blender
/// operates on the gamma-encoded numbers exactly as vanilla's does.
pub fn unorm_of(format: vk::Format) -> Option<vk::Format> {
    match format {
        vk::Format::R8G8B8A8_SRGB => Some(vk::Format::R8G8B8A8_UNORM),
        vk::Format::B8G8R8A8_SRGB => Some(vk::Format::B8G8R8A8_UNORM),
        // Already linear-encoded: the blend is in gamma space by construction.
        vk::Format::R8G8B8A8_UNORM | vk::Format::B8G8R8A8_UNORM => Some(format),
        _ => None,
    }
}

impl WorldRenderer {
    /// Re-open the surrounding rendering scope against `view`, preserving both
    /// attachments (M50).
    ///
    /// `LOAD`/`STORE` on both, and no clear: the colour already holds the frame
    /// so far, and the depth already holds what the solid pass wrote — which is
    /// the whole point, since the glint depth-tests `EQUAL` against it.
    unsafe fn reopen_scope(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        extent: vk::Extent2D,
        color: vk::ImageView,
        depth: vk::ImageView,
    ) {
        let color_attachment = vk::RenderingAttachmentInfo::default()
            .image_view(color)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE);
        let depth_attachment = vk::RenderingAttachmentInfo::default()
            .image_view(depth)
            .image_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE);
        let info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D::default().extent(extent))
            .layer_count(1)
            .color_attachments(std::slice::from_ref(&color_attachment))
            .depth_attachment(&depth_attachment);
        gpu.device.cmd_begin_rendering(cb, &info);
    }

    /// Run `f` with the colour attachment swapped for its UNORM view, so the
    /// fixed-function blend inside sees gamma-encoded numbers — vanilla's
    /// space. A no-op (and `f` is skipped) when there is no such view: a glint
    /// blended in linear light is wrong rather than merely dimmer.
    pub(super) fn in_gamma_space(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        extent: vk::Extent2D,
        f: impl FnOnce(),
    ) {
        let Some(g) = self.gamma else {
            return;
        };
        unsafe {
            gpu.device.cmd_end_rendering(cb);
            Self::scope_barrier(gpu, cb);
            self.reopen_scope(gpu, cb, extent, g.color_unorm, g.depth);
            f();
            gpu.device.cmd_end_rendering(cb);
            Self::scope_barrier(gpu, cb);
            self.reopen_scope(gpu, cb, extent, g.color_srgb, g.depth);
        }
    }

    /// Order one rendering instance's attachment writes before the next
    /// instance's loads and writes of the same images.
    ///
    /// Two dynamic-rendering instances have no implicit dependency between
    /// them: the first's store ops (colour: `COLOR_ATTACHMENT_OUTPUT` /
    /// `COLOR_ATTACHMENT_WRITE`; depth: `LATE_FRAGMENT_TESTS` /
    /// `DEPTH_STENCIL_ATTACHMENT_WRITE`) and the second's `LOAD`s (colour
    /// read; depth read in `EARLY_FRAGMENT_TESTS`) are a read-after-write
    /// hazard — on the colour image through a different (sRGB vs UNORM) view,
    /// so the barrier is a global memory barrier rather than one per view. No
    /// layout changes: both instances use the attachment-optimal layouts.
    unsafe fn scope_barrier(gpu: &Gpu, cb: vk::CommandBuffer) {
        let stages = vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT
            | vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
            | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS;
        let barrier = vk::MemoryBarrier2::default()
            .src_stage_mask(stages)
            .src_access_mask(
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )
            .dst_stage_mask(stages)
            .dst_access_mask(
                vk::AccessFlags2::COLOR_ATTACHMENT_READ
                    | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                    | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
            );
        gpu.device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().memory_barriers(std::slice::from_ref(&barrier)),
        );
    }

    /// Run `f` inside the gamma-space scope — the public door onto
    /// [`Self::in_gamma_space`], for the Velvet UI passes (M52b).
    ///
    /// The Velvet HUD has the same requirement the glint does and for the same
    /// reason: EwoClient's `rgba()` is a plain `/255` with no transfer
    /// function, so Skia composites `WINE 0.50` in **gamma** space, while an
    /// sRGB attachment blends in **linear** — and `dst*(1-a) + src*a` is not
    /// invariant under the sRGB transfer.
    ///
    /// **A pass called in here must have been built with the UNORM format**
    /// (`unorm_of(color_format)`), not the sRGB one. Vulkan requires a
    /// pipeline's rendering formats to match the attachment, so a pass
    /// constructed against the sRGB format and drawn in this scope is a
    /// validation error rather than a subtle colour shift. That is the trap:
    /// the colour-space part is easy to reason about, and the *pipeline
    /// format* part is the one that actually bites.
    ///
    /// Skipped entirely when the target has no mutable-format twin — the same
    /// stance M50 takes, because a Velvet plate blended in linear light is
    /// wrong rather than merely dimmer.
    pub fn with_gamma_space(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        extent: vk::Extent2D,
        f: impl FnOnce(),
    ) {
        self.in_gamma_space(gpu, cb, extent, f);
    }

    /// This frame's attachment views, for the glint's gamma-space scope (M50).
    ///
    /// `None` — no mutable-format target — draws no glint at all. Set by
    /// whoever owns the attachments, which is the caller: `draw` runs inside a
    /// rendering scope it did not begin, and to put the glint's blend in
    /// vanilla's space it has to close that scope, reopen it against the UNORM
    /// view, and hand it back unchanged.
    pub fn set_gamma_scope(&mut self, scope: Option<GammaScope>) {
        self.gamma = scope;
    }
}
