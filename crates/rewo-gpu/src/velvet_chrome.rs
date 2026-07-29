//! Velvet chrome pass (M52b step 3) — the glass plate, analytically.
//!
//! `draw_iw_shell_driven`'s six layers become one instanced quad and one
//! fragment shader over a rounded-box SDF. The constants live in
//! `shaders/velvet_chrome.frag`, transcribed from the Skia original.
//!
//! ## Why this is one draw and not six
//!
//! Skia issues six `draw_rrect` calls, two of them carrying
//! `MaskFilter::blur`. A mask blur over a *rounded rect* has a closed form —
//! the coverage is a smoothstep over the signed distance — so none of it needs
//! a blur pass, a ping-pong target, or a second render target. One quad per
//! shell, one fragment evaluation per covered pixel, every layer composited in
//! the same order Skia draws them.
//!
//! ## The colour-space trap (M50, repeating)
//!
//! **This pass must render through a gamma-space (UNORM) view, not the SRGB
//! attachment.** EwoClient's `rgba()` is a plain `/255` with no transfer
//! function, so Skia composites `WINE 0.50` over the backdrop in *gamma*
//! space. Rewo's swapchain is `B8G8R8A8_SRGB`, where fixed-function blending
//! happens in *linear* — and `dst*(1-a) + src*a` is not invariant under the
//! sRGB transfer, so the same constants produce a visibly different plate.
//!
//! This is exactly the trap M50 hit with the enchantment glint: it went in
//! "structurally correct" and rendered a byte-delta of zero, because the blend
//! space was wrong. M50 already built the fix — `SwapchainTargets` carries a
//! UNORM twin view of every image for precisely this. Use it here, and note
//! that **the Velvet text pass needs the same treatment** or the chrome and
//! the type on top of it will disagree with each other.
//!
//! Recorded rather than silently worked around, because a wrong blend space
//! looks like "the colours are a bit off" rather than like a bug.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::world::DEPTH_FORMAT;
use crate::Gpu;

/// vec4 rect + vec4 params.
const INSTANCE_STRIDE: u64 = 32;
const MAX_SHELLS: usize = 256;
const RING: usize = 2;

/// One glass plate.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shell {
    /// Top-left x, y and size in pixels.
    pub rect: [f32; 4],
    /// `(corner_radius, music_level, music_pulse, alpha)`.
    ///
    /// `level` and `pulse` are the media widget's drive and are **zero for
    /// every other widget** — the Skia original guards the bloom on
    /// `drive.is_some()` precisely so nothing else twitches when a bass note
    /// lands.
    pub params: [f32; 4],
}

impl Shell {
    /// A plate with no music drive — what sixteen of the seventeen widgets use.
    pub fn plain(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Self {
        Self {
            rect: [x, y, w, h],
            params: [radius, 0.0, 0.0, 1.0],
        }
    }
}

pub struct VelvetChromePass {
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    bufs: [vk::Buffer; RING],
    allocs: [Option<Allocation>; RING],
    cursor: usize,
    instances: u32,
}

impl VelvetChromePass {
    pub fn new(gpu: &mut Gpu, color_format: vk::Format) -> Result<Self, String> {
        let device = gpu.device.clone();
        unsafe {
            let push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(8)];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push),
                    None,
                )
                .map_err(|e| format!("velvet chrome layout: {e}"))?;
            let pipeline = build_pipeline(&device, layout, color_format)?;

            let mut bufs = [vk::Buffer::null(); RING];
            let mut allocs: [Option<Allocation>; RING] = [None, None];
            for (i, slot) in allocs.iter_mut().enumerate() {
                let buffer = device
                    .create_buffer(
                        &vk::BufferCreateInfo::default()
                            .size(INSTANCE_STRIDE * MAX_SHELLS as u64)
                            .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
                            .sharing_mode(vk::SharingMode::EXCLUSIVE),
                        None,
                    )
                    .map_err(|e| format!("velvet chrome buffer: {e}"))?;
                let req = device.get_buffer_memory_requirements(buffer);
                let alloc = gpu
                    .allocator
                    .allocate(&AllocationCreateDesc {
                        name: "velvet-chrome-instances",
                        requirements: req,
                        location: MemoryLocation::CpuToGpu,
                        linear: true,
                        allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                    })
                    .map_err(|e| format!("velvet chrome alloc: {e}"))?;
                device
                    .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
                    .map_err(|e| format!("velvet chrome bind: {e}"))?;
                bufs[i] = buffer;
                *slot = Some(alloc);
            }
            Ok(Self {
                layout,
                pipeline,
                bufs,
                allocs,
                cursor: 0,
                instances: 0,
            })
        }
    }

    pub fn draw(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        extent: vk::Extent2D,
        shells: &[Shell],
    ) {
        self.cursor = (self.cursor + 1) % RING;
        let n = shells.len().min(MAX_SHELLS);
        self.instances = n as u32;
        if let Some(slice) = self.allocs[self.cursor]
            .as_mut()
            .and_then(|a| a.mapped_slice_mut())
        {
            let bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    shells.as_ptr() as *const u8,
                    n * INSTANCE_STRIDE as usize,
                )
            };
            slice[..bytes.len()].copy_from_slice(bytes);
        }
        if n == 0 {
            return;
        }
        let (w, h) = (extent.width.max(1) as f32, extent.height.max(1) as f32);
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            let viewport = vk::Viewport::default().width(w).height(h).max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            let screen = [w, h];
            device.cmd_push_constants(
                cb,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                std::slice::from_raw_parts(screen.as_ptr() as *const u8, 8),
            );
            device.cmd_bind_vertex_buffers(cb, 0, &[self.bufs[self.cursor]], &[0]);
            // Six vertices expanded in the shader; one instance per shell.
            device.cmd_draw(cb, 6, self.instances, 0, 0);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        unsafe {
            let device = gpu.device.clone();
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            for (buf, alloc) in self.bufs.iter().zip(self.allocs.iter_mut()) {
                device.destroy_buffer(*buf, None);
                if let Some(a) = alloc.take() {
                    let _ = gpu.allocator.free(a);
                }
            }
        }
    }
}

fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/velvet_chrome.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/velvet_chrome.frag.spv")),
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
        // Per-INSTANCE, not per-vertex: the quad's six corners are generated
        // from gl_VertexIndex, so the only stream is one shell per instance.
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(INSTANCE_STRIDE as u32)
            .input_rate(vk::VertexInputRate::INSTANCE)];
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(16),
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
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
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
        let blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
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
            .create_graphics_pipelines(vk::PipelineCache::null(), &[ci], None)
            .map_err(|(_, e)| format!("velvet chrome pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_instance_struct_matches_the_shader_stride() {
        // A mismatch here reads every shell's params from the next shell's
        // rect, which produces plausible-looking garbage rather than a crash.
        assert_eq!(std::mem::size_of::<Shell>() as u64, INSTANCE_STRIDE);
        assert_eq!(std::mem::align_of::<Shell>(), 4);
    }

    #[test]
    fn a_plain_shell_has_no_music_drive() {
        // Sixteen of seventeen widgets must not twitch on a bass note; the
        // Skia original guards the bloom on `drive.is_some()`.
        let s = Shell::plain(10.0, 20.0, 100.0, 40.0, 12.0);
        assert_eq!(s.params[1], 0.0, "level");
        assert_eq!(s.params[2], 0.0, "pulse");
        assert_eq!(s.params[0], 12.0, "radius");
        assert_eq!(s.params[3], 1.0, "alpha");
    }
}
