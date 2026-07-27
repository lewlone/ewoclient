//! The player inventory screen — panel, backdrop and slot highlights (M35).
//!
//! The screen is four kinds of draw, and only three of them are here:
//!
//! 1. the dimmed backdrop over the world,
//! 2. `inventory.png`, the panel itself,
//! 3. the two slot-highlight sprites around whatever the cursor is over,
//! 4. the item icons and their stack counts — which are the
//!    [`crate::gui_item`] and [`crate::text`] passes, unchanged. An icon in an
//!    inventory slot is the same draw as an icon in a hotbar slot; only the
//!    rectangle differs, which is the whole reason M34's pass took a slot rect
//!    rather than a hotbar index.
//!
//! # Where the panel goes
//!
//! `AbstractContainerScreen.init` is `leftPos = (width - imageWidth) / 2` and
//! `topPos = (height - imageHeight) / 2`, in **GUI space** — the screen scaled
//! down by the same auto GUI scale the HUD uses. Integer division, so an odd
//! leftover pixel lands on the right and bottom; keeping the truncation is
//! what makes the panel sit on whole pixels and the sprite art stay crisp.
//!
//! # The backdrop
//!
//! `Screen.renderTransparentBackground` fills the whole screen with a vertical
//! gradient from `0xC0101010` to `0xD0101010` — a near-black at about 75% and
//! 82% alpha. Not a flat fill: the bottom is fractionally darker.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::entities::create_texture;
use crate::world::DEPTH_FORMAT;
use crate::Gpu;

const VERTEX_STRIDE: u64 = 32; // vec2 pos + vec2 uv + vec4 color
const MAX_VERTS: usize = 1024;
const RING: usize = 2;
const ATLAS_W: u32 = 512;
const ATLAS_H: u32 = 256;

/// `AbstractContainerScreen.imageWidth` / `imageHeight` for the player
/// inventory. Mirrored from [`rewo_world::inventory`] rather than imported —
/// `rewo-gpu` deliberately holds no dependency on the world crate, the same
/// arrangement the font and skin slices use.
pub const GUI_WIDTH: f32 = 176.0;
pub const GUI_HEIGHT: f32 = 166.0;

/// `Screen.renderTransparentBackground`'s two gradient stops, as sRGB + alpha.
const BACKDROP_TOP: [f32; 4] = [16.0 / 255.0, 16.0 / 255.0, 16.0 / 255.0, 192.0 / 255.0];
const BACKDROP_BOTTOM: [f32; 4] = [16.0 / 255.0, 16.0 / 255.0, 16.0 / 255.0, 208.0 / 255.0];

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Borrowed view of `rewo_data::assets::ContainerSprites`.
pub struct ContainerSpriteData<'a> {
    pub background: crate::hud::HudSpriteData<'a>,
    pub highlight_back: crate::hud::HudSpriteData<'a>,
    pub highlight_front: crate::hud::HudSpriteData<'a>,
}

