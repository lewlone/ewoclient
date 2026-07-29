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
//!
//! **Construct this pass with `world::unorm_of(target_format)`**, not the
//! sRGB format. It draws inside `WorldRenderer::with_gamma_space`, and
//! Vulkan requires a pipeline's rendering formats to match the attachment.
//! Building against sRGB and drawing in that scope is a validation error --
//! which is the useful half of this trap, because the colour-space part is
//! easy to reason about and the pipeline-format part is what actually bites.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::world::DEPTH_FORMAT;
use crate::Gpu;

/// vec4 rect + vec4 params.
const INSTANCE_STRIDE: u64 = 32;
const MAX_SHELLS: usize = 256;
const RING: usize = 2;

/// The plate's palette — **data, not constants**.
///
/// EwoClient's HUD is getting a visual overhaul, so Velvet's specific colours
/// are a table here rather than literals in a shader. What stays in the shader
/// is the *structure*: six SDF layers in Skia's draw order. A palette change is
/// then a table edit; only a change to the layer structure itself is a shader
/// edit.
///
/// Each entry is `[r, g, b, a]` where `a` is the layer's alpha. The two music
/// gains on the border stay in the shader because they are structural — `a` is
/// the **resting** alpha and the drive scales from it, so a new palette moves
/// the resting value without flattening the reaction.
///
/// Colours are sRGB `byte / 255` with no transfer function, matching
/// `ewo-jni`'s `rgba()`. See the colour-space note in the module docs: this is
/// composited in gamma space.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellStyle {
    pub bloom: [f32; 4],
    pub shadow: [f32; 4],
    pub outer_ring: [f32; 4],
    pub fill: [f32; 4],
    pub inset_ring: [f32; 4],
    pub top_highlight: [f32; 4],
    pub border: [f32; 4],
}

const fn srgb(r: u8, g: u8, b: u8, a: f32) -> [f32; 4] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a]
}

impl ShellStyle {
    /// The current Velvet plate, transcribed from `draw_iw_shell_driven`.
    ///
    /// One table among possible others. When the HUD is redesigned this is the
    /// thing that gets replaced -- not the shader, and not the widget code.
    pub const VELVET: ShellStyle = ShellStyle {
        bloom: srgb(0xE5, 0xB8, 0xC5, 0.42),         // rose, scaled by energy
        shadow: [0.0, 0.0, 0.0, 0.55],               // black, +6y, sigma 8
        outer_ring: srgb(0x12, 0x00, 0x10, 0.55),    // wine, 1px outside
        fill: srgb(0x12, 0x00, 0x10, 0.50),          // wine
        inset_ring: srgb(0x12, 0x00, 0x10, 0.25),    // wine, 1px inside
        top_highlight: srgb(0xF4, 0xE8, 0xEA, 0.10), // pearl, top 2px only
        border: srgb(0xF4, 0xE8, 0xEA, 0.18),        // pearl, resting alpha
    };
}

impl Default for ShellStyle {
    fn default() -> Self {
        Self::VELVET
    }
}

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
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    style_buf: vk::Buffer,
    style_alloc: Option<Allocation>,
    pipeline: vk::Pipeline,
    bufs: [vk::Buffer; RING],
    allocs: [Option<Allocation>; RING],
    cursor: usize,
    instances: u32,
}

impl VelvetChromePass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        style: ShellStyle,
    ) -> Result<Self, String> {
        let device = gpu.device.clone();
        unsafe {
            let bindings = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let set_layout = device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| format!("velvet chrome set layout: {e}"))?;
            let pool_sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)];
            let pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .map_err(|e| format!("velvet chrome pool: {e}"))?;
            let set_layouts = [set_layout];
            let set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("velvet chrome set: {e}"))?[0];

            let style_buf = device
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(std::mem::size_of::<ShellStyle>() as u64)
                        .usage(vk::BufferUsageFlags::UNIFORM_BUFFER)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                )
                .map_err(|e| format!("velvet chrome style buffer: {e}"))?;
            let req = device.get_buffer_memory_requirements(style_buf);
            let mut style_alloc = gpu
                .allocator
                .allocate(&AllocationCreateDesc {
                    name: "velvet-chrome-style",
                    requirements: req,
                    location: MemoryLocation::CpuToGpu,
                    linear: true,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .map_err(|e| format!("velvet chrome style alloc: {e}"))?;
            device
                .bind_buffer_memory(style_buf, style_alloc.memory(), style_alloc.offset())
                .map_err(|e| format!("velvet chrome style bind: {e}"))?;
            write_style(&mut style_alloc, style);
            let buf_info = [vk::DescriptorBufferInfo::default()
                .buffer(style_buf)
                .offset(0)
                .range(std::mem::size_of::<ShellStyle>() as u64)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&buf_info)],
                &[],
            );

            let push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(8)];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&push),
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
                set_layout,
                pool,
                set,
                style_buf,
                style_alloc: Some(style_alloc),
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
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[self.set],
                &[],
            );
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

    /// Swap the palette. The whole reason it is a UBO rather than shader
    /// constants: a visual overhaul is a table edit and takes effect on the
    /// next frame, with no pipeline rebuild.
    pub fn set_style(&mut self, style: ShellStyle) {
        if let Some(a) = self.style_alloc.as_mut() {
            write_style(a, style);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        unsafe {
            let device = gpu.device.clone();
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_buffer(self.style_buf, None);
            if let Some(a) = self.style_alloc.take() {
                let _ = gpu.allocator.free(a);
            }
            for (buf, alloc) in self.bufs.iter().zip(self.allocs.iter_mut()) {
                device.destroy_buffer(*buf, None);
                if let Some(a) = alloc.take() {
                    let _ = gpu.allocator.free(a);
                }
            }
        }
    }
}

fn write_style(alloc: &mut Allocation, style: ShellStyle) {
    if let Some(slice) = alloc.mapped_slice_mut() {
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                &style as *const ShellStyle as *const u8,
                std::mem::size_of::<ShellStyle>(),
            )
        };
        slice[..bytes.len()].copy_from_slice(bytes);
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
    fn the_style_block_matches_the_shader_layout() {
        // Seven vec4s, std140-compatible by construction. A mismatch shifts
        // every layer's colour by one slot -- the plate still draws, in the
        // wrong colours, which is far easier to accept than a crash.
        assert_eq!(std::mem::size_of::<ShellStyle>(), 7 * 16);
    }

    #[test]
    fn velvet_is_one_table_among_possible_others() {
        // The point of de-baking: a palette is data, so a different one is
        // constructible without touching a shader.
        let overhauled = ShellStyle {
            fill: [0.1, 0.2, 0.3, 0.9],
            ..ShellStyle::VELVET
        };
        assert_ne!(overhauled, ShellStyle::VELVET);
        assert_eq!(overhauled.border, ShellStyle::VELVET.border);
        // And the shipped table is still the transcribed one.
        assert_eq!(ShellStyle::VELVET.fill[3], 0.50, "wine fill alpha");
        assert_eq!(ShellStyle::VELVET.shadow[3], 0.55, "drop shadow alpha");
        assert_eq!(ShellStyle::VELVET.border[3], 0.18, "resting border alpha");
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
