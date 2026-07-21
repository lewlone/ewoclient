//! Windowed frame driver: acquire → clear (dynamic rendering) → overlay →
//! present, with per-frame GPU timestamps and 2 frames in flight.

use ash::vk;

use crate::overlay::{OverlayDraw, OverlayFrameRes, OverlayPipeline};
use crate::swapchain::Swapchain;
use crate::world::{DepthTarget, WorldRenderer};
use crate::{image_barrier, Gpu, FRAMES_IN_FLIGHT};

pub enum RenderOutcome {
    Rendered,
    /// Swapchain is stale (resize/minimize race) — caller should `resize()`.
    NeedsRecreate,
    /// Zero-sized surface (minimized) — nothing drawn.
    Skipped,
}

struct Frame {
    pool: vk::CommandPool,
    cb: vk::CommandBuffer,
    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
    fence: vk::Fence,
}

pub struct Renderer {
    pub swapchain: Swapchain,
    frames: Vec<Frame>,
    overlay: OverlayPipeline,
    overlay_res: Vec<OverlayFrameRes>,
    query_pool: vk::QueryPool,
    cursor: usize,
    frames_submitted: u64,
    preferred_present: vk::PresentModeKHR,
    depth: Option<DepthTarget>,
    /// GPU time of the most recently *measured* frame (2 frames behind).
    pub last_gpu_ms: Option<f32>,
}

impl Renderer {
    pub fn new(
        gpu: &mut Gpu,
        width: u32,
        height: u32,
        preferred_present: vk::PresentModeKHR,
    ) -> Result<Self, String> {
        let swapchain = Swapchain::new(gpu, width, height, preferred_present)?;
        let (overlay, overlay_res) = OverlayPipeline::new(gpu, swapchain.format, FRAMES_IN_FLIGHT)?;
        unsafe {
            let device = &gpu.device;
            let mut frames = Vec::with_capacity(FRAMES_IN_FLIGHT);
            for _ in 0..FRAMES_IN_FLIGHT {
                let pool = device
                    .create_command_pool(
                        &vk::CommandPoolCreateInfo::default()
                            .queue_family_index(gpu.graphics_family),
                        None,
                    )
                    .map_err(|e| format!("command pool: {e}"))?;
                let cb = device
                    .allocate_command_buffers(
                        &vk::CommandBufferAllocateInfo::default()
                            .command_pool(pool)
                            .level(vk::CommandBufferLevel::PRIMARY)
                            .command_buffer_count(1),
                    )
                    .map_err(|e| format!("command buffer: {e}"))?[0];
                let image_available = device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                    .map_err(|e| format!("semaphore: {e}"))?;
                let render_finished = device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                    .map_err(|e| format!("semaphore: {e}"))?;
                let fence = device
                    .create_fence(
                        &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                        None,
                    )
                    .map_err(|e| format!("fence: {e}"))?;
                frames.push(Frame {
                    pool,
                    cb,
                    image_available,
                    render_finished,
                    fence,
                });
            }
            let query_pool = device
                .create_query_pool(
                    &vk::QueryPoolCreateInfo::default()
                        .query_type(vk::QueryType::TIMESTAMP)
                        .query_count((FRAMES_IN_FLIGHT * 2) as u32),
                    None,
                )
                .map_err(|e| format!("query pool: {e}"))?;

            Ok(Self {
                swapchain,
                frames,
                overlay,
                overlay_res,
                query_pool,
                cursor: 0,
                frames_submitted: 0,
                preferred_present,
                depth: None,
                last_gpu_ms: None,
            })
        }
    }

    pub fn present_mode(&self) -> vk::PresentModeKHR {
        self.swapchain.present_mode
    }

    /// (Re)create the depth target to match the swapchain extent — required
    /// before passing a world to `render()`. Call after `new` and after any
    /// resize/recreate.
    pub fn ensure_depth(&mut self, gpu: &mut Gpu) -> Result<(), String> {
        let extent = self.swapchain.extent;
        let needs = match &self.depth {
            Some(d) => d.extent.width != extent.width || d.extent.height != extent.height,
            None => true,
        };
        if needs {
            gpu.wait_idle();
            if let Some(mut old) = self.depth.take() {
                old.destroy(gpu);
            }
            self.depth = Some(DepthTarget::new(gpu, extent)?);
        }
        Ok(())
    }

