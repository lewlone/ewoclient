//! M278 — the glowing outline (vanilla's `minecraft:entity_outline` post
//! effect).
//!
//! Glowing entities draw a second time, in their outline colour, into an
//! offscreen target of their own with its own depth (so the outline shows
//! through walls, as vanilla's does), and `post_effect/entity_outline.json`
//! turns that silhouette into a soft rim:
//!
//! ```text
//! entity_sobel            outline -> swap     (edges: alpha = |Δa| summed)
//! entity_outline_box_blur swap -> outline     (BlurDir (1, 0), radius 2)
//! entity_outline_box_blur outline -> swap     (BlurDir (0, 1))
//! blit                    swap -> outline
//! blitAndBlendToTexture   outline over the frame (SRC_ALPHA, 1 - SRC_ALPHA)
//! ```
//!
//! The final blit is an identity copy, so here the composite samples `swap`
//! directly. Every target is UNORM, as vanilla's RGBA8 one is, and the
//! composite blends through the frame's UNORM view, so the arithmetic is on
//! gamma-encoded numbers like vanilla's. One set of images serves every frame
//! in flight: each use opens with a barrier from the previous frame's last
//! access (barriers order work across command buffers on one queue).

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use super::{DepthTarget, WorldRenderer, DEPTH_FORMAT};
use crate::Gpu;

/// The outline target's format: vanilla's RGBA8, no transfer function.
pub const OUTLINE_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;

/// The post passes' push constants: the input size and the blur direction.
#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    in_size: [f32; 2],
    blur_dir: [f32; 2],
}

struct Target {
    image: vk::Image,
    view: vk::ImageView,
    alloc: Option<Allocation>,
}

impl Target {
    unsafe fn new(gpu: &mut Gpu, extent: vk::Extent2D, name: &str) -> Result<Self, String> {
        let device = gpu.device.clone();
        let image = device
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(OUTLINE_FORMAT)
                    .extent(vk::Extent3D {
                        width: extent.width.max(1),
                        height: extent.height.max(1),
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
            .map_err(|e| format!("{name} image: {e}"))?;
        let req = device.get_image_memory_requirements(image);
        let alloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name,
                requirements: req,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("{name} alloc: {e}"))?;
        device
            .bind_image_memory(image, alloc.memory(), alloc.offset())
            .map_err(|e| format!("{name} bind: {e}"))?;
        let view = device
            .create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(OUTLINE_FORMAT)
                    .subresource_range(color_range()),
                None,
            )
            .map_err(|e| format!("{name} view: {e}"))?;
        Ok(Self {
            image,
            view,
            alloc: Some(alloc),
        })
    }

