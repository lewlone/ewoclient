//! Clouds — `CloudRenderer` + `core/rendertype_clouds.vsh`, ported exactly
//! (M33).
//!
//! Ground truth: `net/minecraft/client/renderer/CloudRenderer.java` and
//! `assets/minecraft/shaders/core/rendertype_clouds.{vsh,fsh}` in the 26.2
//! decompile.
//!
//! # The shape of it
//!
//! Clouds carry **no texture**. `textures/environment/clouds.png` is a *map*,
//! not a surface: one texel is one 12×12×4-block cell, and the only thing read
//! out of it is whether a cell is occupied (alpha ≥ 10) and what colour it
//! carries. Each cell is packed into a `u64` with its four horizontal
//! neighbours' emptiness, and the mesh is built by walking rings outward from
//! the camera's cell and emitting **three bytes per quad** — cell x, cell z,
//! and a direction-plus-flags byte. The vertex shader expands those into
//! corners from a fixed table and shades them from six fixed face colours.
//!
//! That is why the mesh is rebuilt only when the camera crosses a cell
//! boundary: it is position-independent, and per-frame motion rides entirely in
//! the sub-cell offset uniform.
//!
//! # Two things worth not re-deriving
//!
//! `isCellEmpty` is `ARGB.alpha(color) < 10` — a threshold, not a test for
//! zero. And the neighbour lookups in `prepare` wrap with `Math.floorMod`, but
//! **the east and west lookups wrap `x` against `height`, not `width`**
//! (`texture.getPixel(Math.floorMod(x + 1, height), y)`). On vanilla's square
//! `clouds.png` that is invisible; it is transcribed as written, because a
//! resource pack with a non-square cloud map would diverge, and quietly
//! "fixing" it would be inventing behaviour.

use ash::vk;

use crate::buf_ring::{BufRing, BUF_RING};
use crate::Gpu;

/// `CELL_SIZE_IN_BLOCKS`.
pub const CELL_SIZE: f32 = 12.0;
/// The cloud slab's thickness — the `4.0F` in `relativeTopY`, and the `y` of
/// the `CellSize` uniform.
pub const CELL_HEIGHT: f32 = 4.0;
/// `TICKS_PER_CELL` — the texture repeats after `width * 400` ticks.
pub const TICKS_PER_CELL: i64 = 400;
/// The per-tick drift, straight from `cloudOffset * 0.030000001F`.
pub const DRIFT_PER_TICK: f32 = 0.030_000_001;
/// The fixed `cameraPosition.z + 3.96F` bias.
pub const Z_BIAS: f64 = 3.96;
/// `isCellEmpty`: alpha **below 10**, not zero.
pub const EMPTY_ALPHA: u8 = 10;

const FLAG_INSIDE_FACE: i32 = 16;
const FLAG_USE_TOP_COLOR: i32 = 32;

/// `Direction.get3DDataValue()` — the order the shader's vertex table is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Face {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

/// `CloudRenderer.RelativeCameraPos`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelativeCameraPos {
    AboveClouds,
    InsideClouds,
    BelowClouds,
}

impl RelativeCameraPos {
    /// The classification `render` does from the camera-relative slab bounds.
    pub fn of(relative_bottom_y: f32) -> Self {
        let relative_top_y = relative_bottom_y + CELL_HEIGHT;
        if relative_top_y < 0.0 {
            Self::AboveClouds
        } else if relative_bottom_y > 0.0 {
            Self::BelowClouds
        } else {
            Self::InsideClouds
        }
    }
}

/// `CloudStatus` — vanilla's video option. FANCY extrudes the cells into boxes;
/// FAST draws a single downward-facing quad per cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudStatus {
    Fancy,
    Fast,
}

/// One decoded `clouds.png`, packed into `CloudRenderer.TextureData`.
#[derive(Clone, Debug, PartialEq)]
pub struct CloudTexture {
    /// `packCellData(color, north, east, south, west)` per texel, or 0 for an
    /// empty cell.
    pub cells: Vec<u64>,
    pub width: u32,
    pub height: u32,
}

