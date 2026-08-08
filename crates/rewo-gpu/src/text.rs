//! Screen-space text — the vanilla bitmap font rendered as 2D glyph quads,
//! for the chat overlay + the coordinates/debug line. Drawn last (over the
//! HUD) with its own positive viewport (top-left pixel origin), alpha
//! blended, no depth.
//!
//! Each glyph is drawn twice: a black copy offset (+1,+1) scaled px — the
//! vanilla drop shadow — then the tinted glyph, so text stays readable on
//! any background. Layout uses the font's per-glyph advances.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::entities::{create_texture, FontData};
use crate::world::DEPTH_FORMAT;
use crate::Gpu;

const VERTEX_STRIDE: u64 = 32; // vec2 pos + vec2 uv + vec4 color
const MAX_VERTS: usize = 24_576; // ~1000 glyphs × 2 (shadow) × 6 verts / ~2
const RING: usize = 2;

/// One line of text to draw this frame.
pub struct TextLine<'a> {
    /// Top-left pixel origin (screen-space, before GUI scaling by `px`).
    pub x: f32,
    pub y: f32,
    /// Pixel size of one font pixel (GUI scale; vanilla text cell is 8px).
    pub px: f32,
    /// Linear-space color of the text (shadow is a darkened copy).
    pub color: [f32; 3],
    /// Opacity (chat fades old lines).
    pub alpha: f32,
    /// `graphics.text(font, str, x, y, color, **shadow**)`'s last argument.
    ///
    /// Almost everything the HUD draws passes `true`, and before M79 this pass
    /// hard-coded it. `ContextualBar.extractExperienceLevel` passes `false`
    /// for all five of its draws, because the XP level number carries a
    /// four-way black **outline** instead — and drawing both would put a
    /// shadow copy of every outline copy inside the outline, thickening the
    /// glyph rather than framing it.
    pub shadow: bool,
    /// `Style`'s five renderable flags (M126c). Default is plain, so a caller
    /// that has no styling says so by saying nothing.
    pub style: TextStyle,
    pub text: &'a str,
}

/// The five `Style` flags this pass can draw, resolved.
///
/// Colour is not here: it is already `TextLine::color`, because vanilla
/// resolves `Style.getColor()` against the call site's default before the
/// glyph is built (`Font.PreparedTextBuilder.getTextColor`) and Rewo's chat
/// pipeline does the same in `chat_style`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextStyle {
    /// Drawn twice, one pixel apart, with the quad inflated by
    /// `extraThickness` — **and one pixel wider**, which is why
    /// [`width_styled`] takes it.
    pub bold: bool,
    /// The top edge of every quad slides right and the bottom edge left. See
    /// [`TextPass::shear`].
    pub italic: bool,
    /// A one-pixel bar under the run.
    pub underlined: bool,
    /// A one-pixel bar through it.
    pub strikethrough: bool,
    /// Every glyph replaced, per frame, by a random one of the same width.
    pub obfuscated: bool,
}