    fn destroy(&mut self, gpu: &mut Gpu) {
        unsafe {
            gpu.device.destroy_image_view(self.view, None);
            gpu.device.destroy_image(self.image, None);
        }
        if let Some(a) = self.alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(1)
        .base_array_layer(0)
        .layer_count(1)
}

/// The outline's images, pipelines and descriptor sets, at one extent.
pub struct OutlinePass {
    pub extent: vk::Extent2D,
    outline: Target,
    swap: Target,
    depth: DepthTarget,
    nearest: vk::Sampler,
    linear: vk::Sampler,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    /// `(outline, nearest)`, `(swap, linear)`, `(outline, linear)`,
    /// `(swap, nearest)` — the inputs of sobel, blur x, blur y, composite.
    sets: [vk::DescriptorSet; 4],
    post_layout: vk::PipelineLayout,
    sobel: vk::Pipeline,
    blur: vk::Pipeline,
    composite: vk::Pipeline,
    /// The glowing geometry's pipeline, on the entity pass's layout.
    pub geometry: vk::Pipeline,
}

impl OutlinePass {
    /// Build everything at `extent`. `composite_format` is the frame's UNORM
    /// colour format; `entity_layout` the entity pass's pipeline layout.
    pub fn new(
        gpu: &mut Gpu,
        extent: vk::Extent2D,
        composite_format: vk::Format,
        entity_layout: vk::PipelineLayout,
    ) -> Result<Self, String> {
        unsafe {
            let outline = Target::new(gpu, extent, "outline-target")?;
            let swap = Target::new(gpu, extent, "outline-swap")?;
            let depth = DepthTarget::new(gpu, extent)?;
            let device = gpu.device.clone();
            let sampler = |filter: vk::Filter| {
                device.create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(filter)
                        .min_filter(filter)
                        .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .max_lod(0.0),
                    None,
                )
            };
            let nearest =
                sampler(vk::Filter::NEAREST).map_err(|e| format!("outline sampler: {e}"))?;
            let linear =
                sampler(vk::Filter::LINEAR).map_err(|e| format!("outline sampler: {e}"))?;
            let binding = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let set_layout = device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&binding),
                    None,
                )
                .map_err(|e| format!("outline set layout: {e}"))?;
            let sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(4)];
            let pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(4)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(|e| format!("outline pool: {e}"))?;
            let layouts = [set_layout; 4];
            let v = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&layouts),
                )
                .map_err(|e| format!("outline sets: {e}"))?;
            let sets = [v[0], v[1], v[2], v[3]];
            for (set, (view, s)) in sets.iter().zip([
                (outline.view, nearest),
                (swap.view, linear),
                (outline.view, linear),
                (swap.view, nearest),
            ]) {
                let info = [vk::DescriptorImageInfo::default()
                    .sampler(s)
                    .image_view(view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let write = vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&info);
                device.update_descriptor_sets(std::slice::from_ref(&write), &[]);
            }
            let pc = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .offset(0)
                .size(std::mem::size_of::<Push>() as u32)];
            let post_layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(std::slice::from_ref(&set_layout))
                        .push_constant_ranges(&pc),
                    None,
                )
                .map_err(|e| format!("outline layout: {e}"))?;
            let sobel = post_pipeline(
                &device,
                post_layout,
                include_bytes!(concat!(env!("OUT_DIR"), "/outline_sobel.frag.spv")),
                OUTLINE_FORMAT,
                false,
            )?;
            let blur = post_pipeline(
                &device,
                post_layout,
                include_bytes!(concat!(env!("OUT_DIR"), "/outline_blur.frag.spv")),
                OUTLINE_FORMAT,
                false,
            )?;
            let composite = post_pipeline(
                &device,
                post_layout,
                include_bytes!(concat!(env!("OUT_DIR"), "/outline_composite.frag.spv")),
                composite_format,
                true,
            )?;
            let geometry = crate::entities::build_outline_pipeline(&device, entity_layout)?;
            Ok(Self {
                extent,
                outline,
                swap,
                depth,
                nearest,
                linear,
                set_layout,
                pool,
                sets,
                post_layout,
                sobel,
                blur,
                composite,
                geometry,
            })
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        unsafe {
            let d = gpu.device.clone();
            for p in [self.sobel, self.blur, self.composite, self.geometry] {
                d.destroy_pipeline(p, None);
            }
            d.destroy_pipeline_layout(self.post_layout, None);
            d.destroy_descriptor_pool(self.pool, None);
            d.destroy_descriptor_set_layout(self.set_layout, None);
            d.destroy_sampler(self.nearest, None);
            d.destroy_sampler(self.linear, None);
        }
        self.outline.destroy(gpu);
        self.swap.destroy(gpu);
        self.depth.destroy(gpu);
    }
}