#[derive(Clone, Copy, Default)]
struct Rect {
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

/// The GUI scale, and where the panel's top-left corner sits in screen pixels.
///
/// Returned together because every caller needs all three: the render places
/// sprites with them, and the app maps the cursor back through them to find
/// which slot it is over.
pub fn gui_origin(w: f32, h: f32) -> (f32, f32, f32) {
    let scale = crate::hud::gui_scale(w, h);
    let (sw, sh) = (w / scale, h / scale);
    // Integer division in vanilla, and it matters: a half-pixel origin would
    // resample every sprite in the panel.
    let left = ((sw - GUI_WIDTH) / 2.0).floor();
    let top = ((sh - GUI_HEIGHT) / 2.0).floor();
    (left * scale, top * scale, scale)
}

/// Turn a cursor position in screen pixels into GUI-space coordinates relative
/// to the panel's top-left — which is what
/// `rewo_world::inventory::slot_at` expects.
pub fn screen_to_gui(mouse: (f64, f64), w: f32, h: f32) -> (f64, f64) {
    let (left, top, scale) = gui_origin(w, h);
    (
        (mouse.0 - left as f64) / scale as f64,
        (mouse.1 - top as f64) / scale as f64,
    )
}

pub struct ContainerPass {
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    pipeline: vk::Pipeline,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    image: vk::Image,
    image_alloc: Option<Allocation>,
    view: vk::ImageView,
    bufs: [vk::Buffer; RING],
    allocs: [Option<Allocation>; RING],
    cursor: usize,
    /// The back half's vertex count, and the total. The front highlight is the
    /// tail of the same buffer: vanilla brackets the item between the two
    /// sprites, so this pass is drawn twice per frame with the icons in
    /// between, and two ranges of one upload is simpler than two buffers.
    back_verts: u32,
    total_verts: u32,
    panel: Rect,
    highlight_back: Rect,
    highlight_front: Rect,
    white: Rect,
}

impl ContainerPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        sprites: &ContainerSpriteData<'_>,
    ) -> Result<Self, String> {
        let mut atlas = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
        let place = |dst: &mut [u8], s: &crate::hud::HudSpriteData<'_>, x: u32, y: u32| {
            for row in 0..s.h {
                let src = (row * s.w * 4) as usize;
                let d = (((y + row) * ATLAS_W + x) * 4) as usize;
                let n = (s.w * 4) as usize;
                dst[d..d + n].copy_from_slice(&s.rgba[src..src + n]);
            }
        };
        // The background sheet is 256×256 and only its top-left 176×166 is the
        // panel, so the UVs below crop what the blit crops.
        place(&mut atlas, &sprites.background, 0, 0);
        place(&mut atlas, &sprites.highlight_back, 256, 0);
        place(&mut atlas, &sprites.highlight_front, 280, 0);
        // One opaque texel, so the untextured backdrop can share this pipeline.
        let w = (304 * 4) as usize;
        atlas[w..w + 4].copy_from_slice(&[255, 255, 255, 255]);

        let uv = |x: f32, y: f32, w: f32, h: f32| Rect {
            u0: x / ATLAS_W as f32,
            v0: y / ATLAS_H as f32,
            u1: (x + w) / ATLAS_W as f32,
            v1: (y + h) / ATLAS_H as f32,
        };
        let panel = uv(0.0, 0.0, GUI_WIDTH, GUI_HEIGHT);
        let highlight_back = uv(256.0, 0.0, 24.0, 24.0);
        let highlight_front = uv(280.0, 0.0, 24.0, 24.0);
        // Half a texel in, so bilinear filtering cannot reach a neighbour.
        let white = uv(304.5, 0.5, 0.0, 0.0);

        let (image, image_alloc, view) = create_texture(gpu, &atlas, ATLAS_W, ATLAS_H)?;
        let device = gpu.device.clone();
        let sampler = unsafe {
            device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                    None,
                )
                .map_err(|e| format!("container sampler: {e}"))?
        };
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let set_layout = unsafe {
            device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| format!("container set layout: {e}"))?
        };
        let sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)];
        let pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(|e| format!("container pool: {e}"))?
        };
        let set_layouts = [set_layout];
        let set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("container set: {e}"))?[0]
        };
        let info = [vk::DescriptorImageInfo::default()
            .sampler(sampler)
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        unsafe {
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&info)],
                &[],
            );
        }
        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(8)];
        let layout = unsafe {
            device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&push),
                    None,
                )
                .map_err(|e| format!("container layout: {e}"))?
        };
        let pipeline = build_pipeline(&device, layout, color_format)?;

        let mut bufs = [vk::Buffer::null(); RING];
        let mut allocs: [Option<Allocation>; RING] = [None, None];
        for i in 0..RING {
            let (b, a) = new_vertex_buffer(gpu)?;
            bufs[i] = b;
            allocs[i] = Some(a);
        }
        Ok(Self {
            layout,
            set_layout,
            pipeline,
            pool,
            set,
            sampler,
            image,
            image_alloc: Some(image_alloc),
            view,
            bufs,
            allocs,
            cursor: 0,
            back_verts: 0,
            total_verts: 0,
            panel,
            highlight_back,
            highlight_front,
            white,
        })
    }

    /// Build this frame's geometry. `hovered` is a menu slot's GUI-space
    /// top-left, already resolved by the caller — this pass deliberately does
    /// not know the slot layout, which lives once in
    /// `rewo_world::inventory::slot_position`.
    pub fn set_state(&mut self, extent: vk::Extent2D, hovered: Option<(i32, i32)>) {
        let (w, h) = (extent.width.max(1) as f32, extent.height.max(1) as f32);
        let (left, top, scale) = gui_origin(w, h);
        self.cursor = (self.cursor + 1) % RING;
        let mut v: Vec<Vertex> = Vec::with_capacity(64);

        let quad = |v: &mut Vec<Vertex>,
                        x: f32,
                        y: f32,
                        qw: f32,
                        qh: f32,
                        r: Rect,
                        c0: [f32; 4],
                        c1: [f32; 4]| {
            // `c0` is the top colour and `c1` the bottom, so one helper covers
            // both the flat blits (equal colours) and the backdrop gradient.
            let corners = [
                ([x, y], [r.u0, r.v0], c0),
                ([x + qw, y], [r.u1, r.v0], c0),
                ([x + qw, y + qh], [r.u1, r.v1], c1),
                ([x, y], [r.u0, r.v0], c0),
                ([x + qw, y + qh], [r.u1, r.v1], c1),
                ([x, y + qh], [r.u0, r.v1], c1),
            ];
            for (pos, uv, color) in corners {
                if v.len() < MAX_VERTS {
                    v.push(Vertex { pos, uv, color });
                }
            }
        };

        // 1. The backdrop, over everything the world drew.
        quad(&mut v, 0.0, 0.0, w, h, self.white, BACKDROP_TOP, BACKDROP_BOTTOM);
        // 2. The panel.
        let (pw, ph) = (GUI_WIDTH * scale, GUI_HEIGHT * scale);
        quad(&mut v, left, top, pw, ph, self.panel, WHITE, WHITE);
        // 3. The hovered slot's *back* highlight, at `slot - 4` and 24×24 —
        //    a four-pixel bleed on every side of the 16 px icon.
        let hl = hovered.map(|(sx, sy)| {
            (
                left + (sx - 4) as f32 * scale,
                top + (sy - 4) as f32 * scale,
                24.0 * scale,
            )
        });
        if let Some((x, y, size)) = hl {
            quad(&mut v, x, y, size, size, self.highlight_back, WHITE, WHITE);
        }
        self.back_verts = v.len() as u32;

        // 4. And the front half, which the second draw picks up after the
        //    icons have gone down between them.
        if let Some((x, y, size)) = hl {
            quad(&mut v, x, y, size, size, self.highlight_front, WHITE, WHITE);
        }
        self.total_verts = v.len() as u32;
        self.upload(&v);
    }

    fn upload(&mut self, v: &[Vertex]) {
        if v.is_empty() {
            return;
        }
        if let Some(alloc) = self.allocs[self.cursor].as_ref() {
            if let Some(ptr) = alloc.mapped_ptr() {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        v.as_ptr() as *const u8,
                        ptr.as_ptr() as *mut u8,
                        std::mem::size_of_val(v),
                    );
                }
            }
        }
    }

    /// The backdrop, the panel and the back highlight — everything that goes
    /// *under* the item icons.
    pub fn draw_back(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D) {
        self.draw_range(gpu, cb, extent, 0, self.back_verts);
    }

    /// The front highlight, over the icons.
    pub fn draw_front(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D) {
        self.draw_range(
            gpu,
            cb,
            extent,
            self.back_verts,
            self.total_verts - self.back_verts,
        );
    }

    fn draw_range(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        extent: vk::Extent2D,
        first: u32,
        count: u32,
    ) {
        if count == 0 {
            return;
        }
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            let viewport = vk::Viewport::default()
                .width(extent.width as f32)
                .height(extent.height as f32)
                .max_depth(1.0);
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
            let push = [extent.width as f32, extent.height as f32];
            device.cmd_push_constants(
                cb,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytemuck::cast_slice(&push),
            );
            device.cmd_bind_vertex_buffers(cb, 0, &[self.bufs[self.cursor]], &[0]);
            device.cmd_draw(cb, count, 1, first, 0);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        let device = gpu.device.clone();
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            for b in self.bufs {
                device.destroy_buffer(b, None);
            }
        }
        if let Some(a) = self.image_alloc.take() {
            let _ = gpu.allocator.free(a);
        }
        for a in self.allocs.iter_mut() {
            if let Some(a) = a.take() {
                let _ = gpu.allocator.free(a);
            }
        }
    }
}