    pub fn resize(&mut self, gpu: &Gpu, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        // winit fires several same-size Resized events during window setup;
        // each recreate costs a wait_idle, so skip the no-ops.
        if width == self.swapchain.extent.width && height == self.swapchain.extent.height {
            return Ok(());
        }
        self.swapchain
            .recreate(gpu, width, height, self.preferred_present)
    }

    /// Unconditional recreate — for OUT_OF_DATE/suboptimal recovery, where
    /// the swapchain is stale even at an unchanged extent.
    pub fn recreate(&mut self, gpu: &Gpu, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        self.swapchain
            .recreate(gpu, width, height, self.preferred_present)
    }

    pub fn render(
        &mut self,
        gpu: &Gpu,
        mut world: Option<(&mut WorldRenderer, [[f32; 4]; 4])>,
        draw: &OverlayDraw,
        clear: [f32; 4],
    ) -> Result<RenderOutcome, String> {
        if world.is_some() && self.depth.is_none() {
            return Err("render: world pass requires ensure_depth() first".into());
        }
        if self.swapchain.extent.width == 0 || self.swapchain.extent.height == 0 {
            return Ok(RenderOutcome::Skipped);
        }
        let frame = &self.frames[self.cursor];
        let device = &gpu.device;
        unsafe {
            device
                .wait_for_fences(&[frame.fence], true, u64::MAX)
                .map_err(|e| format!("wait fence: {e}"))?;

            // This slot's queries are from 2 frames ago and are complete now.
            if self.frames_submitted >= FRAMES_IN_FLIGHT as u64 {
                let mut ts = [0u64; 2];
                if device
                    .get_query_pool_results(
                        self.query_pool,
                        (self.cursor * 2) as u32,
                        &mut ts,
                        vk::QueryResultFlags::TYPE_64,
                    )
                    .is_ok()
                {
                    let ns = (ts[1].wrapping_sub(ts[0])) as f64 * gpu.timestamp_period_ns;
                    self.last_gpu_ms = Some((ns / 1.0e6) as f32);
                }
            }

            let (image_index, mut suboptimal) = match self.swapchain.fns.acquire_next_image(
                self.swapchain.handle,
                u64::MAX,
                frame.image_available,
                vk::Fence::null(),
            ) {
                Ok(pair) => pair,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => return Ok(RenderOutcome::NeedsRecreate),
                Err(e) => return Err(format!("acquire: {e}")),
            };

            device
                .reset_fences(&[frame.fence])
                .map_err(|e| format!("reset fence: {e}"))?;
            device
                .reset_command_pool(frame.pool, vk::CommandPoolResetFlags::empty())
                .map_err(|e| format!("reset pool: {e}"))?;
            device
                .begin_command_buffer(
                    frame.cb,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin cb: {e}"))?;

            let q0 = (self.cursor * 2) as u32;
            device.cmd_reset_query_pool(frame.cb, self.query_pool, q0, 2);
            device.cmd_write_timestamp2(
                frame.cb,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                self.query_pool,
                q0,
            );

            let image = self.swapchain.images[image_index as usize];
            image_barrier(
                device,
                frame.cb,
                image,
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::empty(),
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            );

            let view = self.swapchain.views[image_index as usize];
            let extent = self.swapchain.extent;

            // Pass 1 (optional): world with depth, clearing both.
            if let Some((world_renderer, view_proj)) = world.as_mut() {
                let depth = self.depth.as_ref().unwrap();
                depth.barrier_for_use(gpu, frame.cb);
                let color_attachment = vk::RenderingAttachmentInfo::default()
                    .image_view(view)
                    .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .load_op(vk::AttachmentLoadOp::CLEAR)
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .clear_value(vk::ClearValue {
                        color: vk::ClearColorValue { float32: clear },
                    });
                let depth_attachment = vk::RenderingAttachmentInfo::default()
                    .image_view(depth.view)
                    .image_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
                    .load_op(vk::AttachmentLoadOp::CLEAR)
                    .store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .clear_value(vk::ClearValue {
                        depth_stencil: vk::ClearDepthStencilValue {
                            depth: 0.0, // reversed-Z: 0 = far
                            stencil: 0,
                        },
                    });
                let rendering_info = vk::RenderingInfo::default()
                    .render_area(vk::Rect2D::default().extent(extent))
                    .layer_count(1)
                    .color_attachments(std::slice::from_ref(&color_attachment))
                    .depth_attachment(&depth_attachment);
                device.cmd_begin_rendering(frame.cb, &rendering_info);
                world_renderer.draw(gpu, frame.cb, *view_proj, extent);
                device.cmd_end_rendering(frame.cb);

                // World writes → overlay pass reads/writes the same color.
                image_barrier(
                    device,
                    frame.cb,
                    image,
                    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags2::COLOR_ATTACHMENT_READ
                        | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                );
            }

            // Pass 2: overlay. Clears when it's the only pass; loads over
            // the world otherwise.
            let overlay_load = if world.is_some() {
                vk::AttachmentLoadOp::LOAD
            } else {
                vk::AttachmentLoadOp::CLEAR
            };
            let color_attachment = vk::RenderingAttachmentInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(overlay_load)
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(vk::ClearValue {
                    color: vk::ClearColorValue { float32: clear },
                });
            let rendering_info = vk::RenderingInfo::default()
                .render_area(vk::Rect2D::default().extent(extent))
                .layer_count(1)
                .color_attachments(std::slice::from_ref(&color_attachment));
            device.cmd_begin_rendering(frame.cb, &rendering_info);
            self.overlay.draw(
                gpu,
                frame.cb,
                &mut self.overlay_res[self.cursor],
                draw,
                extent,
            );
            device.cmd_end_rendering(frame.cb);

            image_barrier(
                device,
                frame.cb,
                image,
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                vk::AccessFlags2::empty(),
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::PRESENT_SRC_KHR,
            );
            device.cmd_write_timestamp2(
                frame.cb,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                self.query_pool,
                q0 + 1,
            );
            device
                .end_command_buffer(frame.cb)
                .map_err(|e| format!("end cb: {e}"))?;

            let waits = [vk::SemaphoreSubmitInfo::default()
                .semaphore(frame.image_available)
                .stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)];
            let signals = [vk::SemaphoreSubmitInfo::default()
                .semaphore(frame.render_finished)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
            let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(frame.cb)];
            let submit = vk::SubmitInfo2::default()
                .wait_semaphore_infos(&waits)
                .command_buffer_infos(&cbs)
                .signal_semaphore_infos(&signals);
            device
                .queue_submit2(gpu.graphics_queue, std::slice::from_ref(&submit), frame.fence)
                .map_err(|e| format!("submit: {e}"))?;

            let swapchains = [self.swapchain.handle];
            let indices = [image_index];
            let present_waits = [frame.render_finished];
            let present_info = vk::PresentInfoKHR::default()
                .wait_semaphores(&present_waits)
                .swapchains(&swapchains)
                .image_indices(&indices);
            match self
                .swapchain
                .fns
                .queue_present(gpu.graphics_queue, &present_info)
            {
                Ok(sub) => suboptimal |= sub,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => suboptimal = true,
                Err(e) => return Err(format!("present: {e}")),
            }

            self.cursor = (self.cursor + 1) % FRAMES_IN_FLIGHT;
            self.frames_submitted += 1;

            if suboptimal {
                return Ok(RenderOutcome::NeedsRecreate);
            }
        }
        Ok(RenderOutcome::Rendered)
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        unsafe {
            let device = &gpu.device;
            for f in self.frames.drain(..) {
                device.destroy_fence(f.fence, None);
                device.destroy_semaphore(f.image_available, None);
                device.destroy_semaphore(f.render_finished, None);
                device.destroy_command_pool(f.pool, None);
            }
            device.destroy_query_pool(self.query_pool, None);
        }
        if let Some(mut depth) = self.depth.take() {
            depth.destroy(gpu);
        }
        self.overlay.destroy(gpu, &mut self.overlay_res);
        self.swapchain.destroy(gpu);
    }
}