/// One full-screen pass's pipeline: no vertex input, no depth, the
/// composite's alpha blend (`ENTITY_OUTLINE_BLIT`) or none.
fn post_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    frag_spv: &[u8],
    format: vk::Format,
    blend: bool,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/outline_post.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(device, frag_spv)?;
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
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
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
        let depth = vk::PipelineDepthStencilStateCreateInfo::default();
        let attach = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(blend)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ZERO)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(if blend {
                // The frame keeps its own alpha (UI passes mask alpha writes).
                vk::ColorComponentFlags::R | vk::ColorComponentFlags::G | vk::ColorComponentFlags::B
            } else {
                vk::ColorComponentFlags::RGBA
            })];
        let blend_state = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attach);
        let dyn_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyn_states);
        let formats = [format];
        // The composite draws inside the frame's own scope, which carries the
        // depth attachment; the other passes render to a colour target alone.
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&formats)
            .depth_attachment_format(if blend {
                DEPTH_FORMAT
            } else {
                vk::Format::UNDEFINED
            });
        let ci = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend_state)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&ci), None)
            .map_err(|(_, e)| format!("outline pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

/// One image barrier on a colour target.
unsafe fn color_barrier(
    gpu: &Gpu,
    cb: vk::CommandBuffer,
    image: vk::Image,
    (src_stage, src_access, old): (vk::PipelineStageFlags2, vk::AccessFlags2, vk::ImageLayout),
    (dst_stage, dst_access, new): (vk::PipelineStageFlags2, vk::AccessFlags2, vk::ImageLayout),
) {
    let b = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(src_stage)
        .src_access_mask(src_access)
        .dst_stage_mask(dst_stage)
        .dst_access_mask(dst_access)
        .old_layout(old)
        .new_layout(new)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(color_range());
    gpu.device.cmd_pipeline_barrier2(
        cb,
        &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&b)),
    );
}

const WRITE: (vk::PipelineStageFlags2, vk::AccessFlags2, vk::ImageLayout) = (
    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
);
const READ: (vk::PipelineStageFlags2, vk::AccessFlags2, vk::ImageLayout) = (
    vk::PipelineStageFlags2::FRAGMENT_SHADER,
    vk::AccessFlags2::SHADER_SAMPLED_READ,
    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
);

impl WorldRenderer {
    /// M278 — make the outline pass match `extent` (a resize rebuilds it,
    /// after the device is idle: the old images may still be in flight).
    /// Without a gamma scope's UNORM format (`unorm`) or an entity pass there
    /// is nothing to composite into, and no outline is drawn.
    pub fn ensure_outline(
        &mut self,
        gpu: &mut Gpu,
        extent: vk::Extent2D,
        unorm: Option<vk::Format>,
    ) -> Result<(), String> {
        if self.outline.as_ref().is_some_and(|o| o.extent == extent) {
            return Ok(());
        }
        let (Some(unorm), Some(layout)) = (unorm, self.entities.as_ref().map(|e| e.layout()))
        else {
            return Ok(());
        };
        if let Some(mut old) = self.outline.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
        self.outline = Some(OutlinePass::new(gpu, extent, unorm, layout)?);
        Ok(())
    }

    /// M278 — whether the last frame drew an outline (for the gates).
    pub fn outline_drawn(&self) -> bool {
        self.outline_drawn.get()
    }