impl CloudTexture {
    /// `CloudRenderer.prepare` — the RGBA8 image to packed cells.
    ///
    /// `color` is read as ARGB the way `NativeImage.getPixel` returns it.
    pub fn from_rgba(rgba: &[u8], width: u32, height: u32) -> Self {
        let px = |x: u32, y: u32| -> u32 {
            let i = ((y * width + x) * 4) as usize;
            let (r, g, b, a) = (rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]);
            ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
        };
        let empty = |c: u32| ((c >> 24) & 0xFF) < EMPTY_ALPHA as u32;
        let mut cells = vec![0u64; (width * height) as usize];
        for y in 0..height {
            for x in 0..width {
                let color = px(x, y);
                if empty(color) {
                    continue;
                }
                // Verbatim, including that east/west wrap `x` against `height`.
                let north = empty(px(x, (y + height - 1) % height));
                let east = empty(px((x + 1) % height, y));
                let south = empty(px(x, (y + 1) % height));
                let west = empty(px((x + height - 1) % height, y));
                cells[(x + y * width) as usize] = pack_cell(color, north, east, south, west);
            }
        }
        Self {
            cells,
            width,
            height,
        }
    }

    fn cell(&self, cell_x: i32, relative_x: i32, cell_z: i32, relative_z: i32) -> u64 {
        let ix = (cell_x + relative_x).rem_euclid(self.width as i32);
        let iy = (cell_z + relative_z).rem_euclid(self.height as i32);
        self.cells[(ix + iy * self.width as i32) as usize]
    }

    /// `buildMesh` — the ring walk, returning one `(x, z, dir_and_flags)` per
    /// quad in emission order.
    ///
    /// The rings run `|dx| + |dz| = ring`, which walks outward in a diamond, so
    /// nearer cells are emitted first. Within a ring both `+dz` and `-dz` are
    /// built, the `-dz` copy skipped at `dz == 0` so the axis is not doubled.
    pub fn build_mesh(
        &self,
        relative_pos: RelativeCameraPos,
        center_cell_x: i32,
        center_cell_z: i32,
        status: CloudStatus,
        radius_cells: i32,
    ) -> Vec<[i32; 3]> {
        let extrude = status == CloudStatus::Fancy;
        let mut faces = Vec::new();
        for ring in 0..=2 * radius_cells {
            for dx in -ring..=ring {
                let dz = ring - dx.abs();
                if dz < 0 || dz > radius_cells || dx * dx + dz * dz > radius_cells * radius_cells {
                    continue;
                }
                if dz != 0 {
                    self.try_build_cell(
                        &mut faces,
                        relative_pos,
                        center_cell_x,
                        center_cell_z,
                        extrude,
                        dx,
                        -dz,
                    );
                }
                self.try_build_cell(
                    &mut faces,
                    relative_pos,
                    center_cell_x,
                    center_cell_z,
                    extrude,
                    dx,
                    dz,
                );
            }
        }
        faces
    }

    #[allow(clippy::too_many_arguments)]
    fn try_build_cell(
        &self,
        out: &mut Vec<[i32; 3]>,
        relative_pos: RelativeCameraPos,
        cell_x: i32,
        cell_z: i32,
        extrude: bool,
        dx: i32,
        dz: i32,
    ) {
        let data = self.cell(cell_x, dx, cell_z, dz);
        if data == 0 {
            return;
        }
        if extrude {
            build_extruded_cell(out, relative_pos, dx, dz, data);
        } else {
            // `buildFlatCell` — one DOWN face, flagged to use the *top* colour.
            encode_face(out, dx, dz, Face::Down, FLAG_USE_TOP_COLOR);
        }
    }
}

/// `packCellData` — the ARGB colour in the high bits, four emptiness flags low.
pub fn pack_cell(color: u32, north: bool, east: bool, south: bool, west: bool) -> u64 {
    ((color as i32) as i64 as u64) << 4
        | (north as u64) << 3
        | (east as u64) << 2
        | (south as u64) << 1
        | (west as u64)
}

fn north_empty(d: u64) -> bool {
    (d >> 3) & 1 != 0
}
fn east_empty(d: u64) -> bool {
    (d >> 2) & 1 != 0
}
fn south_empty(d: u64) -> bool {
    (d >> 1) & 1 != 0
}
fn west_empty(d: u64) -> bool {
    d & 1 != 0
}