impl TextStyle {
    pub const PLAIN: Self = Self {
        bold: false,
        italic: false,
        underlined: false,
        strikethrough: false,
        obfuscated: false,
    };
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

/// `Font.width(String)` — the sum of the per-glyph advances.
///
/// The same sum [`push_line`](TextPass::push_line) pens out, in whole pixels
/// rather than scaled ones, so a caller can place text it has not drawn yet.
pub fn width(text: &str, advance: &[u8; 256]) -> i32 {
    width_styled(text, advance, false)
}

/// `StringSplitter`'s width provider —
/// `getGlyph(cp).info().getAdvance(style.isBold())`.
///
/// **`GlyphInfo.getBoldOffset()` is `1.0F`, and it is charged per character**,
/// not once per run: a five-character bold word is five pixels wider, not one.
/// That is what makes the style argument load-bearing rather than cosmetic — a
/// style-blind measure wraps a bold chat line late and lets it overhang the
/// box (M126b).
///
/// The byte-wise sum is the pre-existing approximation this inherits: the
/// atlas is indexed by byte, so a multi-byte UTF-8 character is measured as
/// two glyphs. Bold doubles that error rather than creating it.
pub fn width_styled(text: &str, advance: &[u8; 256], bold: bool) -> i32 {
    let extra = i32::from(bold);
    text.bytes()
        .map(|b| advance[b as usize] as i32 + extra)
        .sum()
}

/// `GuiGraphicsExtractor.centeredText` — `text(font, str, x - font.width(str)
/// / 2, y, color)`.
///
/// **Integer division**, and it truncates toward zero, so an odd-width string
/// sits half a pixel left of the true centre. Rounding it instead moves the
/// `+N` bundle badge — and every other centred label — by a pixel against
/// vanilla.
pub fn centered_x(text: &str, advance: &[u8; 256], center_x: i32) -> i32 {
    center_x - width(text, advance) / 2
}

pub struct TextPass {
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
    atlas_size: u32,
    cell: u32,
    advance: [u8; 256],
    /// The opaque texel the font bake patches into the space glyph's cell —
    /// vanilla's `FontSet.whiteGlyph()`, used by the underline and
    /// strikethrough bars.
    white_texel: (u32, u32),
    /// `FontSet.glyphsByWidth` — the codepoints an obfuscated character may be
    /// replaced by, bucketed by `Mth.ceil(getAdvance(false))`.
    ///
    /// Vanilla builds this over every supported codepoint, skipping
    /// `SpecialGlyphs.MISSING`; Rewo's font is the 256-cell byte-indexed sheet,
    /// so the bucket for width `w` holds every byte whose cell has ink and
    /// whose advance is `w`. The advance table is already whole pixels, so the
    /// `ceil` is the identity.
    ///
    /// Indexed by advance, and an advance is at most the cell width plus the
    /// bake's two pixels of padding — 32 covers it with room, matching
    /// `hasFishyAdvance`'s own ceiling.
    glyphs_by_width: [Vec<u8>; 33],
    /// Advanced once per [`TextPass::draw`], so an obfuscated run changes
    /// every frame as vanilla's does.
    frame: u64,
}

impl TextPass {
    pub fn new(gpu: &mut Gpu, color_format: vk::Format, font: &FontData<'_>) -> Result<Self, String> {
        let (image, image_alloc, view) = create_texture(gpu, font.atlas, font.size, font.size)?;
        let device = gpu.device.clone();
        unsafe {
            let sampler = device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                    None,
                )
                .map_err(|e| format!("text sampler: {e}"))?;
            let bindings = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let set_layout = device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| format!("text set layout: {e}"))?;
            let pool_sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)];
            let pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .map_err(|e| format!("text pool: {e}"))?;
            let set_layouts = [set_layout];
            let set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("text set: {e}"))?[0];
            let image_info = [vk::DescriptorImageInfo::default()
                .sampler(sampler)
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&image_info)],
                &[],
            );

            let pc = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(8)];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&pc),
                    None,
                )
                .map_err(|e| format!("text layout: {e}"))?;
            let pipeline = build_pipeline(&device, layout, color_format)?;

            let mut bufs = [vk::Buffer::null(); RING];
            let mut allocs: [Option<Allocation>; RING] = [None, None];
            for (i, slot) in allocs.iter_mut().enumerate() {
                let buffer = device
                    .create_buffer(
                        &vk::BufferCreateInfo::default()
                            .size(MAX_VERTS as u64 * VERTEX_STRIDE)
                            .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
                            .sharing_mode(vk::SharingMode::EXCLUSIVE),
                        None,
                    )
                    .map_err(|e| format!("text vbuf: {e}"))?;
                let req = device.get_buffer_memory_requirements(buffer);
                let alloc = gpu
                    .allocator
                    .allocate(&AllocationCreateDesc {
                        name: "text-verts",
                        requirements: req,
                        location: MemoryLocation::CpuToGpu,
                        linear: true,
                        allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                    })
                    .map_err(|e| format!("text vbuf alloc: {e}"))?;
                device
                    .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
                    .map_err(|e| format!("text vbuf bind: {e}"))?;
                bufs[i] = buffer;
                *slot = Some(alloc);
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
                atlas_size: font.size,
                cell: font.cell,
                advance: *font.advance,
                white_texel: font.white_texel,
                glyphs_by_width: glyph_width_buckets(font),
                frame: 0,
            })
        }
    }

    /// Pick the glyph an obfuscated character is drawn as this frame.
    ///
    /// Vanilla is `FontSet.getRandomGlyph(this.random, Mth.ceil(glyph.info()
    /// .getAdvance(false)))` — a uniform pick from the same-width bucket,
    /// using the **unstyled** advance (so a bold obfuscated character still
    /// picks from the plain bucket), with `codepoint != 32` excluding the
    /// space. Those three rules are transcribed here and are what a test can
    /// assert.
    ///
    /// **The sequence is not.** `Font.random` is a nanotime-seeded
    /// `RandomSource`, so vanilla's own output differs between two runs of the
    /// same frame; asserting a specific glyph would assert more than vanilla
    /// guarantees. What Rewo needs on top is *reproducibility*, because every
    /// gate in this project renders headlessly and compares bytes — which is
    /// why the choice below is a deliberate divergence rather than a
    /// transcription.
    fn obfuscated_glyph(&self, b: u8, run: u64, index: usize) -> u8 {
        obfuscated_glyph(&self.glyphs_by_width, &self.advance, b, self.frame, run, index)
    }

    /// Build this frame's glyph quads and draw them.
    pub fn draw(&mut self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D, lines: &[TextLine<'_>]) {
        self.frame = self.frame.wrapping_add(1);
        self.cursor = (self.cursor + 1) % RING;
        let mut v: Vec<Vertex> = Vec::with_capacity(1024);
        for line in lines {
            // Shadow first (offset +1 font-px, darkened), then the glyph.
            if line.shadow {
                let sh = [line.color[0] * 0.25, line.color[1] * 0.25, line.color[2] * 0.25];
                self.push_line(&mut v, line, line.px, line.px, sh, line.alpha);
            }
            self.push_line(&mut v, line, 0.0, 0.0, line.color, line.alpha);
        }
        self.verts = v.len() as u32;
        if let Some(slice) = self.allocs[self.cursor]
            .as_mut()
            .and_then(|a| a.mapped_slice_mut())
        {
            let bytes: &[u8] =
                unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 32) };
            slice[..bytes.len()].copy_from_slice(bytes);
        }
        if self.verts == 0 {
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
            device.cmd_draw(cb, self.verts, 1, 0, 0);
        }
    }

    /// Emit one line's glyph quads at (line.x+ox, line.y+oy) in `color`.
    /// The italic shear, in font pixels — `BakedSheetGlyph.shearTop()` and
    /// `shearBottom()`, which are `1.0 - 0.25 * up` and `1.0 - 0.25 * down`.
    ///
    /// `up` and `down` are the glyph quad's edges relative to the pen's `y`,
    /// and for the vanilla sheet they are **0 and 8**: `GlyphBitmap.getTop()`
    /// is `7.0 - getBearingTop()` and `ascii.png` declares `ascent: 7`, so the
    /// top is 0; `getBottom()` adds the 8-px cell. Derived from `self.cell`
    /// rather than written as `(1.0, -1.0)` so the formula is the thing
    /// transcribed — it happens to evaluate to `+1 / -1` here, which is the
    /// two-pixel lean everyone recognises.
    ///
    /// Both are added to **x**, so the top edge slides right and the bottom
    /// edge left.
    fn shear(&self, italic: bool) -> (f32, f32) {
        shear_for_cell(self.cell as f32, italic)
    }

    fn push_line(&self, v: &mut Vec<Vertex>, line: &TextLine<'_>, ox: f32, oy: f32, color: [f32; 3], alpha: f32) {
        let px = line.px;
        let cell = self.cell as f32;
        let atlas = self.atlas_size as f32;
        let color4 = [color[0], color[1], color[2], alpha];
        let st = line.style;
        // `GlyphInfo.getBoldOffset()` — the second draw's x offset, and the
        // per-character advance increase, are the same 1.0.
        let bold_offset = if st.bold { 1.0 } else { 0.0 };
        // `BakedSheetGlyph.extraThickness(bold)` — the quad grows by a tenth of
        // a pixel on every side, which is what stops the two copies leaving a
        // hairline of background between them.
        let thick = if st.bold { 0.1 } else { 0.0 };
        let (shear_top, shear_bottom) = self.shear(st.italic);
        let left = line.x + ox;
        let mut pen = left;
        // The run's own seed, from the **unoffset** origin: `draw` calls this
        // twice for a shadowed line, and a seed that moved with `ox`/`oy` would
        // scramble the shadow to different characters than the text it
        // shadows.
        let run = run_seed(line);
        for (i, b) in line.text.bytes().enumerate() {
            let b = if st.obfuscated {
                self.obfuscated_glyph(b, run, i)
            } else {
                b
            };
            let adv = self.advance[b as usize] as f32 + bold_offset;
            if b != b' ' {
                // The bold copy is a second quad at `x + boldOffset`, not a
                // thicker one — `renderChar` calls `render` twice.
                let copies = if st.bold { 2 } else { 1 };
                for c in 0..copies {
                    if v.len() + 6 > MAX_VERTS {
                        break;
                    }
                    let (cx, cy) =
                        ((b as u32 % 16 * self.cell) as f32, (b as u32 / 16 * self.cell) as f32);
                    let x = pen + c as f32 * bold_offset * px;
                    let (x0, y0) = (x - thick * px, line.y + oy - thick * px);
                    let (x1, y1) = (x + (cell + thick) * px, line.y + oy + (cell + thick) * px);
                    let (u0, u1) = (cx / atlas, (cx + cell) / atlas);
                    let (t0, t1) = (cy / atlas, (cy + cell) / atlas);
                    // The shear is applied per EDGE, not per vertex: both top
                    // corners take `shearTop` and both bottom ones
                    // `shearBottom`.
                    let (st_x, sb_x) = (shear_top * px, shear_bottom * px);
                    let corners = [
                        ([x0 + st_x, y0], [u0, t0]),
                        ([x1 + st_x, y0], [u1, t0]),
                        ([x1 + sb_x, y1], [u1, t1]),
                        ([x0 + st_x, y0], [u0, t0]),
                        ([x1 + sb_x, y1], [u1, t1]),
                        ([x0 + sb_x, y1], [u0, t1]),
                    ];
                    for (pos, uv) in corners {
                        v.push(Vertex { pos, uv, color: color4 });
                    }
                }
            }
            pen += adv * px;
        }
        // `PreparedTextBuilder.accept`'s two effect rects, emitted after the
        // glyphs exactly as `visit` drains them.
        //
        // **Vanilla emits one rect PER CHARACTER** — `(effectX0, …, this.x +
        // advance, …)` inside the per-glyph accept — and they abut exactly,
        // because each starts where the last ended. Their union is one
        // rectangle, so one quad is not an approximation of N: it is the same
        // covered area with no seam where two edges land on one pixel.
        //
        // **`effectX0 = position == 0 ? this.x - 1.0F : this.x`**, and
        // `position` restarts at 0 for every part — `FormattedCharSequence
        // .fromList` chains `accept` without renumbering and
        // `Language.getVisualOrder` decomposes each part separately. One
        // `TextLine` is one part, so the lead-in belongs to its first
        // character, which is why a multi-colour underlined line has each
        // span's bar overlapping the previous one by a pixel.
        if st.underlined || st.strikethrough {
            let x0 = left - px;
            let x1 = pen;
            if st.strikethrough {
                // `this.y + 4.5F - 1.0F` .. `this.y + 4.5F`.
                self.push_effect(v, x0, line.y + oy + 3.5 * px, x1, line.y + oy + 4.5 * px, color4);
            }
            if st.underlined {
                // `this.y + 9.0F - 1.0F` .. `this.y + 9.0F`. Below the 8-px
                // cell, not inside it.
                self.push_effect(v, x0, line.y + oy + 8.0 * px, x1, line.y + oy + 9.0 * px, color4);
            }
        }
    }

    /// One solid quad, sampling the opaque texel the font bake patches into
    /// the space glyph's cell — vanilla's `FontSet.whiteGlyph()`, which is why
    /// the effects go in this pass's buffer rather than the HUD's fills: they
    /// are glyphs, and they must draw after the text they cross.
    fn push_effect(&self, v: &mut Vec<Vertex>, x0: f32, y0: f32, x1: f32, y1: f32, color4: [f32; 4]) {
        if v.len() + 6 > MAX_VERTS {
            return;
        }
        let atlas = self.atlas_size as f32;
        let u = (self.white_texel.0 as f32 + 0.5) / atlas;
        let t = (self.white_texel.1 as f32 + 0.5) / atlas;
        for pos in [
            [x0, y0],
            [x1, y0],
            [x1, y1],
            [x0, y0],
            [x1, y1],
            [x0, y1],
        ] {
            v.push(Vertex { pos, uv: [u, t], color: color4 });
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        unsafe {
            let device = &gpu.device;
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
        for a in self.allocs.iter_mut().filter_map(|a| a.take()) {
            let _ = gpu.allocator.free(a);
        }
        if let Some(a) = self.image_alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

/// `BakedSheetGlyph.shearTop()` / `shearBottom()`, free of the pass so a test
/// can reach them.
///
/// `down` is the cell height, `up` is 0 — see [`TextPass::shear`] for why.
fn shear_for_cell(cell: f32, italic: bool) -> (f32, f32) {
    if italic {
        (1.0, 1.0 - 0.25 * cell)
    } else {
        (0.0, 0.0)
    }
}

/// A run's contribution to the obfuscation seed — its position on screen.
///
/// Vanilla needs no equivalent because its `Font.random` is one shared stream
/// advanced in draw order, so two runs never see the same state. Rewo's pick
/// is a pure function of its arguments, so without this two obfuscated runs
/// whose characters happen to share a bucket would scramble in lockstep.
///
/// Reads `line.x`/`line.y` and **not** the `ox`/`oy` the shadow pass adds —
/// see [`obfuscation_pick`].
fn run_seed(line: &TextLine<'_>) -> u64 {
    (line.x.to_bits() as u64) << 32 | line.y.to_bits() as u64
}

/// `Font.getGlyph`'s obfuscation branch, as a free function so it is testable
/// without a Vulkan device (M97's rule — logic in a place with no test seam is
/// untestable, so move it).
///
/// Three rules, all transcribed and all assertable:
///
/// * the replacement comes from the bucket of glyphs with the **same**
///   `Mth.ceil(getAdvance(false))`, so an obfuscated run never changes width;
/// * that advance is the **unstyled** one, so a bold obfuscated character
///   still picks from the plain bucket;
/// * **a space is never obfuscated** — `codepoint != 32` guards the whole
///   branch, which is what keeps word boundaries visible in scrambled text.
///
/// An empty bucket returns the character itself. Vanilla returns
/// `missingGlyph` there; Rewo has no missing-glyph cell, and drawing the real
/// character is the smaller lie than drawing nothing.
fn obfuscated_glyph(
    buckets: &[Vec<u8>; 33],
    advance: &[u8; 256],
    b: u8,
    frame: u64,
    run: u64,
    index: usize,
) -> u8 {
    if b == b' ' {
        return b;
    }
    let bucket = &buckets[advance[b as usize].min(32) as usize];
    if bucket.is_empty() {
        return b;
    }
    bucket[obfuscation_pick(frame, run, index, bucket.len())]
}

/// Which entry of the same-width bucket a character takes this frame — a
/// **frame-seeded SplitMix64**, stepped by the character's position.
///
/// This is the one part of the obfuscation that is NOT a transcription, and
/// the reason is worth stating rather than hiding. Vanilla's source is
/// `Font.random`, a nanotime-seeded `RandomSource` advanced once per
/// obfuscated glyph per draw: its output differs between two runs of the same
/// frame, so vanilla itself guarantees nothing a byte-comparing gate could
/// assert. Every gate in this project renders headlessly and compares bytes,
/// and the demo PNG has been `2cc56b4acbfb92cb` since M15 — so Rewo needs a
/// source that is reproducible given `(frame, run, index)` while still looking
/// like noise.
///
/// SplitMix64's finalizer is that: `frame` seeds the stream, `index` steps it
/// by the golden-ratio gamma, and the three xor-shift-multiply rounds
/// decorrelate adjacent positions — so a run reads as static rather than as
/// the barber pole a `(index + frame) % len` would give.
///
/// **`run` is what stops two obfuscated runs on screen scrambling
/// identically.** It is derived from the line's own origin, and — load-bearing
/// — from the UNOFFSET origin, so the drop shadow picks the same glyphs as the
/// text it shadows. A shadow of different characters is the failure this
/// argument exists to prevent.
///
/// The `% len` is biased by about 2⁻⁵⁶ for the bucket sizes here (at most
/// 256), which is below anything observable and far below vanilla's own
/// `nextInt`.
fn obfuscation_pick(frame: u64, run: u64, index: usize, len: usize) -> usize {
    const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut z = frame
        .wrapping_mul(GAMMA)
        .wrapping_add(run)
        .wrapping_add((index as u64).wrapping_mul(GAMMA));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z % len as u64) as usize
}

/// `FontSet.reload`'s `glyphsByWidth` fill, for a byte-indexed sheet.
///
/// ```java
/// if (glyph.info() != SpecialGlyphs.MISSING) {
///    glyphsByWidth.computeIfAbsent(Mth.ceil(glyph.info().getAdvance(false)), …).add(codepoint);
/// }
/// ```
///
/// The `MISSING` skip is what stops an obfuscated character turning into a
/// row of the missing-glyph box. Rewo's sheet has no missing-glyph concept, so
/// the analogue is **the cell has ink**: a blank cell is either the space or an
/// undrawn codepoint, and neither is something vanilla would offer. Read off
/// the atlas the pass is about to upload, so the buckets cannot disagree with
/// what is actually drawable.
fn glyph_width_buckets(font: &FontData<'_>) -> [Vec<u8>; 33] {
    let mut out: [Vec<u8>; 33] = std::array::from_fn(|_| Vec::new());
    let (size, cell) = (font.size as usize, font.cell as usize);
    for b in 0u16..256 {
        let (cx, cy) = ((b as usize % 16) * cell, (b as usize / 16) * cell);
        let inked = (0..cell).any(|y| {
            (0..cell).any(|x| {
                let i = ((cy + y) * size + cx + x) * 4 + 3;
                font.atlas.get(i).is_some_and(|&a| a != 0)
            })
        });
        // The white texel the bake patches into the space cell would otherwise
        // make the space look inked — and vanilla excludes the space from
        // obfuscation at the call site anyway.
        if inked && b != 32 {
            let w = font.advance[b as usize].min(32) as usize;
            out[w].push(b as u8);
        }
    }
    out
}

fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/text.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/text.frag.spv")),
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
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R | vk::ColorComponentFlags::G | vk::ColorComponentFlags::B,
            )];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
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
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&ci), None)
            .map_err(|(_, e)| format!("text pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16x16 sheet of 8-px cells, so the geometry matches the vanilla
    /// `ascii.png` the real bake produces: `size` 128, `cell` 8.
    const SIZE: u32 = 128;
    const CELL: u32 = 8;

    /// An atlas with ink in the cells named, and the white texel patched into
    /// the space cell exactly as `rewo_data::assets` does.
    fn atlas(inked: &[u8]) -> Vec<u8> {
        let mut a = vec![0u8; (SIZE * SIZE * 4) as usize];
        for &b in inked {
            let (cx, cy) = ((b as u32 % 16) * CELL, (b as u32 / 16) * CELL);
            let i = (((cy + 1) * SIZE + cx + 1) * 4 + 3) as usize;
            a[i] = 255;
        }
        let (wx, wy) = ((32 % 16) * CELL, (32 / 16) * CELL);
        let wi = (((wy * SIZE) + wx) * 4) as usize;
        a[wi..wi + 4].copy_from_slice(&[255, 255, 255, 255]);
        a
    }

    fn font<'a>(atlas: &'a [u8], advance: &'a [u8; 256]) -> FontData<'a> {
        FontData {
            atlas,
            size: SIZE,
            cell: CELL,
            advance,
            white_texel: ((32 % 16) * CELL, (32 / 16) * CELL),
        }
    }

    // -- the width provider -----------------------------------------------

    #[test]
    fn bold_costs_one_pixel_per_character_not_one_per_run() {
        // `GlyphInfo.getBoldOffset()` is 1.0F and `getAdvance(bold)` adds it to
        // EVERY glyph. Reading it as a one-off makes a long bold line measure
        // short by its own length, which is how a wrap ends up in the wrong
        // place rather than merely looking different.
        let mut adv = [0u8; 256];
        for b in b'a'..=b'z' {
            adv[b as usize] = 6;
        }
        assert_eq!(width("abcde", &adv), 30);
        assert_eq!(width_styled("abcde", &adv, true), 35);
        assert_eq!(width_styled("abcde", &adv, false), width("abcde", &adv));
    }

    // -- the obfuscation buckets ------------------------------------------

    #[test]
    fn a_bucket_holds_only_glyphs_of_its_own_width() {
        // `glyphsByWidth.computeIfAbsent(Mth.ceil(getAdvance(false)), …)`.
        let mut adv = [0u8; 256];
        adv[b'a' as usize] = 6;
        adv[b'b' as usize] = 6;
        adv[b'i' as usize] = 4;
        let a = atlas(&[b'a', b'b', b'i']);
        let buckets = glyph_width_buckets(&font(&a, &adv));
        assert_eq!(buckets[6], vec![b'a', b'b']);
        assert_eq!(buckets[4], vec![b'i']);
        assert!(buckets[5].is_empty());
    }

    #[test]
    fn a_blank_cell_is_not_a_candidate() {
        // Vanilla's `glyph.info() != SpecialGlyphs.MISSING`. A blank cell is
        // Rewo's version of that, and offering one would replace a character
        // with a hole — which reads as the text having been cut short rather
        // than scrambled.
        let mut adv = [0u8; 256];
        adv[b'a' as usize] = 6;
        adv[b'z' as usize] = 6;
        // `z` has an advance but no ink.
        let a = atlas(&[b'a']);
        let buckets = glyph_width_buckets(&font(&a, &adv));
        assert_eq!(buckets[6], vec![b'a']);
    }

    #[test]
    fn the_space_cell_is_excluded_even_though_the_bake_inks_it() {
        // The bake patches an opaque white texel into the space cell for the
        // effect quads (and for nametag backdrops), so an ink test alone would
        // offer the space as a replacement glyph — and a run of "spaces" is
        // indistinguishable from the text vanishing.
        let mut adv = [0u8; 256];
        adv[b' ' as usize] = 4;
        adv[b'i' as usize] = 4;
        let a = atlas(&[b'i']);
        let buckets = glyph_width_buckets(&font(&a, &adv));
        assert_eq!(buckets[4], vec![b'i']);
    }

    // -- the obfuscation pick ----------------------------------------------

    fn fixture() -> ([Vec<u8>; 33], [u8; 256]) {
        let mut adv = [0u8; 256];
        for b in b'a'..=b'z' {
            adv[b as usize] = 6;
        }
        adv[b'i' as usize] = 4;
        adv[b' ' as usize] = 4;
        let inked: Vec<u8> = (b'a'..=b'z').collect();
        let a = atlas(&inked);
        (glyph_width_buckets(&font(&a, &adv)), adv)
    }

    #[test]
    fn a_replacement_always_has_the_original_width() {
        // The property the whole bucket table exists for: an obfuscated run
        // must not change length, or the text jitters horizontally as it
        // scrambles.
        let (buckets, adv) = fixture();
        for frame in 0..64u64 {
            for index in 0..8usize {
                for &b in b"abcxyz" {
                    let g = obfuscated_glyph(&buckets, &adv, b, frame, 7, index);
                    assert_eq!(adv[g as usize], adv[b as usize], "{b} at {frame}/{index}");
                }
            }
        }
    }

    #[test]
    fn a_space_is_never_obfuscated() {
        // `codepoint != 32` guards the whole branch. Without it the word
        // boundaries scramble too and the run reads as one solid block.
        let (buckets, adv) = fixture();
        for frame in 0..64u64 {
            assert_eq!(obfuscated_glyph(&buckets, &adv, b' ', frame, 0, 0), b' ');
        }
    }

    #[test]
    fn an_empty_bucket_returns_the_character_itself() {
        let (buckets, adv) = fixture();
        // Advance 4 has only the un-inked `i`… so give it a width nothing has.
        let mut adv2 = adv;
        adv2[b'a' as usize] = 30;
        assert_eq!(obfuscated_glyph(&buckets, &adv2, b'a', 1, 2, 3), b'a');
    }

    #[test]
    fn the_same_frame_and_position_pick_the_same_glyph() {
        // Reproducibility is the whole reason this diverges from vanilla's
        // nanotime-seeded `RandomSource`: a headless gate compares bytes, so
        // two renders of one frame must agree.
        let (buckets, adv) = fixture();
        for index in 0..16usize {
            let a = obfuscated_glyph(&buckets, &adv, b'a', 12, 34, index);
            let b = obfuscated_glyph(&buckets, &adv, b'a', 12, 34, index);
            assert_eq!(a, b);
        }
    }

    #[test]
    fn the_run_changes_between_frames() {
        // …and it must still LOOK random. Asserted over a run rather than one
        // character, because any single character can legitimately repeat.
        let (buckets, adv) = fixture();
        let of = |frame: u64| -> Vec<u8> {
            (0..12)
                .map(|i| obfuscated_glyph(&buckets, &adv, b'a', frame, 5, i))
                .collect()
        };
        assert_ne!(of(1), of(2));
        assert_ne!(of(2), of(3));
    }

    #[test]
    fn two_runs_at_the_same_positions_differ() {
        // What `run_seed` is for. Vanilla gets this from one shared stream
        // advanced in draw order; a pure function of `(frame, index)` alone
        // would scramble every run on screen identically.
        let (buckets, adv) = fixture();
        let of = |run: u64| -> Vec<u8> {
            (0..12)
                .map(|i| obfuscated_glyph(&buckets, &adv, b'a', 9, run, i))
                .collect()
        };
        assert_ne!(of(100), of(200));
    }

    #[test]
    fn the_shadow_copy_scrambles_identically() {
        // `run_seed` reads `line.x`/`line.y`, not the `ox`/`oy` the shadow pass
        // adds — so the two `push_line` calls for one shadowed run agree. A
        // seed that moved with the offset would draw a shadow of DIFFERENT
        // characters, which reads as doubled garbage rather than as a shadow.
        let line = TextLine {
            x: 4.0,
            y: 100.0,
            px: 2.0,
            color: [1.0; 3],
            alpha: 1.0,
            shadow: true,
            style: TextStyle {
                obfuscated: true,
                ..TextStyle::PLAIN
            },
            text: "abc",
        };
        assert_eq!(run_seed(&line), run_seed(&line));
        // And two runs one pixel apart do NOT agree, which is what makes the
        // assertion above about the offset rather than about determinism.
        let moved = TextLine { x: 5.0, ..line };
        assert_ne!(run_seed(&line), run_seed(&moved));
    }

    #[test]
    fn the_pick_is_in_range() {
        for frame in 0..32u64 {
            for run in [0u64, 7, 1 << 40] {
                for index in 0..32usize {
                    for len in 1..40usize {
                        assert!(obfuscation_pick(frame, run, index, len) < len);
                    }
                }
            }
        }
    }

    #[test]
    fn adjacent_positions_are_not_correlated() {
        // The reason for a real mixing function rather than `(index + frame)
        // % len`: that reads as a barber pole, every character stepping its
        // bucket in lockstep. Measured as "the deltas between neighbours take
        // many different values", which a linear pick cannot do.
        let deltas: std::collections::HashSet<i64> = (0..64)
            .map(|i| {
                let a = obfuscation_pick(3, 11, i, 26) as i64;
                let b = obfuscation_pick(3, 11, i + 1, 26) as i64;
                b - a
            })
            .collect();
        assert!(deltas.len() > 10, "only {} distinct deltas", deltas.len());
    }

    // -- the italic shear ---------------------------------------------------

    #[test]
    fn the_shear_is_plus_one_over_minus_one_for_the_vanilla_sheet() {
        // `shearTop = 1.0 - 0.25 * up`, `shearBottom = 1.0 - 0.25 * down`,
        // with `up = 0` and `down = 8` for `ascii.png` (`ascent: 7`, so
        // `getTop() = 7 - 7 = 0`, and the 8-px cell below it). The formula is
        // what is transcribed; these are the numbers it produces.
        assert_eq!(shear_for_cell(8.0, true), (1.0, -1.0));
        assert_eq!(shear_for_cell(8.0, false), (0.0, 0.0));
        // A taller cell leans further, which is what makes it a formula rather
        // than a pair of constants.
        assert_eq!(shear_for_cell(12.0, true), (1.0, -2.0));
    }
}