fn new_vertex_buffer(gpu: &mut Gpu) -> Result<(vk::Buffer, Allocation), String> {
    let size = VERTEX_STRIDE * MAX_VERTS as u64;
    let buf = unsafe {
        gpu.device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("container vertex buffer: {e}"))?
    };
    let req = unsafe { gpu.device.get_buffer_memory_requirements(buf) };
    let alloc = gpu
        .allocator
        .allocate(&AllocationCreateDesc {
            name: "container-verts",
            requirements: req,
            location: MemoryLocation::CpuToGpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|e| format!("container vertex alloc: {e}"))?;
    unsafe {
        gpu.device
            .bind_buffer_memory(buf, alloc.memory(), alloc.offset())
            .map_err(|e| format!("container bind: {e}"))?;
    }
    Ok((buf, alloc))
}

fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/container.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/container.frag.spv")),
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
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(VERTEX_STRIDE as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(8),
            vk::VertexInputAttributeDescription::default()
                .location(2)
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
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B,
            )];
        let blend_state =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
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
            .color_blend_state(&blend_state)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), &[ci], None)
            .map_err(|(_, e)| format!("container pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel is centred by vanilla's integer division, and the result is
    /// scaled up — so at GUI scale 2 on a 1280×720 window the origin is an even
    /// number of pixels and the sprite art lands on whole texels.
    #[test]
    fn the_panel_is_centred_on_whole_pixels() {
        let (left, top, scale) = gui_origin(1280.0, 720.0);
        assert_eq!(scale, 3.0, "1280×720 gives GUI scale 3");
        // 1280/3 = 426.67 wide in GUI space, so (426.67 - 176)/2 = 125.33 → 125.
        assert_eq!(left, 125.0 * 3.0);
        assert_eq!(top, ((720.0 / 3.0 - GUI_HEIGHT) / 2.0).floor() * 3.0);
        assert_eq!(left.fract(), 0.0);
        assert_eq!(top.fract(), 0.0);
    }

    /// The cursor maps back through the same origin, so the point the panel is
    /// drawn at and the point a click is tested against cannot drift apart.
    #[test]
    fn the_cursor_maps_back_through_the_same_origin() {
        let (w, h) = (1280.0, 720.0);
        let (left, top, scale) = gui_origin(w, h);
        // The panel's own top-left must map to GUI (0, 0).
        let g = screen_to_gui((left as f64, top as f64), w, h);
        assert!(g.0.abs() < 1e-9 && g.1.abs() < 1e-9, "{g:?}");
        // And one GUI pixel across is `scale` screen pixels across.
        let g = screen_to_gui(((left + scale) as f64, top as f64), w, h);
        assert!((g.0 - 1.0).abs() < 1e-9, "{g:?}");
    }

    /// The backdrop's bottom is darker than its top — `0xD0` against `0xC0`.
    /// A flat fill would look almost right, which is why this is pinned.
    #[test]
    fn the_backdrop_is_a_gradient_not_a_fill() {
        assert!(BACKDROP_BOTTOM[3] > BACKDROP_TOP[3]);
        assert_eq!(BACKDROP_TOP[3], 192.0 / 255.0);
        assert_eq!(BACKDROP_BOTTOM[3], 208.0 / 255.0);
    }
}
