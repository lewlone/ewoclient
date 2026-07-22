//! Headless render target: same clear + overlay pass as the windowed path,
//! into an offscreen image, with a readback path that writes a PNG.
//!
//! This is the M0 seed of the project's headless-verification policy
//! (REWO_PLAN.md): every milestone must be checkable without a human at
//! the window. Serialized (one submit + fence wait per frame) — this is a
//! correctness harness, not the perf path.

use std::path::Path;

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::overlay::{OverlayDraw, OverlayFrameRes, OverlayPipeline};
use crate::world::{DepthTarget, WorldRenderer};
use crate::{color_range, image_barrier, Gpu};

pub struct Offscreen {
    pub extent: vk::Extent2D,
    pub format: vk::Format,
    image: vk::Image,
    image_alloc: Option<Allocation>,
    view: vk::ImageView,
    depth: DepthTarget,
    readback: vk::Buffer,
    readback_alloc: Option<Allocation>,
    pool: vk::CommandPool,
    cb: vk::CommandBuffer,
    fence: vk::Fence,
    query_pool: vk::QueryPool,
    overlay: OverlayPipeline,
    overlay_res: Vec<OverlayFrameRes>,
    pub last_gpu_ms: Option<f32>,
}

impl Offscreen {
    pub fn new(gpu: &mut Gpu, width: u32, height: u32) -> Result<Self, String> {
        let format = vk::Format::R8G8B8A8_SRGB;
        let extent = vk::Extent2D {
            width,
            height,
        };
        let (overlay, overlay_res) = OverlayPipeline::new(gpu, format, 1)?;
        let depth = DepthTarget::new(gpu, extent)?;
        unsafe {
            let device = &gpu.device;
            let image = device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(format)
                        .extent(vk::Extent3D {
                            width,
                            height,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(
                            vk::ImageUsageFlags::COLOR_ATTACHMENT
                                | vk::ImageUsageFlags::TRANSFER_SRC,
                        )
                        .sharing_mode(vk::SharingMode::EXCLUSIVE)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .map_err(|e| format!("offscreen image: {e}"))?;
            let req = device.get_image_memory_requirements(image);
            let image_alloc = gpu
                .allocator
                .allocate(&AllocationCreateDesc {
                    name: "offscreen-color",
                    requirements: req,
                    location: MemoryLocation::GpuOnly,
                    linear: false,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .map_err(|e| format!("offscreen alloc: {e}"))?;
            device
                .bind_image_memory(image, image_alloc.memory(), image_alloc.offset())
                .map_err(|e| format!("offscreen bind: {e}"))?;
            let view = device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format)
                        .subresource_range(color_range()),
                    None,
                )
                .map_err(|e| format!("offscreen view: {e}"))?;

            let readback_size = (width as u64) * (height as u64) * 4;
            let readback = device
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(readback_size)
                        .usage(vk::BufferUsageFlags::TRANSFER_DST)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                )
                .map_err(|e| format!("readback buffer: {e}"))?;
            let req = device.get_buffer_memory_requirements(readback);
            let readback_alloc = gpu
                .allocator
                .allocate(&AllocationCreateDesc {
                    name: "offscreen-readback",
                    requirements: req,
                    location: MemoryLocation::GpuToCpu,
                    linear: true,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .map_err(|e| format!("readback alloc: {e}"))?;
            device
                .bind_buffer_memory(readback, readback_alloc.memory(), readback_alloc.offset())
                .map_err(|e| format!("readback bind: {e}"))?;

            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.graphics_family),
                    None,
                )
                .map_err(|e| format!("offscreen pool: {e}"))?;
            let cb = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .map_err(|e| format!("offscreen cb: {e}"))?[0];
            let fence = device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .map_err(|e| format!("offscreen fence: {e}"))?;
            let query_pool = device
                .create_query_pool(
                    &vk::QueryPoolCreateInfo::default()
                        .query_type(vk::QueryType::TIMESTAMP)
                        .query_count(2),
                    None,
                )
                .map_err(|e| format!("offscreen query pool: {e}"))?;

            Ok(Self {
                extent,
                format,
                image,
                image_alloc: Some(image_alloc),
                view,
                depth,
                readback,
                readback_alloc: Some(readback_alloc),
                pool,
                cb,
                fence,
                query_pool,
                overlay,
                overlay_res,
                last_gpu_ms: None,
            })
        }
    }

    /// Render one frame (optional world pass, then overlay) and wait for
    /// it. Serialized on purpose — see module docs.
    pub fn render(
        &mut self,
        gpu: &Gpu,
        mut world: Option<(&mut WorldRenderer, [[f32; 4]; 4])>,
        draw: &OverlayDraw,
        clear: [f32; 4],
    ) -> Result<(), String> {
        unsafe {
            let device = &gpu.device;
            device
                .reset_command_pool(self.pool, vk::CommandPoolResetFlags::empty())
                .map_err(|e| format!("reset pool: {e}"))?;
            device
                .begin_command_buffer(
                    self.cb,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin cb: {e}"))?;

            device.cmd_reset_query_pool(self.cb, self.query_pool, 0, 2);
            device.cmd_write_timestamp2(
                self.cb,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                self.query_pool,
                0,
            );

            // Contents are recreated every frame — transition from UNDEFINED.
            image_barrier(
                device,
                self.cb,
                self.image,
                vk::PipelineStageFlags2::TOP_OF_PIPE,
                vk::AccessFlags2::empty(),
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            );

            // Pass 1 (optional): world with depth.
            if let Some((world_renderer, view_proj)) = world.as_mut() {
                // GPU cull before the render pass.
                world_renderer.cull(gpu, self.cb, *view_proj);
                self.depth.barrier_for_use(gpu, self.cb);
                let color_attachment = vk::RenderingAttachmentInfo::default()
                    .image_view(self.view)
                    .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .load_op(vk::AttachmentLoadOp::CLEAR)
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .clear_value(vk::ClearValue {
                        color: vk::ClearColorValue { float32: clear },
                    });
                let depth_attachment = vk::RenderingAttachmentInfo::default()
                    .image_view(self.depth.view)
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
                    .render_area(vk::Rect2D::default().extent(self.extent))
                    .layer_count(1)
                    .color_attachments(std::slice::from_ref(&color_attachment))
                    .depth_attachment(&depth_attachment);
                device.cmd_begin_rendering(self.cb, &rendering_info);
                world_renderer.draw(gpu, self.cb, *view_proj, self.extent);
                device.cmd_end_rendering(self.cb);
                image_barrier(
                    device,
                    self.cb,
                    self.image,
                    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags2::COLOR_ATTACHMENT_READ
                        | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                );
            }

            let overlay_load = if world.is_some() {
                vk::AttachmentLoadOp::LOAD
            } else {
                vk::AttachmentLoadOp::CLEAR
            };
            let color_attachment = vk::RenderingAttachmentInfo::default()
                .image_view(self.view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(overlay_load)
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(vk::ClearValue {
                    color: vk::ClearColorValue { float32: clear },
                });
            let rendering_info = vk::RenderingInfo::default()
                .render_area(vk::Rect2D::default().extent(self.extent))
                .layer_count(1)
                .color_attachments(std::slice::from_ref(&color_attachment));
            device.cmd_begin_rendering(self.cb, &rendering_info);
            self.overlay
                .draw(gpu, self.cb, &mut self.overlay_res[0], draw, self.extent);
            device.cmd_end_rendering(self.cb);

            // Leave the image ready for readback; `save_png` copies from
            // TRANSFER_SRC_OPTIMAL.
            image_barrier(
                device,
                self.cb,
                self.image,
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                vk::PipelineStageFlags2::TRANSFER,
                vk::AccessFlags2::TRANSFER_READ,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            );
            device.cmd_write_timestamp2(
                self.cb,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                self.query_pool,
                1,
            );
            device
                .end_command_buffer(self.cb)
                .map_err(|e| format!("end cb: {e}"))?;

            let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(self.cb)];
            let submit = vk::SubmitInfo2::default().command_buffer_infos(&cbs);
            device
                .queue_submit2(gpu.graphics_queue, std::slice::from_ref(&submit), self.fence)
                .map_err(|e| format!("submit: {e}"))?;
            device
                .wait_for_fences(&[self.fence], true, u64::MAX)
                .map_err(|e| format!("wait fence: {e}"))?;
            device
                .reset_fences(&[self.fence])
                .map_err(|e| format!("reset fence: {e}"))?;

            let mut ts = [0u64; 2];
            if device
                .get_query_pool_results(
                    self.query_pool,
                    0,
                    &mut ts,
                    vk::QueryResultFlags::TYPE_64,
                )
                .is_ok()
            {
                let ns = (ts[1].wrapping_sub(ts[0])) as f64 * gpu.timestamp_period_ns;
                self.last_gpu_ms = Some((ns / 1.0e6) as f32);
            }
        }
        Ok(())
    }

    /// Copy the last rendered frame to the readback buffer and return the
    /// raw RGBA8 pixels (row-major, `extent.width × extent.height`).
    pub fn read_rgba(&mut self, gpu: &Gpu) -> Result<Vec<u8>, String> {
        unsafe {
            let device = &gpu.device;
            device
                .reset_command_pool(self.pool, vk::CommandPoolResetFlags::empty())
                .map_err(|e| format!("reset pool: {e}"))?;
            device
                .begin_command_buffer(
                    self.cb,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin cb: {e}"))?;
            let region = vk::BufferImageCopy::default()
                .buffer_offset(0)
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(0)
                        .base_array_layer(0)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: self.extent.width,
                    height: self.extent.height,
                    depth: 1,
                });
            device.cmd_copy_image_to_buffer(
                self.cb,
                self.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.readback,
                std::slice::from_ref(&region),
            );
            let buffer_barrier = vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                .dst_access_mask(vk::AccessFlags2::HOST_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.readback)
                .offset(0)
                .size(vk::WHOLE_SIZE);
            let dep = vk::DependencyInfo::default()
                .buffer_memory_barriers(std::slice::from_ref(&buffer_barrier));
            device.cmd_pipeline_barrier2(self.cb, &dep);
            device
                .end_command_buffer(self.cb)
                .map_err(|e| format!("end cb: {e}"))?;
            let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(self.cb)];
            let submit = vk::SubmitInfo2::default().command_buffer_infos(&cbs);
            device
                .queue_submit2(gpu.graphics_queue, std::slice::from_ref(&submit), self.fence)
                .map_err(|e| format!("submit: {e}"))?;
            device
                .wait_for_fences(&[self.fence], true, u64::MAX)
                .map_err(|e| format!("wait fence: {e}"))?;
            device
                .reset_fences(&[self.fence])
                .map_err(|e| format!("reset fence: {e}"))?;

            let len = (self.extent.width * self.extent.height * 4) as usize;
            let alloc = self.readback_alloc.as_ref().ok_or("readback alloc gone")?;
            let mapped = alloc.mapped_slice().ok_or("readback not host-visible")?;
            Ok(mapped[..len].to_vec())
        }
    }

    /// Copy the last rendered frame to the readback buffer and encode a PNG.
    pub fn save_png(&mut self, gpu: &Gpu, path: &Path) -> Result<(), String> {
        let pixels = self.read_rgba(gpu)?;
        let file = std::fs::File::create(path).map_err(|e| format!("create {path:?}: {e}"))?;
        let writer = std::io::BufWriter::new(file);
        let mut encoder = png::Encoder::new(writer, self.extent.width, self.extent.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut png_writer = encoder
            .write_header()
            .map_err(|e| format!("png header: {e}"))?;
        png_writer
            .write_image_data(&pixels)
            .map_err(|e| format!("png data: {e}"))?;
        Ok(())
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        self.depth.destroy(gpu);
        unsafe {
            let device = &gpu.device;
            device.destroy_query_pool(self.query_pool, None);
            device.destroy_fence(self.fence, None);
            device.destroy_command_pool(self.pool, None);
            device.destroy_buffer(self.readback, None);
            if let Some(a) = self.readback_alloc.take() {
                let _ = gpu.allocator.free(a);
            }
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            if let Some(a) = self.image_alloc.take() {
                let _ = gpu.allocator.free(a);
            }
        }
        self.overlay.destroy(gpu, &mut self.overlay_res);
    }
}