/// `encodeFace` — three bytes: `x >> 1`, `z >> 1`, and direction | flags with
/// the two dropped low bits folded into bits 7 and 6.
///
/// Stored here as sign-extended `i32`s (see the shader's module comment on why
/// that is a representation change only).
fn encode_face(out: &mut Vec<[i32; 3]>, x: i32, z: i32, direction: Face, flags: i32) {
    let mut dir_and_flags = direction as i32 | flags;
    dir_and_flags |= (x & 1) << 7;
    dir_and_flags |= (z & 1) << 6;
    // The Java `(byte)` casts, reproduced: the arithmetic shift keeps a
    // negative cell negative, and `dirAndFlags` becomes negative once bit 7 is
    // set — which the shader's `& FLAG_EXTRA_X` recovers regardless.
    out.push([
        (x >> 1) as i8 as i32,
        (z >> 1) as i8 as i32,
        dir_and_flags as u8 as i8 as i32,
    ]);
}

/// `buildExtrudedCell`.
fn build_extruded_cell(
    out: &mut Vec<[i32; 3]>,
    relative_pos: RelativeCameraPos,
    x: i32,
    z: i32,
    data: u64,
) {
    if relative_pos != RelativeCameraPos::BelowClouds {
        encode_face(out, x, z, Face::Up, 0);
    }
    if relative_pos != RelativeCameraPos::AboveClouds {
        encode_face(out, x, z, Face::Down, 0);
    }
    // A side is drawn only when the neighbour is empty **and** the cell is on
    // the far side of the camera in that axis — a side facing away is never
    // visible, so it is never built.
    if north_empty(data) && z > 0 {
        encode_face(out, x, z, Face::North, 0);
    }
    if south_empty(data) && z < 0 {
        encode_face(out, x, z, Face::South, 0);
    }
    if west_empty(data) && x > 0 {
        encode_face(out, x, z, Face::West, 0);
    }
    if east_empty(data) && x < 0 {
        encode_face(out, x, z, Face::East, 0);
    }
    // The nine cells around the camera get all six faces again, inward-wound,
    // so a camera inside the cloud still sees it.
    if x.abs() <= 1 && z.abs() <= 1 {
        for direction in [
            Face::Down,
            Face::Up,
            Face::North,
            Face::South,
            Face::West,
            Face::East,
        ] {
            encode_face(out, x, z, direction, FLAG_INSIDE_FACE);
        }
    }
}

/// Everything the per-frame uniform needs, derived from the camera and clock.
///
/// Separated from the Vulkan pass so the gate can grade the arithmetic without
/// a device.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudPlacement {
    pub cell_x: i32,
    pub cell_z: i32,
    /// The mesh's origin, in **WORLD** space.
    ///
    /// Vanilla's is camera-relative — `(-xInCell, relativeBottomY, -zInCell)` —
    /// because its model-view carries the camera translation. Rewo's
    /// `view_proj` already includes it, so the camera is added back here or the
    /// whole deck would hang over the world origin. That is the same correction
    /// the weather columns need, and for the same reason.
    pub offset: [f32; 3],
    pub relative_pos: RelativeCameraPos,
}

/// The placement half of `CloudRenderer.render`.
pub fn placement(
    camera: [f64; 3],
    bottom_y: f32,
    game_time: i64,
    partial_ticks: f32,
    texture_width: u32,
    texture_height: u32,
) -> CloudPlacement {
    let relative_bottom_y = bottom_y - camera[1] as f32;
    // `gameTime % (width * 400L)` — the drift wraps exactly when the texture
    // does, so the sky never jumps.
    let cloud_offset = (game_time.rem_euclid(texture_width as i64 * TICKS_PER_CELL)) as f32
        + partial_ticks;
    let mut cloud_x = camera[0] + (cloud_offset * DRIFT_PER_TICK) as f64;
    let mut cloud_z = camera[2] + Z_BIAS;
    let width_blocks = texture_width as f64 * CELL_SIZE as f64;
    let height_blocks = texture_height as f64 * CELL_SIZE as f64;
    cloud_x -= (cloud_x / width_blocks).floor() * width_blocks;
    cloud_z -= (cloud_z / height_blocks).floor() * height_blocks;
    let cell_x = (cloud_x / CELL_SIZE as f64).floor() as i32;
    let cell_z = (cloud_z / CELL_SIZE as f64).floor() as i32;
    let x_in_cell = (cloud_x - cell_x as f64 * CELL_SIZE as f64) as f32;
    let z_in_cell = (cloud_z - cell_z as f64 * CELL_SIZE as f64) as f32;
    CloudPlacement {
        cell_x,
        cell_z,
        // `camera + (-xInCell, relativeBottomY, -zInCell)`, which in y is just
        // the deck's own height.
        offset: [
            camera[0] as f32 - x_in_cell,
            bottom_y,
            camera[2] as f32 - z_in_cell,
        ],
        // The classification still keys off the CAMERA-relative height: which
        // side of the deck you are on is not a world-space question.
        relative_pos: RelativeCameraPos::of(relative_bottom_y),
    }
}

