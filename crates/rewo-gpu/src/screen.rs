//! The screen framework's render half (M82) — a screen's backdrop gradient
//! and its buttons.
//!
//! Deliberately *only* those two. Everything else a screen draws already has a
//! pass: its text is [`crate::text`] (the vanilla bitmap font, the same one
//! the HUD and the F3 overlay use), its item icons are [`crate::gui_item`],
//! and the inventory's panel and slot highlights stay in [`crate::container`],
//! which is inventory-specific from `slot_at` to `preview_rect` and has no
//! business growing a second screen's chrome.
//!
//! # The pipeline is `container`'s, not a copy of it
//!
//! One textured quad × a per-vertex sRGB colour, blended src-over, no depth —
//! byte-for-byte what `container.vert`/`container.frag` already are. So this
//! pass builds [`crate::container::build_pipeline`] rather than shipping a
//! second GLSL pair that could drift from it. The colour is linearised in the
//! fragment shader because the attachment is an sRGB format that encodes on
//! store; the texture is sampled from an `R8G8B8A8_SRGB` image and is already
//! linear. Same discipline as every other Rewo UI pass.
//!
//! # The button is blitted 1:1 and that is a decision
//!
//! `widget/button.png` is 200×20 with a `nine_slice` `.mcmeta` (`border: 3`),
//! and `Button.BIG_WIDTH` is 200 — so at the size the death screen draws them
//! the nine-slice **is** a 1:1 blit: the corners come out at 3 px and the
//! middles span exactly the source's own `200 − 6` and `20 − 6`.
//!
//! `blitNineSlicedSprite` is not transcribed here. It is ~200 lines with a
//! tile-vs-stretch fork (`GuiSpriteScaling.NineSlice.stretchInner`, default
//! **false** → `blitTiledSprite`), and every line of it would be untested at
//! the one size Rewo actually uses. A screen that wants a 150-wide button
//! needs it; until then [`ButtonDraw`] is asserted to be the sprite's own
//! size, so the gap fails loud rather than stretching silently.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::container::{build_pipeline, Rect, Vertex};
use crate::entities::create_texture;
use crate::Gpu;

const VERTEX_STRIDE: u64 = 32; // vec2 pos + vec2 uv + vec4 color
/// The backdrop plus a generous button count. A vanilla screen tops out well
/// below this (`PauseScreen` has eight); the statistics screen's list will
/// need its own budget when it lands.
const MAX_VERTS: usize = 1024;
const RING: usize = 2;
const ATLAS_W: u32 = 256;
const ATLAS_H: u32 = 64;

/// `Button.BIG_WIDTH` / `Button.DEFAULT_HEIGHT`, and the three sheets' own
/// size. Mirrored from `rewo_world::screen` rather than imported: `rewo-gpu`
/// holds no dependency on the world crate, the same arrangement the font, skin
/// and container slices use.
pub const SPRITE_W: i32 = 200;
pub const SPRITE_H: i32 = 20;

/// Which of `AbstractButton.SPRITES` to blit.
///
/// A mirror of `rewo_world::screen::ButtonSprite` for the crate-boundary
/// reason above. The *choice* between the three is the world crate's — it is
/// `WidgetSprites.get(active, hoveredOrFocused)` and belongs with the widget
/// model — so this type never decides, it only names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonSprite {
    Enabled,
    Disabled,
    Highlighted,
}

/// One button to draw, in **GUI space** (the app multiplies nothing; this pass
/// applies the GUI scale, exactly as [`crate::container`] does for the panel).
#[derive(Clone, Copy, Debug)]
pub struct ButtonDraw {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub sprite: ButtonSprite,
}

/// Everything a screen asks this pass for in one frame.
#[derive(Clone, Debug, Default)]
pub struct ScreenDraw {
    /// `(top, bottom)` sRGB + straight alpha, or `None` for a screen that
    /// paints no backdrop of its own. `col1` is the top — see
    /// `ColoredRectangleRenderState.buildVertices`.
    pub backdrop: Option<([f32; 4], [f32; 4])>,
    pub buttons: Vec<ButtonDraw>,
}

impl ScreenDraw {
    pub fn is_empty(&self) -> bool {
        self.backdrop.is_none() && self.buttons.is_empty()
    }
}

/// Borrowed view of `rewo_data::assets::WidgetSprites`.
pub struct WidgetSpriteData<'a> {
    pub button: crate::hud::HudSpriteData<'a>,
    pub button_disabled: crate::hud::HudSpriteData<'a>,
    pub button_highlighted: crate::hud::HudSpriteData<'a>,
}

pub struct ScreenPass {
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
    verts: u32,
    enabled: Rect,
    disabled: Rect,
    highlighted: Rect,
    white: Rect,
}