    /// M278 — the outline: the glowing entities into the outline target, the
    /// post chain, and the composite over the frame. Ends and reopens the
    /// frame's rendering scope, as the gamma scope does.
    pub(super) fn draw_outline(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        self.outline_drawn.set(false);
        let (Some(pass), Some(entities), Some(g)) = (&self.outline, &self.entities, self.gamma)
        else {
            return;
        };
        if pass.extent != extent || !entities.has_outline() {
            return;
        }
        let d = &gpu.device;
        let size = [extent.width as f32, extent.height as f32];
        let full = vk::Rect2D::default().extent(extent);
        let viewport = vk::Viewport::default()
            .width(extent.width as f32)
            .height(extent.height as f32)
            .max_depth(1.0);
        let none = (
            vk::PipelineStageFlags2::FRAGMENT_SHADER
                | vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags2::SHADER_SAMPLED_READ | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            vk::ImageLayout::UNDEFINED,
        );
        let begin = |view: vk::ImageView, depth: Option<vk::ImageView>, clear: bool| {
            let color = vk::RenderingAttachmentInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(if clear {
                    vk::AttachmentLoadOp::CLEAR
                } else {
                    vk::AttachmentLoadOp::DONT_CARE
                })
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(vk::ClearValue {
                    color: vk::ClearColorValue { float32: [0.0; 4] },
                });
            let depth_att = depth.map(|v| {
                vk::RenderingAttachmentInfo::default()
                    .image_view(v)
                    .image_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
                    .load_op(vk::AttachmentLoadOp::CLEAR)
                    .store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .clear_value(vk::ClearValue {
                        depth_stencil: vk::ClearDepthStencilValue {
                            depth: 0.0,
                            stencil: 0,
                        },
                    })
            });
            let mut info = vk::RenderingInfo::default()
                .render_area(full)
                .layer_count(1)
                .color_attachments(std::slice::from_ref(&color));
            if let Some(da) = depth_att.as_ref() {
                info = info.depth_attachment(da);
            }
            unsafe { d.cmd_begin_rendering(cb, &info) };
        };
        let post = |pipeline: vk::Pipeline, set: vk::DescriptorSet, dir: [f32; 2]| unsafe {
            d.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
            d.cmd_set_viewport(cb, 0, &[viewport]);
            d.cmd_set_scissor(cb, 0, &[full]);
            d.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                pass.post_layout,
                0,
                &[set],
                &[],
            );
            let p = Push {
                in_size: size,
                blur_dir: dir,
            };
            d.cmd_push_constants(
                cb,
                pass.post_layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                std::slice::from_raw_parts(
                    (&p as *const Push) as *const u8,
                    std::mem::size_of::<Push>(),
                ),
            );
            d.cmd_draw(cb, 3, 1, 0, 0);
        };
        unsafe {
            d.cmd_end_rendering(cb);
            // The silhouettes, through walls: the target's own depth.
            color_barrier(gpu, cb, pass.outline.image, none, WRITE);
            color_barrier(gpu, cb, pass.swap.image, none, WRITE);
            pass.depth.barrier_for_use(gpu, cb);
            begin(pass.outline.view, Some(pass.depth.view), true);
            entities.draw_outline_geometry(gpu, cb, view_proj, extent, pass.geometry);
            d.cmd_end_rendering(cb);
            // entity_sobel: outline -> swap.
            color_barrier(gpu, cb, pass.outline.image, WRITE, READ);
            begin(pass.swap.view, None, false);
            post(pass.sobel, pass.sets[0], [0.0, 0.0]);
            d.cmd_end_rendering(cb);
            // The x blur: swap -> outline.
            color_barrier(gpu, cb, pass.swap.image, WRITE, READ);
            color_barrier(gpu, cb, pass.outline.image, READ, WRITE);
            begin(pass.outline.view, None, false);
            post(pass.blur, pass.sets[1], [1.0, 0.0]);
            d.cmd_end_rendering(cb);
            // The y blur: outline -> swap. GL's texCoord y runs up the screen
            // and `gl_FragCoord`'s down, so vanilla's (0, 1) is (0, -1) here:
            // the kernel is not symmetric.
            color_barrier(gpu, cb, pass.outline.image, WRITE, READ);
            color_barrier(gpu, cb, pass.swap.image, READ, WRITE);
            begin(pass.swap.view, None, false);
            post(pass.blur, pass.sets[2], [0.0, -1.0]);
            d.cmd_end_rendering(cb);
            color_barrier(gpu, cb, pass.swap.image, WRITE, READ);
            // The composite, in the frame's UNORM view, then the sRGB scope
            // handed back.
            Self::scope_barrier(gpu, cb);
            self.reopen_scope(gpu, cb, extent, g.color_unorm, g.depth);
            post(pass.composite, pass.sets[3], [0.0, 0.0]);
            d.cmd_end_rendering(cb);
            Self::scope_barrier(gpu, cb);
            self.reopen_scope(gpu, cb, extent, g.color_srgb, g.depth);
        }
        self.outline_drawn.set(true);
    }
}