/// `range * 16` blocks, in cells — `Mth.ceil(radiusBlocks / 12.0F)`.
pub fn radius_cells(render_distance_chunks: i32) -> i32 {
    let radius_blocks = render_distance_chunks * 16;
    (radius_blocks as f32 / CELL_SIZE).ceil() as i32
}

// -- the Vulkan pass ----------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CloudInfo {
    color: [f32; 4],
    offset: [f32; 4],
    cell_size: [f32; 4],
    camera: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Push {
    mvp: [[f32; 4]; 4],
    fog_clouds_end: f32,
    _pad: [f32; 3],
}

/// One frame's clouds, ready to draw.
pub struct CloudDraw {
    pub faces: Vec<[i32; 3]>,
    pub placement: CloudPlacement,
    /// The eye in world space — the fog fade measures from it, because the
    /// positions are world-space rather than vanilla's camera-relative.
    pub camera: [f32; 3],
    /// The dimension's `visual/cloud_color`, as straight ARGB. Alpha 0 means
    /// the caller should not have built this at all.
    pub color_argb: i32,
    pub fog_clouds_end: f32,
}

pub struct CloudPass {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    /// One descriptor set **per ring slot** (M86).
    ///
    /// This pass is the only one whose per-frame data reaches the shader
    /// through a descriptor set rather than a vertex binding, and
    /// `vkUpdateDescriptorSets` is illegal on a set that a pending command
    /// buffer has bound. Rewriting one set every frame was therefore a second
    /// hazard sitting on top of the buffer one, and it needs the same remedy:
    /// set `i` describes slot `i`, so a set is only ever rewritten when its
    /// buffers are, i.e. once per [`BUF_RING`] frames.
    sets: [vk::DescriptorSet; BUF_RING],
    /// This frame's uniform and face list. See `buf_ring`'s module docs.
    ubo: BufRing,
    faces: BufRing,
    quad_count: u32,
    fog_clouds_end: f32,
}