impl ScreenPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        sprites: &WidgetSpriteData<'_>,
    ) -> Result<Self, String> {
        let mut atlas = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
        let place = |dst: &mut [u8], s: &crate::hud::HudSpriteData<'_>, x: u32, y: u32| {
            for row in 0..s.h.min(ATLAS_H - y) {
                let src = (row * s.w * 4) as usize;
                let d = (((y + row) * ATLAS_W + x) * 4) as usize;
                let n = (s.w.min(ATLAS_W - x) * 4) as usize;
                dst[d..d + n].copy_from_slice(&s.rgba[src..src + n]);
            }
        };
        place(&mut atlas, &sprites.button, 0, 0);
        place(&mut atlas, &sprites.button_disabled, 0, 20);
        place(&mut atlas, &sprites.button_highlighted, 0, 40);
        // One opaque white texel so the untextured backdrop can share this
        // pipeline — the fragment shader's `texture * color` then leaves the
        // gradient alone.
        let w = ((4 * ATLAS_W + 208) * 4) as usize;
        atlas[w..w + 4].copy_from_slice(&[255, 255, 255, 255]);

        let uv = |x: f32, y: f32, w: f32, h: f32| Rect {
            u0: x / ATLAS_W as f32,
            v0: y / ATLAS_H as f32,
            u1: (x + w) / ATLAS_W as f32,
            v1: (y + h) / ATLAS_H as f32,
        };
        let enabled = uv(0.0, 0.0, SPRITE_W as f32, SPRITE_H as f32);
        let disabled = uv(0.0, 20.0, SPRITE_W as f32, SPRITE_H as f32);
        let highlighted = uv(0.0, 40.0, SPRITE_W as f32, SPRITE_H as f32);
        // Half a texel in, so filtering cannot reach a neighbour.
        let white = uv(208.5, 4.5, 0.0, 0.0);

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
                .map_err(|e| format!("screen sampler: {e}"))?
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
                .map_err(|e| format!("screen set layout: {e}"))?
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
                .map_err(|e| format!("screen pool: {e}"))?
        };
        let set_layouts = [set_layout];
        let set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("screen set: {e}"))?[0]
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
                .map_err(|e| format!("screen layout: {e}"))?
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
            verts: 0,
            enabled,
            disabled,
            highlighted,
            white,
        })
    }

    /// Build this frame's geometry from a screen's draw list.
    pub fn set_state(&mut self, extent: vk::Extent2D, draw: &ScreenDraw) {
        let (w, h) = (extent.width.max(1) as f32, extent.height.max(1) as f32);
        let scale = crate::hud::gui_scale(w, h);
        self.cursor = (self.cursor + 1) % RING;
        let mut v: Vec<Vertex> = Vec::with_capacity(64);

        // 1. The backdrop, over everything the world drew, in *screen* pixels
        //    so it covers the leftover strip an odd window size leaves past
        //    `floor(h / scale) * scale`.
        if let Some((top, bottom)) = draw.backdrop {
            push_quad(&mut v, 0.0, 0.0, w, h, self.white, top, bottom);
        }

        // 2. The buttons, in GUI space.
        for b in &draw.buttons {
            let rect = match b.sprite {
                ButtonSprite::Enabled => self.enabled,
                ButtonSprite::Disabled => self.disabled,
                ButtonSprite::Highlighted => self.highlighted,
            };
            // See the module docs: nine-slice is not transcribed, so a button
            // at any other size would be silently stretched. Refuse instead.
            if b.width != SPRITE_W || b.height != SPRITE_H {
                log::warn!(
                    "screen: {}x{} button needs blitNineSlicedSprite, which is not \
                     implemented — skipped",
                    b.width,
                    b.height
                );
                continue;
            }
            push_quad(
                &mut v,
                b.x as f32 * scale,
                b.y as f32 * scale,
                b.width as f32 * scale,
                b.height as f32 * scale,
                rect,
                [1.0; 4],
                [1.0; 4],
            );
        }

        self.verts = v.len() as u32;
        if let Some(alloc) = self.allocs[self.cursor].as_ref() {
            if let Some(ptr) = alloc.mapped_ptr() {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        v.as_ptr() as *const u8,
                        ptr.as_ptr() as *mut u8,
                        std::mem::size_of_val(&v[..]),
                    );
                }
            }
        }
    }

    pub fn draw(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D) {
        if self.verts == 0 {
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
            device.cmd_draw(cb, self.verts, 1, 0, 0);
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

/// One textured quad, top colour `c0` and bottom `c1`.
///
/// A near-twin of `container::push_quad` and separate from it for one reason:
/// the cap is this pass's `MAX_VERTS`, not that one's. Sharing the function
/// would mean threading a cap through every container call site to buy nothing.
fn push_quad(
    v: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    qw: f32,
    qh: f32,
    r: Rect,
    c0: [f32; 4],
    c1: [f32; 4],
) {
    let corners = [
        ([x, y], [r.u0, r.v0], c0),
        ([x + qw, y], [r.u1, r.v0], c0),
        ([x + qw, y + qh], [r.u1, r.v1], c1),
        ([x, y], [r.u0, r.v0], c0),
        ([x + qw, y + qh], [r.u1, r.v1], c1),
        ([x, y + qh], [r.u0, r.v1], c1),
    ];
    if v.len() + corners.len() > MAX_VERTS {
        return;
    }
    for (pos, uv, color) in corners {
        v.push(Vertex { pos, uv, color });
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
            .map_err(|e| format!("screen vertex buffer: {e}"))?
    };
    let req = unsafe { gpu.device.get_buffer_memory_requirements(buf) };
    let alloc = gpu
        .allocator
        .allocate(&AllocationCreateDesc {
            name: "screen-verts",
            requirements: req,
            location: MemoryLocation::CpuToGpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|e| format!("screen vertex alloc: {e}"))?;
    unsafe {
        gpu.device
            .bind_buffer_memory(buf, alloc.memory(), alloc.offset())
            .map_err(|e| format!("screen bind: {e}"))?;
    }
    Ok((buf, alloc))
}