impl CloudPass {
    pub fn new(gpu: &mut Gpu, color_format: vk::Format) -> Result<Self, String> {
        let device = gpu.device.clone();
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX),
        ];
        let set_layout = unsafe {
            device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| format!("cloud set layout: {e}"))?
        };
        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(BUF_RING as u32),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(BUF_RING as u32),
        ];
        let pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(BUF_RING as u32)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .map_err(|e| format!("cloud pool: {e}"))?
        };
        let set_layouts = [set_layout];
        let ring_layouts = [set_layout; BUF_RING];
        let sets: [vk::DescriptorSet; BUF_RING] = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&ring_layouts),
                )
                .map_err(|e| format!("cloud set: {e}"))?
                .try_into()
                .map_err(|_| "cloud set: wrong count".to_string())?
        };
        let push_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(std::mem::size_of::<Push>() as u32)];
        let layout = unsafe {
            device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&push_range),
                    None,
                )
                .map_err(|e| format!("cloud layout: {e}"))?
        };
        let pipeline = build_pipeline(&device, layout, color_format)?;
        Ok(Self {
            pipeline,
            layout,
            set_layout,
            pool,
            sets,
            ubo: BufRing::new(),
            faces: BufRing::new(),
            quad_count: 0,
            fog_clouds_end: 0.0,
        })
    }

    /// Upload one frame's mesh + uniform. An empty mesh is legal and draws
    /// nothing — which is also what a dimension with a transparent cloud
    /// colour should produce, though the caller is expected to skip us
    /// entirely in that case.
    pub fn set_draw(&mut self, gpu: &mut Gpu, draw: &CloudDraw) -> Result<(), String> {
        self.quad_count = draw.faces.len() as u32;
        self.fog_clouds_end = draw.fog_clouds_end;
        if draw.faces.is_empty() {
            self.ubo.clear(gpu);
            self.faces.clear(gpu);
            return Ok(());
        }
        let argb = draw.color_argb as u32;
        let ch = |shift: u32| ((argb >> shift) & 0xFF) as f32 / 255.0;
        // `ARGB.vector4fFromARGB32` — the colour reaches the shader as a plain
        // multiplier. It is linearized for the same reason `end_sky` linearizes
        // its constant: the attachment re-encodes on store, so a gamma-space
        // multiply has to be applied in linear to come back out proportional.
        let info = CloudInfo {
            color: [
                srgb_to_linear(ch(16)),
                srgb_to_linear(ch(8)),
                srgb_to_linear(ch(0)),
                ch(24),
            ],
            offset: [
                draw.placement.offset[0],
                draw.placement.offset[1],
                draw.placement.offset[2],
                0.0,
            ],
            cell_size: [CELL_SIZE, CELL_HEIGHT, CELL_SIZE, 0.0],
            camera: [draw.camera[0], draw.camera[1], draw.camera[2], 0.0],
        };
        self.ubo.set(
            gpu,
            bytemuck::bytes_of(&info),
            vk::BufferUsageFlags::UNIFORM_BUFFER,
        )?;
        let flat: Vec<i32> = draw.faces.iter().flat_map(|f| f.iter().copied()).collect();
        self.faces.set(
            gpu,
            bytemuck::cast_slice(&flat),
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        // The two rings are written together on every path, so they are always
        // on the same slot — and that slot's descriptor set is the one to
        // rewrite. Asserted rather than assumed: a future edit that clears one
        // without the other would otherwise describe slot A's uniform with
        // slot B's faces, which validation cannot see.
        debug_assert_eq!(self.ubo.cursor(), self.faces.cursor());
        let slot = self.ubo.cursor();
        let (Some(ubo), Some(faces)) = (self.ubo.peek(), self.faces.peek()) else {
            return Err("cloud: buffers vanished after upload".into());
        };
        let ubo_info = [vk::DescriptorBufferInfo::default()
            .buffer(ubo)
            .range(vk::WHOLE_SIZE)];
        let faces_info = [vk::DescriptorBufferInfo::default()
            .buffer(faces)
            .range(vk::WHOLE_SIZE)];
        unsafe {
            gpu.device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.sets[slot])
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                        .buffer_info(&ubo_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.sets[slot])
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .buffer_info(&faces_info),
                ],
                &[],
            );
        }
        Ok(())
    }

    pub fn draw(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        if self.quad_count == 0 {
            return;
        }
        // Claim both slots, so the ring knows this frame reads them.
        let (Some(_), Some(_)) = (self.ubo.bind(), self.faces.bind()) else {
            return;
        };
        let set = self.sets[self.ubo.cursor()];
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[set],
                &[],
            );
            let push = Push {
                mvp: view_proj,
                fog_clouds_end: self.fog_clouds_end,
                _pad: [0.0; 3],
            };
            device.cmd_push_constants(
                cb,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytemuck::bytes_of(&push),
            );
            // Six vertices per quad, no vertex or index buffer at all: the
            // shader reads the three-int face record straight out of the
            // storage buffer and maps `gl_VertexIndex % 6` back onto the
            // quad's four corners.
            device.cmd_draw(cb, self.quad_count * 6, 1, 0, 0);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        self.ubo.destroy(gpu);
        self.faces.destroy(gpu);
        let device = gpu.device.clone();
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_449_936 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
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
            include_bytes!(concat!(env!("OUT_DIR"), "/clouds.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/clouds.frag.spv")),
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
        // No vertex buffer: every attribute is computed from `gl_VertexIndex`.
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        // `RenderPipelines.CLOUDS` culls; `FLAT_CLOUDS` sets `withCull(false)`.
        // Culling is what the interior faces exist for — without it every cell
        // draws both sides of every quad and the translucent blend doubles up.
        // The front-face convention is COUNTER_CLOCKWISE, and that is measured
        // rather than reasoned: `weathershot`'s g2 renders a solid deck from
        // both above and below, and CLOCKWISE covers 6394/880 pixels where
        // COUNTER_CLOCKWISE covers 15550/10503. CLOCKWISE looked right from
        // below alone, which is why the witness grades both sides.
        //
        // Note that BACK+CCW measures identically to no culling at all here:
        // the mesh only builds faces on the camera's side of each cell, so
        // culling's real job is the inward-wound interior faces, not the deck.
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // `DepthStencilState.DEFAULT` — tested and written. Reversed-Z, so
        // GREATER rather than vanilla's LESS.
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::GREATER);
        // `BlendFunction.TRANSLUCENT` — src-alpha over one-minus-src-alpha,
        // which is how the cloud colour's 0xcc alpha reaches the screen.
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
                    | vk::ColorComponentFlags::B
                    | vk::ColorComponentFlags::A,
            )];
        let blend_state =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [color_format];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&color_formats)
            .depth_attachment_format(crate::world::DEPTH_FORMAT);
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
            .map_err(|(_, e)| format!("cloud pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `side`x`side` map with exactly one occupied cell, at (1, 1).
    ///
    /// `side` matters: the cell walk wraps with `floorMod`, so a map smaller
    /// than the radius shows the same cell more than once. Tests that care
    /// about *which* faces one cell builds must stay inside one wrap.
    fn one_cell_in(side: u32) -> CloudTexture {
        let mut rgba = vec![0u8; (side * side * 4) as usize];
        let i = ((side + 1) * 4) as usize;
        rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
        CloudTexture::from_rgba(&rgba, side, side)
    }

    fn one_cell() -> CloudTexture {
        one_cell_in(4)
    }

    #[test]
    fn the_empty_threshold_is_alpha_below_ten() {
        // Alpha 9 is empty, alpha 10 is not — `< 10`, not `== 0`.
        for (alpha, occupied) in [(0u8, false), (9, false), (10, true), (255, true)] {
            let mut rgba = vec![0u8; 4];
            rgba.copy_from_slice(&[255, 255, 255, alpha]);
            let t = CloudTexture::from_rgba(&rgba, 1, 1);
            assert_eq!(
                t.cells[0] != 0,
                occupied,
                "alpha {alpha} should be {}",
                if occupied { "solid" } else { "empty" }
            );
        }
    }

    #[test]
    fn an_isolated_cell_has_all_four_neighbours_empty() {
        let t = one_cell();
        let d = t.cells[1 * 4 + 1];
        assert!(north_empty(d) && east_empty(d) && south_empty(d) && west_empty(d));
        // And the colour survives the pack, shifted up four bits.
        assert_eq!((d >> 4) as u32, 0xFFFF_FFFF);
    }

    #[test]
    fn a_flat_cell_is_one_down_face_using_the_top_colour() {
        let t = one_cell();
        // Centre the walk on the occupied cell so it is the only hit.
        let faces = t.build_mesh(RelativeCameraPos::BelowClouds, 1, 1, CloudStatus::Fast, 1);
        assert_eq!(faces.len(), 1, "FAST is one quad per cell");
        let [x, z, flags] = faces[0];
        assert_eq!((x, z), (0, 0));
        assert_eq!(flags & 7, Face::Down as i32);
        assert_ne!(
            flags & FLAG_USE_TOP_COLOR,
            0,
            "a flat cloud is lit as its top, not its underside"
        );
    }

    /// Above the clouds you never see their bottom; below, never their top.
    #[test]
    fn the_camera_side_decides_which_horizontal_face_is_built() {
        let t = one_cell();
        let dirs = |p| {
            t.build_mesh(p, 1, 1, CloudStatus::Fancy, 1)
                .into_iter()
                // Ignore the interior faces the centre cell always gets.
                .filter(|f| f[2] & FLAG_INSIDE_FACE == 0)
                .map(|f| f[2] & 7)
                .collect::<Vec<_>>()
        };
        assert_eq!(dirs(RelativeCameraPos::AboveClouds), vec![Face::Up as i32]);
        assert_eq!(dirs(RelativeCameraPos::BelowClouds), vec![Face::Down as i32]);
        assert_eq!(
            dirs(RelativeCameraPos::InsideClouds),
            vec![Face::Up as i32, Face::Down as i32]
        );
    }

    /// The nine cells around the camera get a full inward-wound set on top of
    /// whatever else they built.
    #[test]
    fn the_centre_cell_gets_six_interior_faces() {
        let t = one_cell();
        let faces = t.build_mesh(RelativeCameraPos::InsideClouds, 1, 1, CloudStatus::Fancy, 1);
        let interior: Vec<i32> = faces
            .iter()
            .filter(|f| f[2] & FLAG_INSIDE_FACE != 0)
            .map(|f| f[2] & 7)
            .collect();
        assert_eq!(interior.len(), 6);
        for d in 0..6 {
            assert!(interior.contains(&d), "missing interior face {d}");
        }
    }

    /// A side is built only when the neighbour is empty AND the cell sits on
    /// the far side of the camera — the "and" is what stops a solid cloud bank
    /// from emitting its own hidden interior walls.
    #[test]
    fn sides_are_built_only_away_from_the_camera() {
        // 16 wide, so a radius-3 walk cannot wrap around onto the same cell
        // from the other side and build its opposite face too.
        let t = one_cell_in(16);
        // Put the camera two cells west so the cell sits at dx = +2, dz = 0:
        // out of interior range, and only its WEST side faces us.
        let faces = t.build_mesh(RelativeCameraPos::InsideClouds, -1, 1, CloudStatus::Fancy, 3);
        let sides: Vec<i32> = faces
            .iter()
            .filter(|f| f[2] & FLAG_INSIDE_FACE == 0)
            .map(|f| f[2] & 7)
            .filter(|d| *d >= Face::North as i32)
            .collect();
        assert_eq!(sides, vec![Face::West as i32]);
    }

    /// The low bit of each coordinate rides in the flag byte; the shader
    /// reassembles it. A cell at an odd coordinate that lost its low bit would
    /// render 12 blocks away.
    #[test]
    fn odd_cell_coordinates_survive_the_byte_packing() {
        for (x, z) in [(3i32, 5i32), (-3, -5), (7, -1), (-8, 2)] {
            let mut out = Vec::new();
            encode_face(&mut out, x, z, Face::Up, 0);
            let [ex, ez, flags] = out[0];
            let rx = (ex << 1) | ((flags & 128) >> 7);
            let rz = (ez << 1) | ((flags & 64) >> 6);
            assert_eq!((rx, rz), (x, z), "round trip for ({x}, {z})");
        }
    }

    #[test]
    fn the_slab_classification_switches_at_its_own_faces() {
        // The slab spans [bottom, bottom + 4] relative to the camera.
        assert_eq!(
            RelativeCameraPos::of(-5.0),
            RelativeCameraPos::AboveClouds,
            "top below the eye"
        );
        assert_eq!(RelativeCameraPos::of(-4.0), RelativeCameraPos::InsideClouds);
        assert_eq!(RelativeCameraPos::of(0.0), RelativeCameraPos::InsideClouds);
        assert_eq!(
            RelativeCameraPos::of(0.1),
            RelativeCameraPos::BelowClouds,
            "bottom above the eye"
        );
    }

    #[test]
    fn the_drift_wraps_with_the_texture_rather_than_jumping() {
        // At `gameTime = width * 400` the offset returns to 0, so the sky is
        // exactly where it started.
        let t = CloudTexture::from_rgba(&[0u8; 4 * 4 * 4], 4, 4);
        let a = placement([0.0, 100.0, 0.0], 192.33, 0, 0.0, t.width, t.height);
        let b = placement(
            [0.0, 100.0, 0.0],
            192.33,
            t.width as i64 * TICKS_PER_CELL,
            0.0,
            t.width,
            t.height,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn the_radius_rounds_up_to_whole_cells() {
        // 12 chunks = 192 blocks = exactly 16 cells; 13 chunks = 208 blocks
        // needs 18 (17.33 rounded up).
        assert_eq!(radius_cells(12), 16);
        assert_eq!(radius_cells(13), 18);
    }
}
